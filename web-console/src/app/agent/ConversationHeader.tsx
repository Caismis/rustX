import { useToastNotice } from '../../locale/use-toast-notice';
import { Toast } from '../../presentation/primitives/Toast';
import { IconWarningOutline16 } from '../../presentation/primitives/icons';
import { actionError } from '../../bindings/action-error';
import { BtwPanel } from './BtwPanel';
import { SubagentHeader } from './Subagents';
import { BackgroundJobs } from './BackgroundJobs';
import { useSubagents } from './subagent-context';
import { useConversationPreferences } from '../conversation-preferences';
import { OpenWorkspace } from './OpenWorkspace';
import type { ProductHostWorkspaces } from '../../workspaces/host';
import type { WorkspaceAuthority } from '../../workspaces/authority';
import { useTranslation } from '../../locale/react';
import { useState, type ReactNode } from 'react';
import type { AppServerClient } from '../../client/app-server';
import type { SessionView } from '../../client/contracts';
import type { SourceTarget } from '../../../../protocol/app-server/v54';
import { useClientSelector } from '../../client/selectors';
import { Menu } from '../../presentation/primitives/Menu';
import { Button } from '../../presentation/primitives/Button';
import { IconEllipsisOutline16, IconDownloadOutline16, IconForkOutline16 } from '../../presentation/primitives/icons';
import { navigateTabs } from '../../presentation/primitives/tabs';
import { lineageSwitchSafe } from '../../bindings/projection';
import { sessionDisplayTitle } from '../../bindings/session-title';
import { SessionConfiguration } from '../SessionConfiguration';
import agentCss from '../../presentation/agent/Conversation.module.css';
export function ConversationHeader({ host, authority, client, view, authorityRevision, connected, attached, commandOpen, invokeCommand, openOwningSettings, conversationMode, setConversationMode, settingsFeedback, previewToggle }: {
  host: ProductHostWorkspaces; authority: WorkspaceAuthority; settingsFeedback?: ReactNode; previewToggle?: ReactNode; client: AppServerClient; view?: SessionView; authorityRevision?: number; connected: boolean; attached: boolean; commandOpen: boolean;
  invokeCommand: (request: { id: 'tree' }) => void;
  openOwningSettings: (owner: SourceTarget) => void; conversationMode: 'chat' | 'trajectory'; setConversationMode: (mode: 'chat' | 'trajectory') => void;
}) {
  const tx = useTranslation();
  const [displayPreferences] = useConversationPreferences();
  const subagents = useSubagents();
  const owner = JSON.stringify([client.getSnapshot().generation, view?.id]);
  const [failure, setFailure] = useToastNotice(owner);
  const error = failure?.text;
  const exportSession = () => { if (!view) return; setFailure(''); void client.exportSession(view.id).catch(cause => setFailure(actionError(cause))); };
  return <header className={`${agentCss.header} ${!view ? agentCss.headerBlank : ""}`}>
      <div className={agentCss.titleRow}>
        {view && <div className={agentCss.titleCluster}>
          <SubagentHeader title={sessionDisplayTitle(tx, view.summary)}/>
          {!subagents?.selected && <BackgroundJobs client={client} sessionId={view.id}/>}
        </div>}
        <div className={agentCss.headerUtilities}>
          {view && !subagents?.selected && <BtwPanel key={JSON.stringify([authorityRevision, view.id, view.target])} client={client} sessionId={view.id}/>}
          {subagents?.selected && <div id="subagent-header-actions"/>}
          {view?.summary && <OpenWorkspace client={client} host={host} authority={authority} target={{ session_id: view.id, active_node: view.summary.active_node }} disabled={!connected || !!view.deleting}/>}
          {view && !subagents?.selected && <SessionActions client={client} sessionId={view.id} connected={connected} attached={attached} commandOpen={commandOpen} tree={() => invokeCommand({ id: 'tree' })} exportSession={exportSession}/>}
        </div>
        <div className={agentCss.headerCorner}>{previewToggle}</div>
      </div>
      {view && !subagents?.selected && <SessionConfiguration key={`${authorityRevision}:${view.id}`} client={client} view={view} openOwningSettings={openOwningSettings} />}
      {settingsFeedback}
      {view && displayPreferences.codingView && <div className={agentCss.tabs} role="tablist" aria-label={tx('agent:conversation-header.conversation-view')} onKeyDown={navigateTabs}>{(['chat', 'trajectory'] as const).map(mode => <button type="button" className={`${agentCss.tab} ${conversationMode === mode ? agentCss.tabActive : ""}`} key={mode} role="tab" id={`view-tab-${mode}`} aria-controls="conversation-view" tabIndex={conversationMode === mode ? 0 : -1} aria-selected={conversationMode === mode} onClick={() => setConversationMode(mode)}>{mode === 'chat' ? tx('agent:conversation-header.chat') : tx('agent:conversation-header.trajectory')}</button>)}</div>}
      {error && <Toast key={failure?.sequence} text={error} holdMs={6000} icon={<IconWarningOutline16/>} onDone={() => setFailure('')}/>}
      </header>;
}
function SessionActions({ client, sessionId, connected, attached, commandOpen, tree, exportSession }: {
  client: AppServerClient; sessionId: string; connected: boolean; attached: boolean; commandOpen: boolean;
  tree: () => void; exportSession: () => void;
}) {
  const tx = useTranslation();
  const [sessionMenuOpen, setSessionMenuOpen] = useState(false);
  const safe = useClientSelector(client, state => lineageSwitchSafe(state.views[sessionId]));
  return <Menu open={sessionMenuOpen} onClose={() => setSessionMenuOpen(false)} align="end" dense autoFocus
          anchor={<Button size="sm" className={agentCss.iconButton} aria-label={tx('agent:conversation-header.session-actions')} aria-haspopup="menu" aria-expanded={sessionMenuOpen} onClick={() => setSessionMenuOpen(value => !value)}><IconEllipsisOutline16 /></Button>}
          items={[{ id: 'export', label: tx('agent:conversation-header.export'), icon: <IconDownloadOutline16/>, disabled: !connected }, { id: 'tree', label: tx('agent:conversation-header.session-tree'), icon: <IconForkOutline16/>, disabled: !attached || commandOpen || !safe }]}
          onSelect={id => { setSessionMenuOpen(false); if (id === 'tree') tree(); else if (id === 'export') exportSession(); }} />;
}
