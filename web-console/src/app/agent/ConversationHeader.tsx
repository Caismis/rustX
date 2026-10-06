import { OpenWorkspace } from './OpenWorkspace';
import type { ProductHostWorkspaces } from '../../workspaces/host';
import type { WorkspaceAuthority } from '../../workspaces/authority';
import { useTranslation } from '../../locale/react';
import { useState, type ReactNode } from 'react';
import type { AppServerClient, SessionView } from '../../client/app-server';
import type { SourceTarget } from '../../../../protocol/app-server/v34';
import { useClientSelector } from '../../client/selectors';
import { Menu } from '../../presentation/primitives/Menu';
import { Button } from '../../presentation/primitives/Button';
import { IconInspectOutline12, IconEllipsisOutline16, IconDownloadOutline16, IconForkOutline16 } from '../../presentation/primitives/icons';
import { navigateTabs } from '../../presentation/primitives/tabs';
import { lineageSwitchSafe } from '../../bindings/projection';
import { sessionDisplayTitle } from '../../bindings/session-title';
import { SessionConfiguration } from '../SessionConfiguration';
import agentCss from '../../presentation/agent/Conversation.module.css';
export function ConversationHeader({ host, authority, client, view, authorityRevision, connected, attached, commandOpen, inspectorOpen, toggleInspector, invokeCommand, openOwningSettings, conversationMode, setConversationMode, settingsFeedback }: {
  host: ProductHostWorkspaces; authority: WorkspaceAuthority; settingsFeedback?: ReactNode; client: AppServerClient; view?: SessionView; authorityRevision?: number; connected: boolean; attached: boolean; commandOpen: boolean;
  inspectorOpen: boolean; toggleInspector: () => void; invokeCommand: (request: { id: 'tree' }) => void;
  openOwningSettings: (owner: SourceTarget) => void; conversationMode: 'chat' | 'trajectory'; setConversationMode: (mode: 'chat' | 'trajectory') => void;
}) {
  const tx = useTranslation();
  const owner = JSON.stringify([client.getSnapshot().generation, view?.id]);
  const [failure, setFailure] = useState<{ owner: string; message: string }>();
  const error = failure?.owner === owner ? failure.message : undefined;
  const exportSession = () => { if (!view) return; setFailure(undefined); void client.exportSession(view.id).catch(cause => setFailure({ owner, message: String(cause) })); };
  return <header className={`${agentCss.header} ${!view ? agentCss.headerBlank : ""}`}><div className={`${agentCss.titleRow} agent-title-row`}><div className={agentCss.titleCluster}><strong id="session-title" aria-label={view ? tx('agent:conversation-header.session-title') : tx('agent:conversation-header.product-title')}>{view ? sessionDisplayTitle(tx, view.summary) : 'rustX'}</strong></div>
        <div className="row">{view?.summary && <OpenWorkspace client={client} host={host} authority={authority} target={{ session_id: view.id, active_node: view.summary.active_node }} disabled={!connected || !!view.deleting}/>} {view && <SessionActions client={client} sessionId={view.id} connected={connected} attached={attached} commandOpen={commandOpen} tree={() => invokeCommand({ id: 'tree' })} exportSession={exportSession}/>}

          <Button aria-label={tx('agent:conversation-header.toggle-inspector')} aria-expanded={inspectorOpen} onClick={toggleInspector}><IconInspectOutline12 /></Button></div>
      </div>
      {view && <SessionConfiguration key={`${authorityRevision}:${view.id}`} client={client} view={view} openOwningSettings={openOwningSettings} />}
      {settingsFeedback}
      {view && <div className={agentCss.tabs} role="tablist" aria-label={tx('agent:conversation-header.conversation-view')} onKeyDown={navigateTabs}>{(['chat', 'trajectory'] as const).map(mode => <Button className={`${agentCss.tab} ${conversationMode === mode ? agentCss.tabActive : ""}`} key={mode} role="tab" id={`view-tab-${mode}`} aria-controls="conversation-view" tabIndex={conversationMode === mode ? 0 : -1} aria-selected={conversationMode === mode} onClick={() => setConversationMode(mode)}>{mode === 'chat' ? tx('agent:conversation-header.chat') : tx('agent:conversation-header.trajectory')}</Button>)}</div>}
      {error && <p role="alert">{error}</p>}
      </header>;
}
function SessionActions({ client, sessionId, connected, attached, commandOpen, tree, exportSession }: {
  client: AppServerClient; sessionId: string; connected: boolean; attached: boolean; commandOpen: boolean;
  tree: () => void; exportSession: () => void;
}) {
  const tx = useTranslation();
  const [sessionMenuOpen, setSessionMenuOpen] = useState(false);
  const safe = useClientSelector(client, state => lineageSwitchSafe(state.views[sessionId]));
  return <Menu open={sessionMenuOpen} onClose={() => setSessionMenuOpen(false)} align="end" autoFocus
          anchor={<Button aria-label={tx('agent:conversation-header.session-actions')} aria-haspopup="menu" aria-expanded={sessionMenuOpen} onClick={() => setSessionMenuOpen(value => !value)}><IconEllipsisOutline16 /></Button>}
          items={[{ id: 'export', label: tx('agent:conversation-header.export'), icon: <IconDownloadOutline16 />, disabled: !connected }, { id: 'tree', label: tx('agent:conversation-header.session-tree'), icon: <IconForkOutline16 />, disabled: !attached || commandOpen || !safe }]}
          onSelect={id => { setSessionMenuOpen(false); if (id === 'tree') tree(); else if (id === 'export') exportSession(); }} />;
}
