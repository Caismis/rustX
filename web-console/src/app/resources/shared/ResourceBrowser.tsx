import { useState } from 'react';
import type { SkillDescriptor, SourceScope, SourceSettings } from '../../../../../protocol/app-server/v77';
import type { AppServerClient } from '../../../client/app-server';
import { useTranslation } from '../../../locale/react';
import type { CatalogView } from '../../../presentation/catalog/CatalogControls';
import { CardMenu } from '../../../presentation/catalog/CardMenu';
import { IconEditOutline16 } from '../../../presentation/primitives/icons';
import { SkillIcon } from '../skills/SkillIcon';
import { SkillDisplayDialog, type SkillDisplayWriter } from '../skills/SkillDisplayDialog';
import { SkillDetail } from '../skills/SkillDetail';
import { useSkillCatalog } from '../skills/useSkillCatalog';
import { extensionEntries } from './inventory';
import { ResourceAvailability, SkillSelectionPolicy } from './ResourceAvailability';
import { ResourceCard, ResourceGroup } from './ResourceCards';

export function ResourceBrowser({ client, source, scope, revision, query, view, focus, onFocus, bindDisplayWrite }: {
  client: AppServerClient; source: SourceSettings; scope: SourceScope; revision?: string;
  bindDisplayWrite: (skill: SkillDescriptor) => SkillDisplayWriter | undefined;
  query: string; view: CatalogView; focus?: string; onFocus: (name?: string) => void;
}) {
  const tx = useTranslation();
  const [refresh, setRefresh] = useState(0);
  const [editing, setEditing] = useState<{ skill: SkillDescriptor; write: SkillDisplayWriter }>();
  const skills = useSkillCatalog(client, source, true, scope, refresh);
  const skillsByName = new Map(skills?.skills.map(skill => [skill.catalog.name, skill]));
  // Only a complete, successful discovery can freeze the dynamic all policy.
  // Loading and failed pagination are unavailable evidence, never an empty list.
  const skillNames = skills && skills.error === undefined
    ? skills.skills.filter(skill => !skill.disable_model_invocation).map(skill => skill.catalog.name) : undefined;
  const entries = new Map(extensionEntries(source, scope, 'skill').map(entry => [entry.name, entry]));
  // Skills discovery remains available even when rustx.toml cannot be resolved.
  for (const skill of skills?.skills ?? []) {
    if (entries.has(skill.catalog.name)) continue;
    entries.set(skill.catalog.name, {
      family: 'skill', name: skill.catalog.name, owner: skill.provenance.source, path: skill.catalog.location,
      relationship: skill.provenance.source === scope ? 'authored' : 'inherited', valid: true, diagnostics: [],
      capability: {family:'skill', inventory:'native', authoring:'none', selection:'skills', preparation:false, diagnostics:true},
    });
  }
  const needle = query.trim().toLocaleLowerCase();
  const matches = [...entries.values()].filter(row => {
    const skill = skillsByName.get(row.name);
    return [row.name, row.path, skill?.catalog.description, skill?.display?.authored.display_name, skill?.display?.authored.short_description]
      .some(value => value?.toLocaleLowerCase().includes(needle));
  });
  const owned = matches.filter(row => row.owner === scope);
  const inherited = matches.filter(row => row.owner !== scope);

  const renderCard = (row: typeof matches[number]) => {
    const skill = skillsByName.get(row.name);
    const display = skill?.display;
    const name = display?.authored.display_name ?? row.name;
    return <ResourceCard key={row.name} name={name} subtitle={name === row.name ? undefined : row.name}
      icon={<SkillIcon icon={display?.icon} />}
      description={display?.authored.short_description ?? skill?.catalog.description ?? ''}
      diagnostic={row.diagnostics[0]} onOpen={() => onFocus(row.name)}
      menu={skill && display?.revision && <CardMenu label={tx('resources:card.actions', { p0: name })}
        items={[{ id: 'edit', label: tx('resources:skills.edit-display'), icon: <IconEditOutline16 /> }]}
        onSelect={() => {
          const write = bindDisplayWrite(skill);
          if (write) setEditing({ skill, write });
        }} />}
      availability={revision !== undefined && skill?.disable_model_invocation === false && <ResourceAvailability family="skill" name={row.name} valid={row.valid}
        source={source} scope={scope} revision={revision} skillNames={skillNames} />} />;
  };

  return <>
    {revision !== undefined && <SkillSelectionPolicy source={source} scope={scope} revision={revision} skillNames={skillNames} />}
    {skills?.error && <p role="alert">{skills.error}</p>}
    <ResourceGroup label={tx('resources:scope.owned')} count={owned.length} view={view}
      empty={<p role="status">{tx(needle ? 'resources:no-results' : !skills ? 'resources:loading' : 'resources:empty')}</p>}>
      {owned.map(renderCard)}
    </ResourceGroup>
    {!!inherited.length && <ResourceGroup label={tx('resources:scope.inherited')} count={inherited.length} view={view} empty={null}>
      {inherited.map(renderCard)}
    </ResourceGroup>}
    {focus && <SkillDetail key={focus} client={client} source={source} name={focus} display={skillsByName.get(focus)?.display} close={() => onFocus(undefined)} />}
    {editing?.skill.display && <SkillDisplayDialog name={editing.skill.catalog.name} display={editing.skill.display}
      close={() => setEditing(undefined)} save={async authored => {
        try {
          await editing.write(authored);
        } finally {
          setRefresh(value => value + 1);
        }
      }} />}
  </>;
}
