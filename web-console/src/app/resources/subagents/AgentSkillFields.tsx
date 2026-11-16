import { useId } from 'react';
import type { SourceScope, SourceSettings, SourceToolSelection } from '../../../../../protocol/app-server/v78';
import type { AppServerClient } from '../../../client/app-server';
import { useTranslation } from '../../../locale/react';
import { Switch } from '../../../presentation/primitives/Switch';
import { useSkillCatalog } from '../skills/useSkillCatalog';
import { ResourceChoices, ScopedChoices, resourceScopes } from './ScopedChoices';
import css from './Subagents.module.css';

export function AgentSkillFields({ client, source, scope, active, value, change }: {
  client?: AppServerClient; source: SourceSettings; scope: SourceScope; active: boolean;
  value: SourceToolSelection; change: (next: SourceToolSelection) => void;
}) {
  const tx = useTranslation();
  const heading = useId();
  const catalog = useSkillCatalog(client, source, active, scope);
  const scopes = resourceScopes(scope);
  const skills = (catalog?.skills ?? []).filter(skill => !skill.disable_model_invocation && scopes.includes(skill.provenance.source));
  const selected = value === 'all' ? skills.map(skill => skill.catalog.name) : value;
  const names = new Set(skills.map(skill => skill.catalog.name));
  const unavailable = catalog && !catalog.error ? selected.filter(name => !names.has(name)) : [];
  return <div className={css.choiceGroup} role="group" aria-labelledby={heading}>
    <div className={css.sectionHead}>
      <h3 id={heading}>{tx('settings:extension-detail.skill-prompt-visibility')}</h3>
      <div className={css.toolSwitch}>
        <span>{tx('resources:subagents.all-skills')}</span>
        <Switch label={tx('resources:subagents.all-skills')} checked={value === 'all'}
          onChange={checked => change(checked ? 'all' : [])} />
      </div>
    </div>
    {catalog?.error ? <p role="status">{tx('resources:subagents.skills-failed')}</p>
      : !catalog && client && active ? <p role="status">{tx('resources:subagents.discovering-skills')}</p>
      : scopes.map(owner => {
        const options = skills.filter(skill => skill.provenance.source === owner).map(skill => skill.catalog);
        return <ScopedChoices key={owner} owner={owner}>
          {options.length ? <ResourceChoices label={tx(owner === 'workspace' ? 'resources:scope.workspace' : 'resources:scope.global')}
            options={options} selected={selected} change={change} /> : <p>{tx('resources:subagents.no-skills')}</p>}
        </ScopedChoices>;
      })}
    {!!unavailable.length && <div className={css.scopedChoices}>
      <span className={css.scopeHeading}>{tx('resources:subagents.unavailable-skills')}</span>
      <ResourceChoices label={tx('resources:subagents.unavailable-skills')} options={unavailable.map(name => ({ name }))}
        selected={selected} change={change} />
    </div>}
  </div>;
}
