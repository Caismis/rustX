import type { SourceScope, SourceSettings } from '../../../../../protocol/app-server/v77';
import { useTranslation } from '../../../locale/react';
import { IconAgentPresetOutline16, IconChevronRightOutline14 } from '../../../presentation/primitives/icons';
import { ResourceAvailability } from '../shared/ResourceAvailability';
import { extensionEntries } from '../shared/inventory';
import css from './Subagents.module.css';

export function SubagentList({ source, scope, revision, query, onOpen }: {
  source: SourceSettings; scope: SourceScope; revision?: string; query: string; onOpen: (name: string) => void;
}) {
  const tx = useTranslation();
  const profiles = new Map(source.agents.map(agent => [`${agent.scope}:${agent.name}`, agent.source.authored]));
  const needle = query.trim().toLocaleLowerCase();
  const entries = extensionEntries(source, scope, 'agent').map(entry => ({
    ...entry,
    profile: profiles.get(`${entry.owner}:${entry.name}`),
  })).filter(({ name, profile }) => `${name} ${profile?.description ?? ''} ${profile?.model?.model ?? ''}`.toLocaleLowerCase().includes(needle));

  if (!entries.length) return <p className={css.empty}>{tx(needle ? 'resources:no-results' : 'resources:empty')}</p>;
  return <>
    {(['owned', 'inherited'] as const).map(group => {
      const rows = entries.filter(entry => (entry.owner === scope) === (group === 'owned'));
      if (!rows.length) return null;
      const title = tx(group === 'owned' ? 'resources:subagents.own-group' : 'resources:scope.inherited');
      return <section className={css.group} key={group} aria-label={title}>
        <h2>{title} <span>{rows.length}</span></h2>
        <ul className={css.list} role="list">
          {rows.map(entry => <li key={entry.name} aria-label={entry.name} className={css.row}>
            <button type="button" className={css.rowOpen} aria-label={entry.name} onClick={() => onOpen(entry.name)}>
              <span className={css.icon}><IconAgentPresetOutline16 /></span>
              <span className={css.rowBody}>
                <span className={css.rowName}>{entry.name}</span>
                {entry.profile?.description && <span className={css.rowDescription}>{entry.profile.description}</span>}
              </span>
              <span className={css.badge}>{entry.profile?.model?.model ?? tx('resources:subagents.inherit-model')}</span>
              <IconChevronRightOutline14 className={css.rowChevron} aria-hidden="true" />
            </button>
            {revision && <ResourceAvailability family="agent" source={source} scope={scope} name={entry.name} valid={entry.valid} revision={revision} />}
          </li>)}
        </ul>
      </section>;
    })}
  </>;
}
