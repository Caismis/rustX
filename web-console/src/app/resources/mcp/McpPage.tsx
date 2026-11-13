import { useState, type ReactNode } from 'react';
import type { SourceScope, SourceSettings, TransferOperation } from '../../../../../protocol/app-server/v78';
import { useTranslation } from '../../../locale/react';
import type { CatalogView } from '../../../presentation/catalog/CatalogControls';
import catalog from '../../../presentation/catalog/Catalog.module.css';
import { CardMenu } from '../../../presentation/catalog/CardMenu';
import { transferActions } from '../shared/ResourceTransfer';
import { Button } from '../../../presentation/primitives/Button';
import { Menu } from '../../../presentation/primitives/Menu';
import { IconDownloadOutline16, IconEditOutline16, IconEllipsisOutline16, IconPlusOutline16, IconShieldOutline16 } from '../../../presentation/primitives/icons';
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

export function McpDetail({ source, scope, revision, focus, editorDraft, onEditorDraftChange, scopeControl, onClose }: {
  source: SourceSettings; scope: SourceScope; revision?: string; focus: McpFocus;
  editorDraft?: McpEditorDraft; onEditorDraftChange: (draft?: McpEditorDraft) => void;
  scopeControl: ReactNode; onClose: () => void;
}) {
  const tx = useTranslation();
  const entry = extensionEntries(source, scope, 'mcp', 'owned').find(entry => entry.name === focus.name);
  return <section className={css.page} data-mcp-page="" aria-label={focus.name ? tx('settings:extension-detail.mcp-value',{p0:focus.name}) : tx('settings:mcp.new')}>
    {entry?.diagnostics.map((message, index) => <p role="alert" key={index}>{message}</p>)}
    <McpEditor key={`${scope}:${focus.name ?? ''}:${focus.mode ?? ''}`} source={source} scope={scope} revision={revision} focus={focus}
      editorDraft={editorDraft} onEditorDraftChange={onEditorDraftChange} scopeControl={scopeControl} close={onClose} />
  </section>;
}

export function McpPage({ source, scope, revision, onFocus, query, view, group, probeStates = {}, testConnection, probeAvailable = false, onTransfer, transferDisabled, workspaceAvailability, canOverride, onOverride }: {
  probeStates?: Record<string, ProbeState>;
  testConnection?: (id: string) => void; probeAvailable?: boolean;
  source: SourceSettings; scope: SourceScope; revision?: string;
  onFocus: (focus: McpFocus) => void; query: string; view: CatalogView; group?: string;
  onTransfer?: (scope: SourceScope, name: string, revision: string, operation: TransferOperation) => void;
  canOverride?: (name: string) => boolean; onOverride?: (name: string) => void;
  workspaceAvailability?: (name: string, valid?: boolean) => ReactNode;
  transferDisabled?: boolean;
}) {
  const tx = useTranslation();
  const document = documentAuthoring(scope === 'user' ? source.user_mcp : source.workspace_mcp);
  const entries = extensionEntries(source, scope, 'mcp', 'owned');
  const needle = query.trim().toLocaleLowerCase();
  const matches = entries.map(entry => {
    const config = (entry.owner === 'user' ? source.user_mcp : source.workspace_mcp)?.authored?.[entry.name]?.definition;
    const description = config ? config.url ? `http · ${config.url}` : `stdio · ${[config.command, ...(config.args ?? [])].join(' ')}` : '';
    return {...entry, description};
  }).filter(entry => `${entry.name} ${entry.description} ${entry.path}`.toLocaleLowerCase().includes(needle));
  const renderCard = (entry: typeof matches[number]) => <ResourceCard key={entry.name}
    name={entry.name} label={tx('settings:extension-detail.mcp-value',{p0:entry.name})} icon={<McpStatusIcon entry={entry} probe={probeStates[entry.name]} />}
    description={entry.description} diagnostic={entry.diagnostics[0]} onOpen={() => onFocus({kind:'mcp', name:entry.name})}
    availability={workspaceAvailability?.(entry.name, entry.valid) ?? (revision && <ResourceAvailability family="mcp" name={entry.name} valid={entry.valid} source={source} scope={scope} revision={revision} />)}
    menu={<CardMenu label={tx('resources:card.actions', { p0: entry.name })}
      items={[{ id: 'edit', label: tx('resources:card.edit'), icon: <IconEditOutline16 /> },
        ...(canOverride?.(entry.name) ? [{ id: 'override', label: tx('resources:card.override'), icon: <IconEditOutline16 /> }] : []),
        ...(onTransfer ? transferActions(tx, scope, !!transferDisabled || document.state !== 'structured' || !Object.hasOwn(document.document, entry.name)) : [])]}
      onSelect={action => {
        if (action === 'edit') onFocus({ kind: 'mcp', name: entry.name });
        else if (action === 'override') onOverride?.(entry.name);
        else if (document.state === 'structured') onTransfer?.(scope, entry.name, document.revision, action === 'copy-scope' ? 'copy' : 'move');
      }} />}
    actions={<>
      <Button size="sm" variant="outline" disabled={!probeAvailable || entry.valid === false || probeStates[entry.name] === 'checking'}
        aria-label={`${tx('settings:mcp.test-connection')} ${entry.name}`} onClick={() => testConnection?.(entry.name)}>{tx('settings:mcp.test-connection')}</Button>
      <Button size="sm" variant="ghost" aria-label={`${tx('resources:mcp.permissions')} ${entry.name}`}
        onClick={() => onFocus({kind:'mcp', name:entry.name, mode:'permissions'})}><IconShieldOutline16 /></Button>
    </>} />;

  if (group && needle && !matches.length) return null;
  return <div data-mcp-page="">
    {document.state !== 'structured' && <p className={catalog.banner} role="alert">{document.state === 'malformed' ? document.diagnostic : tx('settings:source.not-loaded')}</p>}
    <ResourceGroup label={group ?? tx('settings:mcp.installed')} count={matches.length} view={view}
      empty={<><p>{tx(needle ? 'settings:catalog.no-results' : 'settings:mcp.empty')}</p><p>{tx(needle ? 'settings:catalog.no-results-description' : 'settings:mcp.empty-help')}</p></>}>
      {matches.map(renderCard)}
    </ResourceGroup>
  </div>;
}
