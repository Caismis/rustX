import { useState, type ReactNode } from 'react';
import type { AppServerClient, SessionView } from '../../client/app-server';
import type { SourceTarget } from '../../../../protocol/app-server/v25';
import { useClientSelector, sameValue } from '../../client/selectors';
import { Menu } from '../../presentation/primitives/Menu';
import { Button } from '../../presentation/primitives/Button';
import { IconInspectOutline12 } from '../../presentation/primitives/icons';
import { navigateTabs } from '../../presentation/primitives/tabs';
import { activeAttempt, lineageSwitchSafe } from '../../bindings/projection';
import { sessionDisplayTitle } from '../../bindings/session-title';
import { SessionConfiguration } from '../SessionConfiguration';
import { AgentControls } from './AgentControls';
import agentCss from '../../presentation/agent/Conversation.module.css';
export function ConversationHeader({ client, view, authorityRevision, connected, attached, commandOpen, inspectorOpen, toggleInspector, invokeCommand, openOwningSettings, conversationMode, setConversationMode, settingsFeedback }: {
  settingsFeedback?: ReactNode; client: AppServerClient; view?: SessionView; authorityRevision?: number; connected: boolean; attached: boolean; commandOpen: boolean;
  inspectorOpen: boolean; toggleInspector: () => void; invokeCommand: (request: { id: 'tree' }) => void;
  openOwningSettings: (owner: SourceTarget) => void; conversationMode: 'chat' | 'trajectory'; setConversationMode: (mode: 'chat' | 'trajectory') => void;
}) {
  const [sessionSettingsOpen, setSessionSettingsOpen] = useState(false);
  const owner = JSON.stringify([client.getSnapshot().generation, view?.id]);
  const [failure, setFailure] = useState<{ owner: string; message: string }>();
  const error = failure?.owner === owner ? failure.message : undefined;
  const exportSession = () => { if (!view) return; setFailure(undefined); void client.exportSession(view.id).catch(cause => setFailure({ owner, message: String(cause) })); };
  return <header className={`${agentCss.header} ${!view ? agentCss.headerBlank : ""}`}><div className={`${agentCss.titleRow} agent-title-row`}><div className={agentCss.titleCluster}><strong id="session-title" aria-label={view ? "Session title" : "Product title"}>{view ? sessionDisplayTitle(view.summary) : 'rustX'}</strong></div>
        <div className="row">{view && <SessionActions client={client} sessionId={view.id} connected={connected} attached={attached} commandOpen={commandOpen} settings={() => setSessionSettingsOpen(value => !value)} tree={() => invokeCommand({ id: 'tree' })} exportSession={exportSession}/>}

          <Button aria-label="Toggle Inspector" aria-expanded={inspectorOpen} onClick={toggleInspector}><IconInspectOutline12 /></Button></div>
      </div>
      {view && <SessionConfiguration key={`${authorityRevision}:${view.id}`} client={client} view={view} openOwningSettings={openOwningSettings} />}
      {settingsFeedback}
      {view && sessionSettingsOpen && <section aria-label="Session settings"><p>Workspace: {view.settings?.cwd ?? 'Unavailable'}</p><LiveAgentControls client={client} sessionId={view.id} /><Button onClick={() => setSessionSettingsOpen(false)}>Close Session settings</Button></section>}
      {view && <div className={agentCss.tabs} role="tablist" aria-label="Conversation view" onKeyDown={navigateTabs}>{(['chat', 'trajectory'] as const).map(mode => <Button className={`${agentCss.tab} ${conversationMode === mode ? agentCss.tabActive : ""}`} key={mode} role="tab" id={`view-tab-${mode}`} aria-controls="conversation-view" tabIndex={conversationMode === mode ? 0 : -1} aria-selected={conversationMode === mode} onClick={() => setConversationMode(mode)}>{mode === 'chat' ? 'Chat' : 'Trajectory'}</Button>)}</div>}
      {error && <p role="alert">{error}</p>}
      </header>;
}
function LiveAgentControls({ client, sessionId }: { client: AppServerClient; sessionId: string }) {
  useClientSelector(client, state => {
    const view = state.views[sessionId];
    return { generation: state.generation, target: view?.target, attachment: view?.attachment, intent: view?.attachmentIntent,
      model: view?.snapshot?.model, resources: view?.snapshot?.resources?.revision,
      running: activeAttempt(view?.snapshot), attemptModel: view?.snapshot?.attempt?.model };
  }, sameValue);
  const view = client.getSnapshot().views[sessionId];
  return <AgentControls client={client} view={view}/>;
}

function SessionActions({ client, sessionId, connected, attached, commandOpen, settings, tree, exportSession }: {
  client: AppServerClient; sessionId: string; connected: boolean; attached: boolean; commandOpen: boolean;
  settings: () => void; tree: () => void; exportSession: () => void;
}) {
  const [sessionMenuOpen, setSessionMenuOpen] = useState(false);
  const safe = useClientSelector(client, state => lineageSwitchSafe(state.views[sessionId]));
  return <Menu open={sessionMenuOpen} onClose={() => setSessionMenuOpen(false)} align="end" autoFocus
          anchor={<Button aria-label="Session actions" aria-haspopup="menu" aria-expanded={sessionMenuOpen} onClick={() => setSessionMenuOpen(value => !value)}>•••</Button>}
          items={[{ id: 'settings', label: 'Session settings' }, { id: 'export', label: 'Export', disabled: !connected }, { id: 'tree', label: 'Session tree', disabled: !attached || commandOpen || !safe }]}
          onSelect={id => { setSessionMenuOpen(false); if (id === 'settings') settings(); else if (id === 'tree') tree(); else if (id === 'export') exportSession(); }} />;
}
