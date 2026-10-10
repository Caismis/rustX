import { ResourceAvailability } from './ResourceAvailability';
import type { ReactNode } from 'react';
import { useState } from 'react';
import { useTranslation } from '../../../locale/react';
import type { ResourceFamily, SourceScope, SourceSettings } from '../../../../../protocol/app-server/v44';
import { Button } from '../../../presentation/primitives/Button';
import { IconAgentPresetOutline16, IconCodeOutline16 } from '../../../presentation/primitives/icons';
import { NativeFacts } from '../../components/NativeFacts';
import { Advanced, FilterTabs, Search } from '../primitives/aria';
import { extensionFamilyLabel, type ExtensionFamily } from '../projection';
import type { PageFocus } from '../machines/navigation';
import { TextField } from '../forms/controls';
import { allExtensionEntries, collectionDiagnostics, extensionEntries, resourceFamilies, type ExtensionEntry } from './inventory';
import { ExtensionDetail } from './ExtensionDetail';
import { NativeExtensions } from './NativeExtensions';
import css from '../../../presentation/settings/SettingsContent.module.css';
import catalog from './ResourceCatalog.module.css';

export type ExtensionFilter = 'all' | Exclude<ExtensionFamily, 'mcp'>;

/** Resource management uses native ownership for groups and existing exact-scope
 * editors for writes. Browsing a catalog does not prepare or connect resources. */
export function ExtensionsPage({ source, scope, revision, models, focus, onFocus, filter, onFilter, scopeControl, refresh, refreshing }: {
  source: SourceSettings; scope: SourceScope; revision?: string; models: string[];
  focus?: PageFocus['extensions']; onFocus: (focus?: PageFocus['extensions']) => void;
  filter: ExtensionFilter; onFilter: (filter: ExtensionFilter) => void;
  scopeControl: ReactNode; refresh: () => void; refreshing: boolean;
}) {
  const tx = useTranslation();
  const filters: readonly (readonly [ExtensionFilter, string])[] = [
    ['all', tx('settings:copy.all')], ['skill', tx('settings:catalog.skill')], ['agent', tx('settings:catalog.agent')],
    ['workflow', tx('settings:copy.workflows')], ['managed_python', tx('settings:copy.python')], ['native', tx('settings:copy.native')],
  ];
  const [query, setQuery] = useState('');
  const [creating, setCreating] = useState(false);
  const [name, setName] = useState('');
  const title = filter === 'all' ? tx('settings:extensions-page.extensions') : filters.find(([id]) => id === filter)![1];
  if (focus) return <ExtensionDetail source={source} scope={scope} revision={revision} models={models} family={focus.family} name={focus.name} onFocus={onFocus}/>;
  const entries = filter === 'all' ? allExtensionEntries(source, scope).filter(entry => entry.family !== 'mcp') : filter === 'native' ? [] : extensionEntries(source, scope, filter);
  const canCreate = filter === 'agent';
  const describe = (entry: ExtensionEntry) => {
    if (entry.family === 'agent') return source.agents.find(agent => agent.name === entry.name && agent.scope === entry.owner)?.source.authored?.description || entry.path;
    return entry.path;
  };
  const needle = query.trim().toLocaleLowerCase();
  const matches = entries.filter(entry => [entry.name, describe(entry), entry.path].some(text => text.toLocaleLowerCase().includes(needle)));
  const owned = matches.filter(entry => entry.owner === scope);
  const inherited = matches.filter(entry => entry.owner !== scope);
  const open = (entry: ExtensionEntry) => onFocus({kind:'extension',family:entry.family,name:entry.name});
  const actions = <div className={catalog.actions}>
    <Button size="sm" variant="outline" disabled={refreshing} onClick={refresh}>{tx('settings:catalog.refresh')}</Button>
    {canCreate && <Button size="sm" variant="primary" onClick={() => setCreating(value => !value)}>{tx('settings:catalog.new')}</Button>}
  </div>;
  const cards = (items: readonly ExtensionEntry[]) => <div className={catalog.list} role="list" aria-label={tx('settings:extensions-page.value-extensions',{p0:title})}>
    {items.map(entry => <div key={`${entry.family}:${entry.name}`} role="listitem" aria-label={entry.name} className={catalog.card}><button type="button" className={catalog.open} aria-label={`${extensionFamilyLabel(tx,entry.family)} ${entry.name}`} onClick={() => open(entry)}>
      <span className={catalog.icon} aria-hidden="true">{entry.family === 'agent' ? <IconAgentPresetOutline16/> : <IconCodeOutline16/>}</span>
      <span className={catalog.content}><span className={catalog.name}><span className={catalog.identity}>{entry.name}</span>{filter === 'all' && <span className={catalog.count}>{extensionFamilyLabel(tx,entry.family)}</span>}</span>
        <span className={catalog.description} title={entry.diagnostics[0] ?? describe(entry)}>{entry.diagnostics[0] ?? describe(entry)}</span></span>
      <span className={catalog.state} data-invalid={entry.valid === false || undefined}>{entry.valid === false ? tx('settings:copy.invalid-definition') : entry.selected === undefined ? tx('settings:catalog.unknown') : entry.selected ? tx('settings:catalog.enabled') : tx('settings:catalog.disabled')}</span>
    </button>{revision !== undefined && (entry.family === 'skill' || entry.family === 'agent') && <ResourceAvailability family={entry.family} name={entry.name} valid={entry.valid} source={source} scope={scope} revision={revision} inspect={() => open(entry)}/>}</div>)}
  </div>;
  return <section className={catalog.catalog} aria-label={tx('settings:extensions-page.extensions')}>
    <h3 className={catalog.title}>{title}</h3>
    <div className={catalog.header}>{scopeControl}<span className={catalog.count}>{title} {entries.length}</span>
      <div className={catalog.search}><Search label={tx('settings:extensions-page.find-an-extension')} value={query} onChange={setQuery} placeholder={tx('settings:catalog.search')}/></div>
    </div>
    <FilterTabs label={tx('settings:extensions-page.extension-kinds')} value={filter} onChange={value => {onFilter(value);setCreating(false);setName('');setQuery('');}} options={filters}>
      {filter === 'native' ? <NativeExtensions source={source} scope={scope} revision={revision}/> : <>
        <div className={catalog.group} role="group" aria-label={tx('settings:catalog.owned')}>
          <div className={catalog.groupHeader}><h4 className={catalog.groupTitle}>{tx('settings:catalog.owned')}</h4><span className={catalog.count}>{owned.length}</span>{actions}</div>
          {creating && canCreate && <form className={catalog.create} aria-label={tx('settings:catalog.create-title')} onSubmit={event => {event.preventDefault();if (name.trim() && !entries.some(entry => entry.name === name.trim())) onFocus({kind:'extension',family:filter as ExtensionFamily,name:name.trim()});}}>
            <TextField label={tx('settings:extensions-page.new-value-identity',{p0:extensionFamilyLabel(tx,filter as ExtensionFamily)})} value={name} change={setName}/>
            <div className={css.actions}><Button type="submit" variant="primary" disabled={!name.trim() || entries.some(entry => entry.name === name.trim())}>{tx('settings:extensions-page.add')} {extensionFamilyLabel(tx,filter as ExtensionFamily)}</Button><Button onClick={() => setCreating(false)}>{tx('settings:catalog.cancel')}</Button></div>
          </form>}
          {owned.length ? cards(owned) : <div className={catalog.empty}><span>{tx(query ? 'settings:catalog.no-results' : 'settings:catalog.empty')}</span><p>{tx(query ? 'settings:catalog.no-results-description' : filter === 'skill' ? 'settings:catalog.skill-help' : 'settings:catalog.empty-description')}</p>{canCreate && !query && <Button variant="primary" onClick={() => setCreating(true)}>{tx('settings:catalog.new')}</Button>}</div>}
        </div>
        {!!inherited.length && <div className={catalog.group} role="group" aria-label={tx(scope === 'workspace' ? 'settings:catalog.inherited' : 'settings:catalog.shadowed')}><div className={catalog.groupHeader}><h4 className={catalog.groupTitle}>{tx(scope === 'workspace' ? 'settings:catalog.inherited' : 'settings:catalog.shadowed')}</h4><span className={catalog.count}>{inherited.length}</span></div>{cards(inherited)}</div>}

        {filter === 'skill' && !!entries.length && <p className={css.hint}>{tx('settings:catalog.skill-help')}</p>}
        <CollectionDiagnostics source={source} families={filter === 'all' ? resourceFamilies.filter(family => family !== 'mcp') : [filter as ResourceFamily]}/>
      </>}
    </FilterTabs>
  </section>;
}

/** Diagnostics native attributes to a family's source document or collection
 * rather than to any one identity — a resource document that does not parse,
 * an unreadable directory, an exceeded catalog bound. They belong to the
 * family, so they are listed once here and never on the rows of the resources
 * those documents hold. Skill discovery reports its own typed diagnostics,
 * which name packages rather than Skill identities, so they are listed here on
 * the same terms. */
export function CollectionDiagnostics({ source, families }: { source: SourceSettings; families: readonly ResourceFamily[] }) {
  const tx = useTranslation();
  const diagnostics = collectionDiagnostics(source, families);
  const skills = families.includes('skill') ? source.prospective_resources?.skill_diagnostics ?? [] : [];
  if (!diagnostics.length && !skills.length) return null;
  return <section aria-label={tx('settings:extensions-page.source-diagnostics')}>
    <h4>{tx('settings:extensions-page.source-diagnostics')}</h4>
    {diagnostics.map((item, index) => <p className={css.error} key={index}>
      {extensionFamilyLabel(tx, item.subject.family)} {tx('settings:extensions-page.source')}{item.file ? tx('settings:extensions-page.value', { p0: item.file }) : ''}: {item.reason}
    </p>)}
    {!!skills.length && <Advanced title={tx('settings:extensions-page.skill-discovery-diagnostics-value', { p0: skills.length })}><NativeFacts value={skills} /></Advanced>}
  </section>;
}
