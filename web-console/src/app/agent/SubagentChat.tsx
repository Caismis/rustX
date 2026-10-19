import { useEffect, useMemo, useRef, useState, useSyncExternalStore, type ReactNode } from 'react';
import type { RuntimeClientAgent, RuntimeClientSnapshot } from '../../../../protocol/app-server/v51';
import type { AppServerClient } from '../../client/app-server';
import type { Observation } from '../../client/session-lifecycle/port';
import { AgentReading, type AgentReadingState } from '../../client/agent-reading';
import { turnAnchor } from '../../client/transcript';
import { useTranslation } from '../../locale/react';
import { ChatViewport } from '../../presentation/layout/ChatViewport';
import { AgentTranscript } from './AgentTranscript';
import { DeferredContent } from './DeferredContent';
import chatCss from '../../presentation/agent/Chat.module.css';
import { currentTurnLocation } from './turn-rail-items';
import { TurnNavigation } from './TurnNavigator';

const empty: AgentReadingState = {};
const noopSubscribe = () => () => {};

/** A child owns its reading window; the watch remains a separate live tail. */
export function SubagentChat({ client, sessionId, admission, agent, snapshot, visible, children }: {
  client?: AppServerClient; sessionId?: string; admission?: Observation; agent: RuntimeClientAgent;
  snapshot?: RuntimeClientSnapshot; visible: boolean; children?: ReactNode;
}) {
  const tx = useTranslation(), viewport = useRef<ChatViewport>(null);
  const [active, setActive] = useState<string | null>();
  const view = useMemo(() => Symbol('child reading view'), [admission, agent.agent_id, agent.child_conversation_id, agent.activation_id, visible]);
  const currentView = useRef(view);
  currentView.current = view;
  const readerOwner = useMemo(() => Symbol('child history owner'), [admission, agent.agent_id, agent.child_conversation_id, agent.activation_id]);
  const currentReaderOwner = useRef(readerOwner); currentReaderOwner.current = readerOwner;
  const currentVisible = useRef(visible); currentVisible.current = visible;
  const reader = useMemo(() => client && sessionId && admission ? new AgentReading(client, admission.target, agent.agent_id, agent.child_conversation_id,
    () => currentVisible.current && currentReaderOwner.current === readerOwner && client.isAttachmentObservationCurrent(sessionId, admission)) : undefined,
  [client, sessionId, admission, agent.agent_id, agent.child_conversation_id, readerOwner]);
  useEffect(() => () => reader?.dispose(), [reader]);
  useEffect(() => { if (!visible) reader?.suspend(); else if (snapshot) reader?.observe(snapshot); }, [reader, snapshot, visible]);
  const reading = useSyncExternalStore(reader?.subscribe ?? noopSubscribe, reader?.getSnapshot ?? (() => empty));
  const ownsContent = useMemo(() => () => visible && currentView.current === view && !!client && !!sessionId && !!admission && client.isAttachmentObservationCurrent(sessionId, admission), [visible, view, client, sessionId, admission]);
  const currentId = useMemo(() => snapshot?.attempt && ['admitted','running'].includes(snapshot.attempt.phase.type) ? {
    conversation_id: snapshot.conversation_id, attempt_id: snapshot.attempt.attempt_id,attempt_id_source:snapshot.attempt.attempt_id_source,
  } : undefined, [snapshot?.conversation_id, snapshot?.attempt?.attempt_id,snapshot?.attempt?.attempt_id_source, snapshot?.attempt?.phase.type]);
  return <ChatViewport chromeVisible={visible} ref={viewport} latestLabel={tx('agent:agent-transcript.return-to-latest')}
    historical={!!reading.history?.window} onLatest={() => reader?.returnToLatest()} onActiveTurn={setActive}
    latestTurn={currentId && turnAnchor(currentId)}
    overlay={reader && <TurnNavigation page={reading.outline} currentId={currentId} location={currentTurnLocation(snapshot)}
      loading={reading.outlineLoading} pending={reading.pending} error={reading.navigationError ?? reading.outlineError}
      attached={visible && !!admission && !!client?.isAttachmentObservationCurrent(sessionId!, admission)} active={active}
      onReload={() => { reader.invalidate(); void reader.refreshTurns(); }}
      onNavigate={selection => {
        const intent = viewport.current?.beginNavigation(reader.invalidate);
        if (intent) void reader.navigate(selection, intent.current).then(turn => { if (turn) intent.commit(turnAnchor(turn.id), intent.current); });
      }}/>}
  >
    {snapshot && <AgentTranscript snapshot={snapshot} history={reading.history} loadEarlier={() => void reader?.loadEarlier()} loadLater={() => void reader?.loadLater()} historicalDisabled/>}
    {client && admission && <DeferredContent client={client} target={admission.target} agentId={agent.agent_id} bindings={reading.history?.page.content ?? []} owned={ownsContent}/>}
    <div className={chatCss.column}>{children}</div>
  </ChatViewport>;
}
