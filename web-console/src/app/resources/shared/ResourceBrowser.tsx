import type { SourceScope, SourceSettings } from '../../../../../protocol/app-server/v77';
import type { AppServerClient } from '../../../client/app-server';
import { useTranslation } from '../../../locale/react';
import type { CatalogView } from '../../../presentation/catalog/CatalogControls';
import { IconSkillOutline16 } from '../../../presentation/primitives/icons';
import { SkillDetail } from '../skills/SkillDetail';
import { useSkillCatalog } from '../skills/useSkillCatalog';
import { extensionEntries } from './inventory';
import { ResourceAvailability, SkillSelectionPolicy } from './ResourceAvailability';
import { ResourceCard, ResourceGroup } from './ResourceCards';

export function ResourceBrowser({ client, source, scope, revision, query, view, focus, onFocus }: {
  client: AppServerClient; source: SourceSettings; scope: SourceScope; revision?: string;
  query: string; view: CatalogView; focus?: string; onFocus: (name?: string) => void;
}) {
  const tx = useTranslation();
  const skills = useSkillCatalog(client, source, true);
  const skillsByName = new Map(skills?.skills.map(skill => [skill.catalog.name, skill]));
  // Only a complete, successful discovery can freeze the dynamic all policy.
  // Loading and failed pagination are unavailable evidence, never an empty list.
  const skillNames = skills && skills.error === undefined
    ? skills.skills.filter(skill => !skill.disable_model_invocation).map(skill => skill.catalog.name) : undefined;
  const entries = extensionEntries(source, scope, 'skill').map(entry => ({
    ...entry,
    description: skillsByName.get(entry.name)?.catalog.description ?? '',
  }));
  // Skills discovery remains available even when rustx.toml cannot be resolved.
  for (const skill of skills?.skills ?? []) {
    if (entries.some(row => row.name === skill.catalog.name)) continue;
    entries.push({
      family: 'skill', name: skill.catalog.name, owner: skill.provenance.source, path: skill.catalog.location,
      relationship: skill.provenance.source === scope ? 'authored' : 'inherited', valid: true, diagnostics: [],
      capability: {family:'skill', inventory:'native', authoring:'none', selection:'skills', preparation:false, diagnostics:true},
      description: skill.catalog.description,
    });
  }
  const needle = query.trim().toLocaleLowerCase();
  const matches = entries.filter(row => `${row.name} ${row.description} ${row.path}`.toLocaleLowerCase().includes(needle));
  const owned = matches.filter(row => row.owner === scope);
  const inherited = matches.filter(row => row.owner !== scope);

  const renderCard = (row: typeof entries[number]) => <ResourceCard key={row.name} name={row.name}
    icon={<IconSkillOutline16 />}
    description={row.description} diagnostic={row.diagnostics[0]} onOpen={() => onFocus(row.name)}
    availability={revision !== undefined && skillsByName.get(row.name)?.disable_model_invocation === false && <ResourceAvailability family="skill" name={row.name} valid={row.valid}
      source={source} scope={scope} revision={revision} skillNames={skillNames} />} />;

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
    {focus && <SkillDetail key={focus} client={client} source={source} name={focus} close={() => onFocus(undefined)} />}
  </>;
}
