import { useEffect, useRef, useState } from 'react';
import { useSelector } from '@xstate/react';
import type { SkillDescriptor, SourceScope, SourceSettings, SourceTarget, TransferOperation, WorkspaceCatalogAuthority } from '../../../../../protocol/app-server/v81';
import type { AppServerClient } from '../../../client/app-server';
import { useTranslation } from '../../../locale/react';
import type { CatalogView } from '../../../presentation/catalog/CatalogControls';
import { CardMenu, type CardMenuItems } from '../../../presentation/catalog/CardMenu';
import { IconEditOutline16, IconTrashOutline16 } from '../../../presentation/primitives/icons';
import { SkillIcon } from '../skills/SkillIcon';
import { SkillDeleteDialog } from '../skills/SkillDeleteDialog';
import { SkillDisplayDialog, type SkillDisplayWriter } from '../skills/SkillDisplayDialog';
import { SkillDetail } from '../skills/SkillDetail';
import { useSkillCatalog } from '../skills/useSkillCatalog';
import { useSettingsActor } from '../../settings/machines/react';
import { extensionEntries } from './inventory';
import { ResourceAvailability, SkillSelectionPolicy } from './ResourceAvailability';
import { ResourceCard, ResourceGroup } from './ResourceCards';
import { transferActions } from './ResourceTransfer';

const userTarget = { kind: 'user' } as const;
const rowKey = (scope: SourceScope, name: string) => `${scope}:${name}`;

/** Default-enabled names from a settled catalog. A pending or failed
 * rescan is not a complete directory, so it cannot expand `all` into an exact
 * list. Exact edits and none/all/inherit do not use this list. */
export function defaultEnabledSkillNames(catalog: { pending?: boolean; error?: string; skills: readonly { disable_model_invocation: boolean; enabled_by_default: boolean; catalog: { name: string } }[] } | undefined): string[] | undefined {
  if (!catalog || catalog.pending || catalog.error) return undefined;
  return catalog.skills.filter(skill => !skill.disable_model_invocation && skill.enabled_by_default).map(skill => skill.catalog.name);
}

export function ResourceBrowser({ client, source: observedSource, target, scope, revision, query, view, refresh, onRefresh, workspaceLabel, executionSession,
  bindDisplayWrite, bindDelete, onTransfer, transferDisabled, onCatalogState }: {
  client: AppServerClient; source?: SourceSettings; target: SourceTarget; scope: SourceScope; revision?: string;
  bindDisplayWrite: (skill: SkillDescriptor) => SkillDisplayWriter | undefined;
  bindDelete: (skill: SkillDescriptor) => (() => Promise<void>) | undefined;
  onTransfer?: (from: SourceScope, name: string, revision: string, operation: TransferOperation, taken: ReadonlySet<string>) => void;
  transferDisabled: boolean; workspaceLabel?: string; executionSession?: string;
  query: string; view: CatalogView; refresh: number; onRefresh: () => void;
  onCatalogState?: (state: { status: 'loading' | 'ready' | 'failed'; authority?: WorkspaceCatalogAuthority }) => void;
}) {
  const tx = useTranslation();
  const [localRefresh, setLocalRefresh] = useState(0);
  const [focus, setFocus] = useState<{ name: string; owner: SourceScope }>();
  const [editing, setEditing] = useState<{ skill: SkillDescriptor; write: SkillDisplayWriter }>();
  const [deleting, setDeleting] = useState<{ title: string; owner: SourceScope; path: string; remove: () => Promise<void> }>();
  const settings = useSettingsActor();
  const sourceReading = useSelector(settings, state => state.matches({ authority: { attached: 'reading' } }));
  const effective = useSkillCatalog(client, observedSource, true, scope, refresh + localRefresh, executionSession, target);
  const global = useSkillCatalog(client, observedSource, scope === 'workspace', 'user', refresh + localRefresh);
  const catalogStatus: 'loading' | 'ready' | 'failed' = !effective || effective.pending ? 'loading' : effective.error ? 'failed' : 'ready';
  // A source refresh keeps the last response's authority while the follow-up
  // request is pending. The response that supplies the rows also gates edits.
  const catalogAuthority = effective?.authority;
  const canAuthor = !executionSession || catalogAuthority === 'authoring';
  const source = canAuthor ? observedSource : undefined;
  if (executionSession && catalogAuthority === 'client') {
    if (editing) setEditing(undefined);
    if (deleting) setDeleting(undefined);
  }
  const reportCatalogState = useRef(onCatalogState);
  reportCatalogState.current = onCatalogState;
  useEffect(() => {
    reportCatalogState.current?.({ status: catalogStatus, authority: catalogAuthority });
  }, [catalogAuthority, catalogStatus]);
  const listed = effective?.skills ?? [];
  // The effective catalog drives selection. Global's own catalog keeps shadowed
  // definitions accessible by their exact owning scope.
  const skills = scope === 'workspace' && global && !global.error
    ? [...listed.filter(skill => skill.provenance.source === 'workspace'), ...global.skills]
    : listed;
  const descriptors = new Map(skills.map(skill => [rowKey(skill.provenance.source, skill.catalog.name), skill]));
  // A source read in flight has not yet produced the catalog's source. Withhold
  // names until that read and the catalog are both settled, so all→exact cannot
  // start on the previous directory.
  const skillNames = sourceReading ? undefined : defaultEnabledSkillNames(effective);
  const selectable = new Set(listed.map(skill => rowKey(skill.provenance.source, skill.catalog.name)));
  const catalogError = effective?.error ?? global?.error;
  const entries = new Map(extensionEntries(source, scope, 'skill').map(entry => [rowKey(entry.owner, entry.name), entry]));
  for (const [key, skill] of descriptors) {
    if (entries.has(key)) continue;
    entries.set(key, {
      family: 'skill', name: skill.catalog.name, owner: skill.provenance.source, path: skill.catalog.location,
      relationship: skill.provenance.source === scope ? 'authored' : 'inherited', valid: true, diagnostics: [],
      capability: {family:'skill', inventory:'native', authoring:'none', selection:'skills', preparation:false, diagnostics:true},
    });
  }
  const needle = query.trim().toLocaleLowerCase();
  const matches = [...entries.values()].filter(row => {
    const skill = descriptors.get(rowKey(row.owner, row.name));
    return [row.name, row.path, skill?.catalog.description, skill?.display?.authored.display_name, skill?.display?.authored.short_description]
      .some(value => value?.toLocaleLowerCase().includes(needle));
  });
  const owned = matches.filter(row => row.owner === scope);
  const inherited = matches.filter(row => row.owner !== scope);

  const renderCard = (row: typeof matches[number]) => {
    const key = rowKey(row.owner, row.name);
    const skill = descriptors.get(key);
    const display = skill?.display;
    const operationRevision = skill?.operation_revision;
    const name = display?.authored.display_name ?? row.name;
    const items: CardMenuItems = !canAuthor ? [] : [
      ...(display?.revision ? [{ id: 'edit', label: tx('resources:skills.edit-display'), icon: <IconEditOutline16 /> }] : []),
      ...(operationRevision && onTransfer ? transferActions(tx, row.owner, transferDisabled || !effective || !!effective.error
        || effective.pending || !global || !!global.error || global.pending) : []),
      ...(operationRevision ? [{ id: 'delete', label: tx('resources:skills.delete'), icon: <IconTrashOutline16 />, danger: true,
        disabled: row.owner === 'user' && scope === 'workspace' ? global?.pending : effective?.pending }] : []),
    ];
    return <ResourceCard key={key} name={name} subtitle={name === row.name ? undefined : row.name}
      icon={<SkillIcon icon={display?.icon} />}
      description={display?.authored.short_description ?? skill?.catalog.description ?? ''}
      diagnostic={row.diagnostics[0]} onOpen={() => setFocus(row)}
      menu={skill && !!items.length && <CardMenu label={tx('resources:card.actions', { p0: name })} items={items}
        onSelect={id => {
          if (id === 'edit') {
            const write = bindDisplayWrite(skill);
            if (write) setEditing({ skill, write });
          } else if (id === 'delete') {
            const remove = bindDelete(skill);
            if (remove) setDeleting({ title: name, owner: row.owner, path: skill.catalog.location.replace(/\/SKILL\.md$/, ''), remove });
          } else if (operationRevision && onTransfer) {
            const taken = new Set([...entries].filter(([other]) => other !== key).map(([, entry]) => entry.name));
            onTransfer(row.owner, row.name, operationRevision, id === 'copy-scope' ? 'copy' : 'move', taken);
          }
        }} />}
      availability={source && revision !== undefined && selectable.has(key) && skill?.disable_model_invocation === false
        && <ResourceAvailability family="skill" name={row.name} valid={row.valid} source={source} scope={scope} revision={revision}
          skillNames={skillNames} skillEnabledByDefault={skill.enabled_by_default} />} />;
  };

  return <>
    {source && revision !== undefined && <SkillSelectionPolicy source={source} scope={scope} revision={revision} skillNames={skillNames} />}
    {catalogError && <p role="alert">{catalogError}</p>}
    <ResourceGroup label={tx('resources:scope.owned')} count={owned.length} view={view}
      empty={<p role="status">{tx(needle ? 'resources:no-results' : !effective ? 'resources:loading' : 'resources:empty')}</p>}>
      {owned.map(renderCard)}
    </ResourceGroup>
    {!!inherited.length && <ResourceGroup label={tx('resources:scope.inherited')} count={inherited.length} view={view} empty={null}>
      {inherited.map(renderCard)}
    </ResourceGroup>}
    {focus && <SkillDetail key={JSON.stringify([focus.owner, focus.name, executionSession])} client={client}
      target={focus.owner === 'user' ? userTarget : target} executionSession={focus.owner === 'workspace' ? executionSession : undefined}
      name={focus.name} display={descriptors.get(rowKey(focus.owner, focus.name))?.display} close={() => setFocus(undefined)} />}
    {canAuthor && editing?.skill.display && <SkillDisplayDialog name={editing.skill.catalog.name} display={editing.skill.display}
      close={() => setEditing(undefined)} save={async (authored, upload) => {
        try {
          await editing.write(authored, upload);
        } finally {
          setLocalRefresh(value => value + 1);
        }
      }} />}
    {canAuthor && deleting && <SkillDeleteDialog name={deleting.title}
      scope={deleting.owner === 'user' ? tx('resources:scope.global') : workspaceLabel ?? tx('resources:scope.workspace')}
      path={deleting.path} close={() => setDeleting(undefined)} remove={async () => {
        try {
          await deleting.remove();
        } finally {
          onRefresh();
        }
      }} />}
  </>;
}
