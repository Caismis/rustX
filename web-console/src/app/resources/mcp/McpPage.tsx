import { useState, type ReactNode } from 'react';
import type { SourceScope, SourceSettings } from '../../../../../protocol/app-server/v78';
import { useTranslation } from '../../../locale/react';
import type { CatalogView } from '../../../presentation/catalog/CatalogControls';
import catalog from '../../../presentation/catalog/Catalog.module.css';
import { Button } from '../../../presentation/primitives/Button';
import { Menu } from '../../../presentation/primitives/Menu';
import { IconDownloadOutline16, IconEllipsisOutline16, IconPlusOutline16, IconShieldOutline16 } from '../../../presentation/primitives/icons';
import { documentAuthoring } from '../../settings/projection';
import { extensionEntries } from '../shared/inventory';
import { ResourceAvailability } from '../shared/ResourceAvailability';
import { ResourceCard, ResourceGroup } from '../shared/ResourceCards';
import type { McpEditorDraft, McpFocus } from '../types';
import type { ProbeState } from './useMcpProbes';
import { McpStatusIcon } from './McpStatusIcon';
import { McpEditor } from './McpEditor';
import css from './McpPage.module.css';

export function McpActions({ disabled, onFocus }: { disabled: boolean; onFocus: (focus: McpFocus) => void }) {
  const tx = useTranslation();
  const [menu, setMenu] = useState(false);
  return <div className={css.actions}>
    <Button variant="primary" icon={<IconPlusOutline16 />} disabled={disabled} onClick={() => onFocus({kind:'mcp'})}
      aria-label={`＋ ${tx('settings:mcp.new')}`}>{tx('settings:mcp.new')}</Button>
    <Menu open={menu} onClose={() => setMenu(false)}
      items={[{id:'import', label:tx('settings:mcp.import'), icon:<IconDownloadOutline16 />}]}
      onSelect={() => { setMenu(false); onFocus({kind:'mcp',mode:'json'}); }}
      anchor={<button className={catalog.dots} type="button" disabled={disabled} aria-label={tx('settings:mcp.more')}
        aria-haspopup="menu" aria-expanded={menu} onClick={() => setMenu(!menu)}><IconEllipsisOutline16 /></button>} />
  </div>;
}

export function McpPage({ source, scope, revision, focus, onFocus, editorDraft, onEditorDraftChange, scopeControl, query, view, probeStates = {}, testConnection, probeAvailable = false }: {
  probeStates?: Record<string, ProbeState>;
  testConnection?: (id: string) => void; probeAvailable?: boolean;
  source: SourceSettings; scope: SourceScope; revision?: string; focus?: McpFocus;
  editorDraft?: McpEditorDraft; onEditorDraftChange: (draft?: McpEditorDraft) => void;
  onFocus: (focus?: McpFocus) => void; scopeControl: ReactNode; query: string; view: CatalogView;
}) {
  const tx = useTranslation();
  const document = documentAuthoring(scope === 'user' ? source.user_mcp : source.workspace_mcp);
  const entries = extensionEntries(source, scope, 'mcp');
  if (focus) return <section className={css.page} data-mcp-page="" aria-label={focus.name ? tx('settings:extension-detail.mcp-value',{p0:focus.name}) : tx('settings:mcp.new')}>
    {focus.name && entries.find(entry => entry.name === focus.name)?.diagnostics.map((message, index) => <p role="alert" key={index}>{message}</p>)}
    <McpEditor key={`${scope}:${focus.name ?? ''}:${focus.mode ?? ''}`} source={source} scope={scope} revision={revision} focus={focus}
      editorDraft={editorDraft} onEditorDraftChange={onEditorDraftChange} scopeControl={scopeControl} close={() => onFocus(undefined)} />
  </section>;

  const needle = query.trim().toLocaleLowerCase();
  const matches = entries.map(entry => {
    const config = (entry.owner === 'user' ? source.user_mcp : source.workspace_mcp)?.authored?.[entry.name]?.definition;
    const description = config ? config.url ? `http · ${config.url}` : `stdio · ${[config.command, ...(config.args ?? [])].join(' ')}` : '';
    return {...entry, description};
  }).filter(entry => `${entry.name} ${entry.description} ${entry.path}`.toLocaleLowerCase().includes(needle));
  const isOwned = (entry: typeof entries[number]) => entry.owner === scope || Object.hasOwn(document.state === 'structured' ? document.document : {}, entry.name);
  const installed = matches.filter(isOwned);
  const inherited = matches.filter(entry => !isOwned(entry));

  const renderCard = (entry: typeof matches[number], inherited: boolean) => <ResourceCard key={entry.name}
    name={entry.name} label={tx('settings:extension-detail.mcp-value',{p0:entry.name})} icon={<McpStatusIcon entry={entry} probe={probeStates[entry.name]} />}
    description={entry.description} diagnostic={entry.diagnostics[0]} onOpen={() => onFocus({kind:'mcp', name:entry.name})}
    availability={revision && <ResourceAvailability family="mcp" name={entry.name} valid={entry.valid} source={source} scope={scope} revision={revision} />}
    actions={<>
      <Button size="sm" variant="outline" disabled={inherited || !probeAvailable || entry.valid === false || probeStates[entry.name] === 'checking'}
        aria-label={`${tx('settings:mcp.test-connection')} ${entry.name}`} onClick={() => testConnection?.(entry.name)}>{tx('settings:mcp.test-connection')}</Button>
      <Button size="sm" variant="ghost" aria-label={`${tx('resources:mcp.permissions')} ${entry.name}`}
        onClick={() => onFocus({kind:'mcp', name:entry.name, mode:'permissions'})}><IconShieldOutline16 /></Button>
    </>} />;

  return <div data-mcp-page="">
    {document.state !== 'structured' && <p className={catalog.banner} role="alert">{document.state === 'malformed' ? document.diagnostic : tx('settings:source.not-loaded')}</p>}
    <ResourceGroup label={tx('settings:mcp.installed')} count={installed.length} view={view}
      empty={<><p>{tx(needle ? 'settings:catalog.no-results' : 'settings:mcp.empty')}</p><p>{tx(needle ? 'settings:catalog.no-results-description' : 'settings:mcp.empty-help')}</p></>}>
      {installed.map(entry => renderCard(entry, false))}
    </ResourceGroup>
    {!!inherited.length && <ResourceGroup label={tx('resources:scope.inherited')} count={inherited.length} view={view} empty={null}>
      {inherited.map(entry => renderCard(entry, true))}
    </ResourceGroup>}
  </div>;
}
