import type { ReactNode } from 'react';
import clsx from 'clsx';
import type { SourceScope, SourceSettings, TransferOperation } from '../../../../../protocol/app-server/v80';
import { useTranslation } from '../../../locale/react';
import type { CatalogView } from '../../../presentation/catalog/CatalogControls';
import catalog from '../../../presentation/catalog/Catalog.module.css';
import { CardMenu } from '../../../presentation/catalog/CardMenu';
import { transferActions } from '../shared/ResourceTransfer';
import { IconEditOutline16 } from '../../../presentation/primitives/icons';
import { ResourceAvailability } from '../shared/ResourceAvailability';
import { extensionEntries } from '../shared/inventory';
import { AgentIcon } from './AgentIcon';
import css from './Subagents.module.css';

export function SubagentList({ source, scope, revision, query, view, group, onOpen, onTransfer, transferDisabled, workspaceAvailability, canOverride, onOverride }: {
  canOverride?: (name: string) => boolean; onOverride?: (name: string) => void;
  workspaceAvailability?: (name: string, valid?: boolean) => ReactNode;
  group?: string; transferDisabled?: boolean;
  onTransfer?: (scope: SourceScope, name: string, revision: string, operation: TransferOperation) => void;
  source: SourceSettings; scope: SourceScope; revision?: string; query: string; view: CatalogView; onOpen: (name: string) => void;
}) {
  const tx = useTranslation();
  const documents = new Map(source.agents.map(agent => [`${agent.scope}:${agent.name}`, agent.source]));
  const needle = query.trim().toLocaleLowerCase();
  const entries = extensionEntries(source, scope, 'agent', 'owned').map(entry => {
    const document = documents.get(`${entry.owner}:${entry.name}`);
    return { ...entry, document, profile: document?.authored };
  }).filter(({ name, profile }) => `${name} ${profile?.description ?? ''} ${profile?.model?.model ?? ''}`.toLocaleLowerCase().includes(needle));
  const model = (entry: typeof entries[number]) => entry.valid === false ? '—' : entry.profile?.model?.model ?? tx('resources:subagents.inherit-model');
  const description = (entry: typeof entries[number]) => entry.valid === false ? tx('resources:subagents.invalid-definition')
    : entry.profile?.description?.trim() || tx('resources:subagents.no-description');
  const availability = (entry: typeof entries[number]) => workspaceAvailability?.(entry.name, entry.valid) ?? (revision && <ResourceAvailability family="agent" source={source}
    scope={scope} name={entry.name} valid={entry.valid} revision={revision} />);

  const menu = (entry: typeof entries[number]) => {
    const authored = entry.document;
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
    {!entries.length ? <p className={catalog.state}>{tx('resources:empty')}</p> : view === 'cards' ? <ul className={clsx(catalog.cards, css.agentCards)} role="list">
      {entries.map(entry => <li key={entry.name} aria-label={entry.name} className={clsx(catalog.card, css.agentCard, entry.valid === false && catalog.invalidCard)}>
        <div className={catalog.cardHead}>
          <button type="button" className={catalog.cardOpen} aria-label={entry.name} onClick={() => onOpen(entry.name)}>
            <span className={catalog.fileTile}><AgentIcon icon={entry.profile?.icon} /></span>
            <span className={catalog.cardName}>
              <span className={catalog.cardTitle}>{entry.name}</span>
              <span className={catalog.cardId}>{model(entry)}</span>
            </span>
          </button>
          <div className={catalog.enable}>{availability(entry)}</div>
          {menu(entry)}
        </div>
        <p className={clsx(catalog.cardDescription, css.agentDescription)} data-invalid={entry.valid === false || undefined}>{description(entry)}</p>
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
            <AgentIcon icon={entry.profile?.icon} /><span>{entry.name}</span>
          </button>
          <span className={css.narrowDescription} data-invalid={entry.valid === false || undefined}>{description(entry)}</span>
        </td>
        <td className={catalog.optionalColumn} data-invalid={entry.valid === false || undefined}>{description(entry)}</td>
        <td>{model(entry)}</td>
        <td>{availability(entry)}</td><td>{menu(entry)}</td>
      </tr>)}</tbody>
    </table>}
  </section>;
}
