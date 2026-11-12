import type { SourceScope, SourceSettings } from '../../../../../protocol/app-server/v79';
import { useTranslation } from '../../../locale/react';
import type { CatalogView } from '../../../presentation/catalog/CatalogControls';
import catalog from '../../../presentation/catalog/Catalog.module.css';
import { IconAgentPresetOutline16 } from '../../../presentation/primitives/icons';
import { ResourceAvailability } from '../shared/ResourceAvailability';
import { extensionEntries } from '../shared/inventory';
import css from './Subagents.module.css';

export function SubagentList({ source, scope, revision, query, view, onOpen }: {
  source: SourceSettings; scope: SourceScope; revision?: string; query: string; view: CatalogView; onOpen: (name: string) => void;
}) {
  const tx = useTranslation();
  const profiles = new Map(source.agents.map(agent => [`${agent.scope}:${agent.name}`, agent.source.authored]));
  const needle = query.trim().toLocaleLowerCase();
  const entries = extensionEntries(source, scope, 'agent').map(entry => ({
    ...entry,
    profile: profiles.get(`${entry.owner}:${entry.name}`),
  })).filter(({ name, profile }) => `${name} ${profile?.description ?? ''} ${profile?.model?.model ?? ''}`.toLocaleLowerCase().includes(needle));
  const model = (entry: typeof entries[number]) => entry.profile?.model?.model ?? tx('resources:subagents.inherit-model');
  const availability = (entry: typeof entries[number]) => revision && <ResourceAvailability family="agent" source={source}
    scope={scope} name={entry.name} valid={entry.valid} revision={revision} />;

  if (!entries.length) return <p className={catalog.state}>{tx(needle ? 'resources:no-results' : 'resources:empty')}</p>;
  return <>
    {(['owned', 'inherited'] as const).map(group => {
      const rows = entries.filter(entry => (entry.owner === scope) === (group === 'owned'));
      if (!rows.length) return null;
      const title = tx(group === 'owned' ? 'resources:subagents.own-group' : 'resources:scope.inherited');
      return <section className={css.group} key={group} aria-label={title}>
        <h2>{title} <span>{rows.length}</span></h2>
        {view === 'cards' ? <ul className={catalog.cards} role="list">
          {rows.map(entry => <li key={entry.name} aria-label={entry.name} className={catalog.card}>
            <div className={catalog.cardHead}>
              <button type="button" className={catalog.cardOpen} aria-label={entry.name} onClick={() => onOpen(entry.name)}>
                <span className={catalog.fileTile}><IconAgentPresetOutline16 /></span>
                <span className={catalog.cardName}>
                  <span className={catalog.cardTitle}>{entry.name}</span>
                  <span className={catalog.cardId}>{model(entry)}</span>
                </span>
              </button>
              {revision && <div className={catalog.enable}>{availability(entry)}</div>}
            </div>
            {entry.profile?.description && <p className={catalog.cardDescription}>{entry.profile.description}</p>}
          </li>)}
        </ul> : <table className={`${catalog.table} ${css.table}`}>
          <thead><tr>
            <th scope="col">{tx('resources:subagents.name')}</th>
            <th scope="col" className={catalog.optionalColumn}>{tx('settings:extension-detail.description')}</th>
            <th scope="col">{tx('settings:models-page.model')}</th>
            <th scope="col" className={css.enabledColumn}>{tx('workflows:library.column-enabled')}</th>
          </tr></thead>
          <tbody>{rows.map(entry => <tr key={entry.name}>
            <td>
              <button type="button" className={`${catalog.rowOpen} ${css.rowOpen}`} onClick={() => onOpen(entry.name)}>
                <IconAgentPresetOutline16 /><span>{entry.name}</span>
              </button>
              {entry.profile?.description && <span className={css.narrowDescription}>{entry.profile.description}</span>}
            </td>
            <td className={catalog.optionalColumn}>{entry.profile?.description}</td>
            <td>{model(entry)}</td>
            <td>{availability(entry)}</td>
          </tr>)}</tbody>
        </table>}
      </section>;
    })}
  </>;
}
