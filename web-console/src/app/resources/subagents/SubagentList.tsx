import type { ReactNode } from 'react';
import type { SourceScope, SourceSettings, TransferOperation } from '../../../../../protocol/app-server/v80';
import { useTranslation } from '../../../locale/react';
import catalog from '../../../presentation/catalog/Catalog.module.css';
import { CardMenu } from '../../../presentation/catalog/CardMenu';
import type { CatalogView } from '../../../presentation/catalog/CatalogControls';
import { transferActions } from '../shared/ResourceTransfer';
import { IconAgentPresetOutline16, IconEditOutline16 } from '../../../presentation/primitives/icons';
import { ResourceAvailability } from '../shared/ResourceAvailability';
import { ResourceCard, ResourceGroup } from '../shared/ResourceCards';
import { extensionEntries } from '../shared/inventory';

export function SubagentList({ source, scope, revision, query, group, view, onOpen, onTransfer, transferDisabled, workspaceAvailability, canOverride, onOverride }: {
  canOverride?: (name: string) => boolean; onOverride?: (name: string) => void;
  workspaceAvailability?: (name: string, valid?: boolean) => ReactNode;
  group?: string; view: CatalogView; transferDisabled?: boolean;
  onTransfer?: (scope: SourceScope, name: string, revision: string, operation: TransferOperation) => void;
  source: SourceSettings; scope: SourceScope; revision?: string; query: string; onOpen: (name: string) => void;
}) {
  const tx = useTranslation();
  const profiles = new Map(source.agents.map(agent => [`${agent.scope}:${agent.name}`, agent.source.authored]));
  const needle = query.trim().toLocaleLowerCase();
  const entries = extensionEntries(source, scope, 'agent', 'owned').map(entry => ({
    ...entry,
    profile: profiles.get(`${entry.owner}:${entry.name}`),
  })).filter(({ name, profile }) => `${name} ${profile?.description ?? ''} ${profile?.model?.model ?? ''}`.toLocaleLowerCase().includes(needle));
  const model = (entry: typeof entries[number]) => entry.profile?.model?.model ?? tx('resources:subagents.inherit-model');
  const availability = (entry: typeof entries[number]) => workspaceAvailability?.(entry.name, entry.valid) ?? (revision && <ResourceAvailability family="agent" source={source}
    scope={scope} name={entry.name} valid={entry.valid} revision={revision} />);

  const menu = (entry: typeof entries[number]) => {
    const authored = source.agents.find(agent => agent.scope === scope && agent.name === entry.name)?.source;
    return <CardMenu label={tx('resources:card.actions', { p0: entry.name })}
      items={[{ id: 'edit', label: tx('resources:card.edit'), icon: <IconEditOutline16 /> },
        ...(canOverride?.(entry.name) ? [{ id: 'override', label: tx('resources:card.override'), icon: <IconEditOutline16 /> }] : []),
        ...(onTransfer ? transferActions(tx, scope, !!transferDisabled || !authored?.authored) : [])]}
      onSelect={action => {
        if (action === 'edit') onOpen(entry.name);
        else if (action === 'override') onOverride?.(entry.name);
        else if (authored) onTransfer?.(scope, entry.name, authored.revision, action === 'copy-scope' ? 'copy' : 'move');
      }} />;
  };
  if (!entries.length && (needle || !group)) return group ? null : <p className={catalog.state}>{tx(needle ? 'resources:no-results' : 'resources:empty')}</p>;
  const title = group ?? tx('resources:subagents.own-group');
  return <ResourceGroup label={title} count={entries.length} view={view}
    empty={<p role="status">{tx('resources:empty')}</p>}>
    {entries.map(entry => <ResourceCard key={entry.name} name={entry.name} subtitle={model(entry)}
      icon={<IconAgentPresetOutline16 />} description={entry.profile?.description ?? ''} diagnostic={entry.diagnostics[0]}
      onOpen={() => onOpen(entry.name)} availability={availability(entry)} menu={menu(entry)} />)}
  </ResourceGroup>;
}
