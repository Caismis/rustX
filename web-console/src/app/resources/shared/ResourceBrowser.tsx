import { useState } from 'react';
import type { SourceScope, SourceSettings } from '../../../../../protocol/app-server/v77';
import type { AppServerClient } from '../../../client/app-server';
import { useTranslation } from '../../../locale/react';
import type { CatalogView } from '../../../presentation/catalog/CatalogControls';
import { Button } from '../../../presentation/primitives/Button';
import { DialogSurface } from '../../../presentation/primitives/DialogSurface';
import modal from '../../../presentation/primitives/Modal.module.css';
import { IconAgentPresetOutline16, IconCodeOutline16 } from '../../../presentation/primitives/icons';
import type { ModelProfiles } from '../../settings/models/ModelsPage';
import { SubagentDetail } from '../subagents/SubagentDetail';
import { SkillDetail } from '../skills/SkillDetail';
import { useSkillCatalog } from '../skills/useSkillCatalog';
import { extensionEntries } from './inventory';
import { ResourceAvailability } from './ResourceAvailability';
import { ResourceCard, ResourceGroup } from './ResourceCards';
import css from '../ResourceSurface.module.css';

export function ResourceBrowser({ family, client, source, scope, revision, models, profiles, query, view, focus, onFocus, creating, onCloseCreate }: {
  family: 'skill' | 'agent'; client: AppServerClient; source: SourceSettings; scope: SourceScope; revision?: string;
  models: string[]; profiles: ModelProfiles; query: string; view: CatalogView; focus?: string; onFocus: (name?: string) => void;
  creating: boolean; onCloseCreate: () => void;
}) {
  const tx = useTranslation();
  const skills = useSkillCatalog(client, source, family === 'skill');
  const entries = extensionEntries(source, scope, family).map(entry => ({
    ...entry,
    description: family === 'skill'
      ? skills?.skills.find(skill => skill.catalog.name === entry.name)?.catalog.description ?? ''
      : source.agents.find(agent => agent.name === entry.name && agent.scope === entry.owner)?.source.authored?.description ?? '',
  }));
  // Skills discovery remains available even when rustx.toml cannot be resolved.
  if (family === 'skill') for (const skill of skills?.skills ?? []) {
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
  if (focus && family === 'agent') return <>
    <div className={css.detailHeader}><Button size="sm" onClick={() => onFocus(undefined)}>{tx('resources:back')}</Button><h2>{focus}</h2></div>
    <SubagentDetail source={source} scope={scope} revision={revision} name={focus} models={models} profiles={profiles} />
  </>;

  const renderCard = (row: typeof entries[number]) => <ResourceCard key={row.name} name={row.name}
    icon={family === 'agent' ? <IconAgentPresetOutline16 /> : <IconCodeOutline16 />}
    description={row.description} diagnostic={row.diagnostics[0]} path={row.path} onOpen={() => onFocus(row.name)}
    availability={revision !== undefined && <ResourceAvailability family={family} name={row.name} valid={row.valid}
      source={source} scope={scope} revision={revision} />} />;

  return <>
    {skills?.error && <p role="alert">{skills.error}</p>}
    <ResourceGroup label={tx('resources:scope.owned')} count={owned.length} view={view}
      empty={<p>{tx(needle ? 'resources:no-results' : 'resources:empty')}</p>}>
      {owned.map(renderCard)}
    </ResourceGroup>
    {!!inherited.length && <ResourceGroup label={tx('resources:scope.inherited')} count={inherited.length} view={view} empty={null}>
      {inherited.map(renderCard)}
    </ResourceGroup>}
    {family === 'skill' && focus && <SkillDetail key={focus} client={client} source={source} name={focus} close={() => onFocus(undefined)} />}
    {creating && <NewSubagent names={entries.map(row => row.name)} onClose={onCloseCreate} onContinue={onFocus} />}
  </>;
}

function NewSubagent({ names, onClose, onContinue }: { names: string[]; onClose: () => void; onContinue: (name: string) => void }) {
  const tx = useTranslation();
  const [name, setName] = useState('');
  const identity = name.trim();
  const valid = /^[a-z][a-z0-9_-]{0,63}$/.test(identity);
  const duplicate = names.includes(identity);
  return <DialogSurface open onClose={onClose} title={tx('resources:subagents.new')} overlayClassName={`${modal.root} ${modal.scrim}`} className={modal.dialog}>
    <div className={modal.header}><h2>{tx('resources:subagents.new')}</h2><Button aria-label={tx('resources:close')} onClick={onClose}>×</Button></div>
    <div className={modal.body}>
      <label className={css.create}>{tx('resources:subagents.name')}
        <input autoFocus maxLength={64} value={name} aria-invalid={!!name && !valid || undefined} onChange={event => setName(event.target.value)} />
      </label>
      <p className={css.muted}>{tx('resources:subagents.name-help')}</p>
      {duplicate && <p role="alert">{tx('resources:subagents.duplicate')}</p>}
    </div>
    <div className={modal.footer}>
      <Button variant="outline" onClick={onClose}>{tx('resources:cancel')}</Button>
      <Button variant="primary" disabled={!valid || duplicate} onClick={() => { onContinue(identity); onClose(); }}>{tx('resources:subagents.continue')}</Button>
    </div>
  </DialogSurface>;
}
