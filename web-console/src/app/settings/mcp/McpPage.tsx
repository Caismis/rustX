import { useState, type ReactNode } from 'react';
import type { SourceScope, SourceSettings } from '../../../../../protocol/app-server/v37';
import { useTranslation } from '../../../locale/react';
import { Button } from '../../../presentation/primitives/Button';
import { Menu } from '../../../presentation/primitives/Menu';
import { IconRefreshOutline16 } from '../../../presentation/primitives/icons';
import { Search } from '../primitives/aria';
import { documentAuthoring } from '../projection';
import type { PageFocus } from '../machines/navigation';
import { extensionEntries, preparationLabel } from '../extensions/inventory';
import { CollectionDiagnostics } from '../extensions/ExtensionsPage';
import { ResourceAvailability } from '../extensions/ResourceAvailability';
import { McpIcon } from './McpIcon';
import { McpEditor } from './McpEditor';
import css from './McpPage.module.css';

/** ZCode's dedicated catalog layout, backed by rustX's exact-scope authority. */
export function McpPage({ source, scope, revision, focus, onFocus, scopeControl, refresh, refreshing }: {
  source: SourceSettings; scope: SourceScope; revision?: string; focus?: PageFocus['mcp'];
  onFocus: (focus?: PageFocus['mcp']) => void; scopeControl: ReactNode; refresh: () => void; refreshing: boolean;
}) {
  const tx = useTranslation();
  const [query, setQuery] = useState('');
  const [menu, setMenu] = useState(false);
  const document = documentAuthoring(scope === 'user' ? source.user_mcp : source.workspace_mcp);
  const entries = extensionEntries(source, scope, 'mcp');
  const description = (name: string, owner: SourceScope, path: string) => {
    const config = (owner === 'user' ? source.user_mcp : source.workspace_mcp)?.authored?.[name]?.definition;
    return config ? config.url ? `http · ${config.url}` : `stdio · ${[config.command, ...(config.args ?? [])].join(' ')}` : path;
  };
  if (focus) return <div className={css.page} data-mcp-page=""><McpEditor key={`${scope}:${focus.name ?? ''}:${focus.mode ?? ''}`} source={source} scope={scope} revision={revision} focus={focus} scopeControl={scopeControl} close={() => onFocus(undefined)}/></div>;
  const filtered = entries.filter(entry => `${entry.name} ${description(entry.name, entry.owner, entry.path)}`.toLocaleLowerCase().includes(query.trim().toLocaleLowerCase()));
  const create = () => onFocus({kind:'mcp'});
  const importConfig = () => onFocus({kind:'mcp',mode:'json'});
  const editable = document.state === 'structured';
  const group = (owner: SourceScope) => {
    const rows = filtered.filter(entry => entry.owner === owner);
    if (owner !== scope && !rows.length) return null;
    return <div className={css.group} role="group" aria-label={owner === scope ? tx('settings:mcp.installed') : tx(owner === 'user' ? 'settings:catalog.inherited' : 'settings:catalog.shadowed')}>
      <div className={css.groupHeader}><span>{owner === scope ? tx('settings:mcp.installed') : tx(owner === 'user' ? 'settings:catalog.inherited' : 'settings:catalog.shadowed')} <small>{rows.length}</small></span>
        {owner === scope && <div className={css.actions}>
          <Menu open={menu} onClose={() => setMenu(false)} items={[{id:'import',label:tx('settings:mcp.import'),disabled:!editable}]} onSelect={() => {setMenu(false);importConfig();}}
            anchor={<button className={css.iconButton} type="button" aria-label={tx('settings:mcp.more')} aria-haspopup="menu" aria-expanded={menu} onClick={() => setMenu(!menu)}>···</button>}/>
          <button className={css.iconButton} type="button" title={tx('settings:mcp.refresh')} aria-label={tx('settings:mcp.refresh')} disabled={refreshing} onClick={refresh}><IconRefreshOutline16/></button>
          <Button size="sm" variant="primary" disabled={!editable} onClick={create}>＋ {tx('settings:catalog.new')}</Button>
        </div>}
      </div>
      {rows.length ? <div role="list" className={css.list}>{rows.map(entry => <div key={entry.name} role="listitem" aria-label={entry.name} className={css.row}>
        <button className={css.open} type="button" aria-label={tx('settings:extension-detail.mcp-value',{p0:entry.name})} onClick={() => onFocus({kind:'mcp',name:entry.name})}>
          <span className={css.icon}><McpIcon/><span className={css.dot} data-status={entry.valid === false ? 'error' : entry.preparation === 'ready' ? 'ready' : 'unknown'} title={preparationLabel(tx,entry) ?? tx('settings:catalog.unknown')}/></span>
          <span className={css.content}><span>{entry.name}</span><small>{entry.diagnostics[0] ?? description(entry.name,entry.owner,entry.path)}</small></span>
        </button>
        {revision && <ResourceAvailability family="mcp" name={entry.name} valid={entry.valid} source={source} scope={scope} revision={revision} inspect={() => onFocus({kind:'mcp',name:entry.name,mode:'permissions'})}/>}
      </div>)}</div> : <div className={css.empty}>
        <span>{tx(query ? 'settings:catalog.no-results' : 'settings:mcp.empty')}</span><p>{tx(query ? 'settings:catalog.no-results-description' : 'settings:mcp.empty-help')}</p>
        {!query && editable && <div className={css.actions}><Button size="sm" variant="primary" onClick={create}>＋ {tx('settings:mcp.new')}</Button><Button size="sm" variant="outline" onClick={importConfig}>⇩ {tx('settings:mcp.import')}</Button></div>}
      </div>}
    </div>;
  };
  return <div className={css.page} data-mcp-page="">
    <h3>{tx('settings:catalog.mcp')}</h3>
    <div className={css.toolbar}>{scopeControl}<span className={css.total}>MCP <small>{entries.length}</small></span><Search label={tx('settings:mcp.search')} placeholder={tx('settings:mcp.search')} value={query} onChange={setQuery}/></div>
    {document.state !== 'structured' && <p role="alert">{document.state === 'malformed' ? document.diagnostic : tx('settings:source.not-loaded')}</p>}
    {group(scope)}{group(scope === 'user' ? 'workspace' : 'user')}<CollectionDiagnostics source={source} families={['mcp']}/>
  </div>;
}
