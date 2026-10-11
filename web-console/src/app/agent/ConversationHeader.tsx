import { SubagentHeader } from './Subagents';
import { useSubagents } from './subagent-context';
import { useConversationPreferences } from '../conversation-preferences';
import { OpenWorkspace } from './OpenWorkspace';
import type { ProductHostWorkspaces } from '../../workspaces/host';
import type { WorkspaceAuthority } from '../../workspaces/authority';
import { useTranslation } from '../../locale/react';
import { useState, type ReactNode } from 'react';
import type { AppServerClient, SessionView } from '../../client/app-server';
import type { SourceTarget } from '../../../../protocol/app-server/v44';
import { useClientSelector, sameValue } from '../../client/selectors';
import { Menu } from '../../presentation/primitives/Menu';
import { Button } from '../../presentation/primitives/Button';
import { IconEllipsisOutline16, IconInspectOutline12 } from '../../presentation/primitives/icons';
import { navigateTabs } from '../../presentation/primitives/tabs';
import { activeAttempt, lineageSwitchSafe } from '../../bindings/projection';
import { sessionDisplayTitle } from '../../bindings/session-title';
import { SessionConfiguration } from '../SessionConfiguration';
import { WorkspaceControls } from '../new-conversation/WorkspaceControls';
import { AgentControls } from './AgentControls';
import agentCss from '../../presentation/agent/Conversation.module.css';
export function ConversationHeader({ host, authority, client, view, workspaceId, authorityRevision, connected, attached, commandOpen, inspectorOpen, toggleInspector, invokeCommand, openOwningSettings, conversationMode, setConversationMode, settingsFeedback, previewToggle }: {
  host: ProductHostWorkspaces; workspaceId?: string; authority: WorkspaceAuthority; settingsFeedback?: ReactNode; previewToggle?: ReactNode; client: AppServerClient; view?: SessionView; authorityRevision?: number; connected: boolean; attached: boolean; commandOpen: boolean;
  inspectorOpen: boolean; toggleInspector: () => void; invokeCommand: (request: { id: 'tree' }) => void;
  openOwningSettings: (owner: SourceTarget) => void; conversationMode: 'chat' | 'trajectory'; setConversationMode: (mode: 'chat' | 'trajectory') => void;
}) {
  const tx = useTranslation();
  const [displayPreferences] = useConversationPreferences();
  const subagents = useSubagents();
  const [sessionSettingsOpen, setSessionSettingsOpen] = useState(false);
  const owner = JSON.stringify([client.getSnapshot().generation, view?.id]);
  const [failure, setFailure] = useState<{ owner: string; message: string }>();
  const error = failure?.owner === owner ? failure.message : undefined;
  const exportSession = () => { if (!view) return; setFailure(undefined); void client.exportSession(view.id).catch(cause => setFailure({ owner, message: String(cause) })); };
  return <header className={`${agentCss.header} ${!view ? agentCss.headerBlank : ""}`}>
      <div className={agentCss.titleRow}>
        <div className={agentCss.titleCluster}>
          <SubagentHeader title={view ? sessionDisplayTitle(tx, view.summary) : 'rustX'}/>
        </div>
        <div className={agentCss.headerUtilities}>
          {subagents?.selected && <div id="subagent-header-actions"/>}
          {view?.summary && <OpenWorkspace client={client} host={host} authority={authority} target={{ session_id: view.id, active_node: view.summary.active_node }} disabled={!connected || !!view.deleting}/>}
          {view && !subagents?.selected && <SessionActions client={client} sessionId={view.id} connected={connected} attached={attached} commandOpen={commandOpen} settings={() => setSessionSettingsOpen(value => !value)} tree={() => invokeCommand({ id: 'tree' })} exportSession={exportSession}/>}
          <Button size="sm" className={agentCss.iconButton} aria-label={tx('agent:conversation-header.toggle-inspector')} aria-expanded={inspectorOpen} onClick={toggleInspector}><IconInspectOutline12 /></Button>
        </div>
        <div className={agentCss.headerCorner}>{previewToggle}</div>
      </div>
      {view && !subagents?.selected && <SessionConfiguration key={`${authorityRevision}:${view.id}`} client={client} view={view} openOwningSettings={openOwningSettings} />}
      {settingsFeedback}
      {view && !subagents?.selected && sessionSettingsOpen && <section aria-label={tx('agent:conversation-header.session-settings')}><p>{tx('agent:conversation-header.workspace')}{' '}{view.settings?.cwd ?? tx('agent:conversation-header.unavailable')}</p><WorkspaceControls client={client} host={host} workspaceId={workspaceId}>{source => <LiveAgentControls client={client} sessionId={view.id} coldSource={source}/>}</WorkspaceControls><Button onClick={() => setSessionSettingsOpen(false)}>{tx('agent:conversation-header.close-session-settings')}</Button></section>}
      {view && displayPreferences.codingView && <div className={agentCss.tabs} role="tablist" aria-label={tx('agent:conversation-header.conversation-view')} onKeyDown={navigateTabs}>{(['chat', 'trajectory'] as const).map(mode => <button type="button" className={`${agentCss.tab} ${conversationMode === mode ? agentCss.tabActive : ""}`} key={mode} role="tab" id={`view-tab-${mode}`} aria-controls="conversation-view" tabIndex={conversationMode === mode ? 0 : -1} aria-selected={conversationMode === mode} onClick={() => setConversationMode(mode)}>{mode === 'chat' ? tx('agent:conversation-header.chat') : tx('agent:conversation-header.trajectory')}</button>)}</div>}
      {error && <p role="alert">{error}</p>}
      </header>;
}
function LiveAgentControls({ client, sessionId, coldSource }: { client: AppServerClient; sessionId: string; coldSource?: import('../../../../protocol/app-server/v44').SourceSettings }) {
  useClientSelector(client, state => {
    const view = state.views[sessionId];
    return { generation: state.generation, target: view?.target, attachment: view?.attachment, intent: view?.attachmentIntent,
      model: view?.snapshot?.model, modelIntent: view?.modelIntent, settings: view?.settings, resources: view?.snapshot?.resources?.revision,
      running: activeAttempt(view?.snapshot), attemptModel: view?.snapshot?.attempt?.model };
  }, sameValue);
  const view = client.getSnapshot().views[sessionId];
  return <AgentControls client={client} view={view} coldSource={coldSource}/>;
}

function SessionActions({ client, sessionId, connected, attached, commandOpen, settings, tree, exportSession }: {
  client: AppServerClient; sessionId: string; connected: boolean; attached: boolean; commandOpen: boolean;
  settings: () => void; tree: () => void; exportSession: () => void;
}) {
  const tx = useTranslation();
  const [sessionMenuOpen, setSessionMenuOpen] = useState(false);
  const safe = useClientSelector(client, state => lineageSwitchSafe(state.views[sessionId]));
  return <Menu open={sessionMenuOpen} onClose={() => setSessionMenuOpen(false)} align="end" dense autoFocus
          anchor={<Button size="sm" className={agentCss.iconButton} aria-label={tx('agent:conversation-header.session-actions')} aria-haspopup="menu" aria-expanded={sessionMenuOpen} onClick={() => setSessionMenuOpen(value => !value)}><IconEllipsisOutline16 /></Button>}
          items={[{ id: 'settings', label: tx('agent:conversation-header.session-settings') }, { id: 'export', label: tx('agent:conversation-header.export'), disabled: !connected }, { id: 'tree', label: tx('agent:conversation-header.session-tree'), disabled: !attached || commandOpen || !safe }]}
          onSelect={id => { setSessionMenuOpen(false); if (id === 'settings') settings(); else if (id === 'tree') tree(); else if (id === 'export') exportSession(); }} />;
}
