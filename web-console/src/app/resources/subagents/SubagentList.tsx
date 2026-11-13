import type { ReactNode } from 'react';
import type { SourceScope, SourceSettings, TransferOperation } from '../../../../../protocol/app-server/v78';
import { useTranslation } from '../../../locale/react';
import type { CatalogView } from '../../../presentation/catalog/CatalogControls';
import catalog from '../../../presentation/catalog/Catalog.module.css';
import { CardMenu } from '../../../presentation/catalog/CardMenu';
import { transferActions } from '../shared/ResourceTransfer';
import { IconAgentPresetOutline16, IconEditOutline16 } from '../../../presentation/primitives/icons';
import { ResourceAvailability } from '../shared/ResourceAvailability';
import { extensionEntries } from '../shared/inventory';
import css from './Subagents.module.css';

export function SubagentList({ source, scope, revision, query, view, group, onOpen, onTransfer, transferDisabled, workspaceAvailability, canOverride, onOverride }: {
  canOverride?: (name: string) => boolean; onOverride?: (name: string) => void;
  workspaceAvailability?: (name: string, valid?: boolean) => ReactNode;
  group?: string; transferDisabled?: boolean;
  onTransfer?: (scope: SourceScope, name: string, revision: string, operation: TransferOperation) => void;
  source: SourceSettings; scope: SourceScope; revision?: string; query: string; view: CatalogView; onOpen: (name: string) => void;
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
  return <section className={css.group} role="group" aria-label={title}>
    <h2>{title} <span>{entries.length}</span></h2>
    {!entries.length ? <p className={catalog.state}>{tx('resources:empty')}</p> : view === 'cards' ? <ul className={catalog.cards} role="list">
      {entries.map(entry => <li key={entry.name} aria-label={entry.name} className={catalog.card}>
        <div className={catalog.cardHead}>
          <button type="button" className={catalog.cardOpen} aria-label={entry.name} onClick={() => onOpen(entry.name)}>
            <span className={catalog.fileTile}><IconAgentPresetOutline16 /></span>
            <span className={catalog.cardName}>
              <span className={catalog.cardTitle}>{entry.name}</span>
              <span className={catalog.cardId}>{model(entry)}</span>
            </span>
          </button>
          <div className={catalog.enable}>{availability(entry)}</div>
          {menu(entry)}
        </div>
        {entry.profile?.description && <p className={catalog.cardDescription}>{entry.profile.description}</p>}
      </li>)}
    </ul> : <table className={`${catalog.table} ${css.table}`}>
      <thead><tr>
        <th scope="col">{tx('resources:subagents.name')}</th>
        <th scope="col" className={catalog.optionalColumn}>{tx('settings:extension-detail.description')}</th>
        <th scope="col">{tx('settings:models-page.model')}</th>
        <th scope="col" className={css.enabledColumn}>{tx('workflows:library.column-enabled')}</th>
        <th scope="col" aria-label={tx('resources:card.actions', { p0: '' })} />
      </tr></thead>
      <tbody>{entries.map(entry => <tr key={entry.name}>
        <td>
          <button type="button" className={`${catalog.rowOpen} ${css.rowOpen}`} onClick={() => onOpen(entry.name)}>
            <IconAgentPresetOutline16 /><span>{entry.name}</span>
          </button>
          {entry.profile?.description && <span className={css.narrowDescription}>{entry.profile.description}</span>}
        </td>
        <td className={catalog.optionalColumn}>{entry.profile?.description}</td>
        <td>{model(entry)}</td>
        <td>{availability(entry)}</td><td>{menu(entry)}</td>
      </tr>)}</tbody>
    </table>}
  </section>;
}
