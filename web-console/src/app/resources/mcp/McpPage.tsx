import { useState, type ReactNode } from 'react';
import type { SourceScope, SourceSettings } from '../../../../../protocol/app-server/v77';
import { useTranslation } from '../../../locale/react';
import { Button } from '../../../presentation/primitives/Button';
import { Menu } from '../../../presentation/primitives/Menu';
import { IconSearchOutline16, IconPlusOutline16, IconDownloadOutline16, IconShieldOutline16 } from '../../../presentation/primitives/icons';
import { Search } from '../../settings/primitives/aria';
import { documentAuthoring } from '../../settings/projection';
import type {McpFocus} from '../types';
import { extensionEntries } from '../shared/inventory';
import { CollectionDiagnostics } from '../shared/CollectionDiagnostics';
import { ResourceAvailability } from '../shared/ResourceAvailability';
import type { ProbeState } from './useMcpProbes';
import { McpStatusIcon } from './McpStatusIcon';
import { McpEditor } from './McpEditor';
import css from './McpPage.module.css';

/** Scope-local MCP management using the shared Harness settings presentation. */
export function McpPage({ source, scope, revision, focus, onFocus, scopeControl, probeStates = {}, testConnection, probeAvailable = false }: {
  probeStates?: Record<string, ProbeState>;
  testConnection?: (id: string) => void; probeAvailable?: boolean;
  source: SourceSettings; scope: SourceScope; revision?: string; focus?: McpFocus;
  onFocus: (focus?: McpFocus) => void; scopeControl: ReactNode;
}) {
  const tx = useTranslation();
  const [query, setQuery] = useState('');
  const [menu, setMenu] = useState(false);
  const document = documentAuthoring(scope === 'user' ? source.user_mcp : source.workspace_mcp);
  // Keep local definitions separate from the global resources they inherit.
  const entries = extensionEntries(source, scope, 'mcp');
  const description = (name: string, owner: SourceScope, path: string) => {
    const config = (owner === 'user' ? source.user_mcp : source.workspace_mcp)?.authored?.[name]?.definition;
    return config ? config.url ? `http · ${config.url}` : `stdio · ${[config.command, ...(config.args ?? [])].join(' ')}` : path;
  };
  if (focus) return <section className={css.page} data-mcp-page="" aria-label={focus.name ? tx('settings:extension-detail.mcp-value',{p0:focus.name}) : tx('settings:mcp.new')}>
    {focus.name && entries.find(entry=>entry.name===focus.name)?.diagnostics.map((message,index)=><p role="alert" key={index}>{message}</p>)}
    <McpEditor key={`${scope}:${focus.name ?? ''}:${focus.mode ?? ''}`} source={source} scope={scope} revision={revision} focus={focus} scopeControl={scopeControl} close={() => onFocus(undefined)}/>
  </section>;
  const owned = (entry: typeof entries[number]) => entry.owner===scope || Object.hasOwn(document.state==='structured'?document.document:{},entry.name);
  const filtered = entries.filter(entry => `${entry.name} ${description(entry.name, entry.owner, entry.path)}`.toLocaleLowerCase().includes(query.trim().toLocaleLowerCase()));
  const create = () => onFocus({kind:'mcp'});
  const importConfig = () => onFocus({kind:'mcp',mode:'json'});
  const editable = document.state === 'structured';
  const group = (rows: typeof entries, inherited = false) => {
    return <div className={css.group} role="group" aria-label={tx(inherited ? 'resources:scope.inherited' : 'settings:mcp.installed')}>
      <div className={css.groupHeader}><span>{tx(inherited ? 'resources:scope.inherited' : 'settings:mcp.installed')} <small>{rows.length}</small></span>
        {!inherited && <div className={css.actions}>
          <Menu open={menu} onClose={() => setMenu(false)} items={[{id:'import',label:tx('settings:mcp.import'),icon:<IconDownloadOutline16/>,disabled:!editable}]} onSelect={() => {setMenu(false);importConfig();}}
            anchor={<button className={css.iconButton} type="button" aria-label={tx('settings:mcp.more')} aria-haspopup="menu" aria-expanded={menu} onClick={() => setMenu(!menu)}>⋯</button>}/>
          <Button size="sm" variant="primary" disabled={!editable} onClick={create} aria-label={`＋ ${tx('settings:catalog.new')}`}><IconPlusOutline16/>{tx('settings:catalog.new')}</Button>
        </div>}
      </div>
      {rows.length ? <div role="list" className={css.list}>{rows.map(entry => <div key={entry.name} role="listitem" aria-label={entry.name} className={css.row}>
        <button className={css.open} type="button" aria-label={tx('settings:extension-detail.mcp-value',{p0:entry.name})} onClick={() => onFocus({kind:'mcp',name:entry.name})}>
          <McpStatusIcon entry={entry} probe={probeStates[entry.name]}/>
          <span className={css.content}><span>{entry.name}</span><small title={entry.diagnostics[0] ?? description(entry.name,entry.owner,entry.path)}>{entry.diagnostics[0] ?? description(entry.name,entry.owner,entry.path)}</small></span>
        </button>
        <Button className={css.connection} size="sm" variant="outline" disabled={inherited || !probeAvailable || entry.valid === false || probeStates[entry.name] === 'checking'} aria-label={`${tx('settings:mcp.test-connection')} ${entry.name}`} onClick={() => testConnection?.(entry.name)}>{tx('settings:mcp.test-connection')}</Button>
        <Button className={css.permissions} size="sm" variant="ghost" aria-label={`${tx('resources:mcp.permissions')} ${entry.name}`} onClick={()=>onFocus({kind:'mcp',name:entry.name,mode:'permissions'})}><IconShieldOutline16/></Button>
        {revision && <ResourceAvailability family="mcp" name={entry.name} valid={entry.valid} source={source} scope={scope} revision={revision}/>}
      </div>)}</div> : <div className={css.empty}>
        <span>{tx(query ? 'settings:catalog.no-results' : 'settings:mcp.empty')}</span><p>{tx(query ? 'settings:catalog.no-results-description' : 'settings:mcp.empty-help')}</p>
        {!query && editable && <div className={css.actions}><Button size="sm" variant="primary" onClick={create} aria-label={`＋ ${tx('settings:mcp.new')}`}><IconPlusOutline16/>{tx('settings:mcp.new')}</Button><Button size="sm" variant="outline" onClick={importConfig}><IconDownloadOutline16/>{tx('settings:mcp.import')}</Button></div>}
      </div>}
    </div>;
  };
  return <div className={css.page} data-mcp-page="">
    <div className={css.toolbar}>{scopeControl}<span className={css.total}>MCP <small>{filtered.length}</small></span><div className={css.search}><IconSearchOutline16/><Search label={tx('settings:mcp.search')} placeholder={tx('settings:mcp.search')} value={query} onChange={setQuery}/></div></div>
    {document.state !== 'structured' && <p role="alert">{document.state === 'malformed' ? document.diagnostic : tx('settings:source.not-loaded')}</p>}
    {group(filtered.filter(entry=>owned(entry)))}{filtered.some(entry=>!owned(entry))&&group(filtered.filter(entry=>!owned(entry)),true)}<CollectionDiagnostics source={source} families={['mcp']}/>
  </div>;
}
