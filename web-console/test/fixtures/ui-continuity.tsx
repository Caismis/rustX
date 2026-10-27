import { useEffect, useState } from 'react';
import { createRoot } from 'react-dom/client';
import { AgentTranscript } from '../../src/app/agent/AgentTranscript';
import { SourceContent } from '../../src/app/agent/SourceContent';
import { TrajectoryLedger } from '../../src/app/trajectory/TrajectoryLedger';
import type { TrajectoryLedgerRow } from '../../src/app/trajectory/layout';
import type { TrajectoryRowActions } from '../../src/app/trajectory/TrajectoryRow';
import { useComposerContent } from '../../src/app/agent/useComposerContent';
import { TodoDock } from '../../src/app/composer/TodoDock';
import { todoDock } from '../../src/bindings/composer-context';
import { conversationPreferences } from '../../src/app/conversation-preferences';
import type { ContentChunk, ContentRef, RuntimeClientGoalSnapshot, RuntimeClientTranscriptEntry, TurnProcessView } from '../../../protocol/app-server/v61';
import { Server, snapshot } from '../fixture';
import '../../src/presentation/theme/base.css';
import '../../src/presentation/theme/design-platform.css';
import '../../src/presentation/theme/reset.css';

const mode = new URLSearchParams(location.search).get('mode');
conversationPreferences().update({ transcriptMode: mode === 'compact' ? 'compact' : 'standard' }, false);
const server = new Server(), tasks = [1, 2, 3].map(id => ({ id: String(id), subject: `Original task ${id}`, status: 'pending' as const }));
server.snapshots.set('A', { ...snapshot(), todos: { tasks: [], next_id: '4' } });
await server.attached('A');
const view = server.client.getSnapshot().views.A, cut = view.checkpoint!.read_cut;
view.checkpoint!.collections.push({ domain: 'todos.tasks', revision: '1', total: '3', loaded: '0', next_offset: '0' });
server.handlers.set('conversation/checkpointPage', () => ({ type: 'checkpoint_page', page: { read_cut: cut, cursor: cut.revision, domain: 'todos.tasks', total: '3', next_offset: null, collections: [], items: tasks.map((value, index) => ({ offset: String(index), identity: value.id, value, content: [], deferred: false })) } }));
const text = Array.from({ length: 80 }, (_, index) => `Reasoning paragraph ${index + 1}: retain the reader's place as this live message is published.`).join('\n\n');
const process: TurnProcessView = { attempt_id_source: null, conversation_id: 'c', attempt_id: 'a', final_message_locator: null, final_message_id_deferred: false, final_message_id_source: null, final_message_id: null, outcome: 'running', control_cursor: '100', message_count: 1, tool_call_count: 0 };
const committed: RuntimeClientTranscriptEntry = { cursor: '100', turn_process: process, item: { type: 'message', message: { role: 'assistant', id: 'live-message', content: [{ type: 'reasoning', text }] } } };
const live = { ...snapshot(), conversation_id: 'c', attempt: { attempt_id: 'a', phase: { type: 'running' as const }, turn: 1, in_flight: { message_id: 'live-message', blocks: [{ type: 'reasoning' as const, block_index: 0, text }] } } };
const prefix = 'Released paragraph.\n'.repeat(1500), suffix = 'An extended prefix.\n';
const reference: ContentRef = { conversation_id: 'c', source: { type: 'live_publication_prefix', component: 'payload', stream_id: 'stream', message_id: 'message', attempt_id: 'a', turn_id: 't', publication_request_id: 'request', block_index: 0 }, representation: 'utf8_text', version: 1, content_revision: 1, readable_end: prefix.length };
const contentChunk = (ref: ContentRef): ContentChunk => ({ content_ref: ref, offset: 0, end_offset: ref.readable_end, readable_end: ref.readable_end, data_base64: btoa(prefix + (ref.content_revision === 2 ? suffix : '')), next_offset: null, complete: true });
let finishPrefix: (() => void) | undefined;
const readPrefix = async (ref: ContentRef) => ref.content_revision === 1 ? contentChunk(ref) : new Promise<ContentChunk>(resolve => { finishPrefix = () => resolve(contentChunk(ref)); });
const rowActions: TrajectoryRowActions = { select: () => {}, toggleTurn: () => {}, toggleCalls: () => {}, foldableTurns: new Set(), callOwners: new Set(), requestNumbers: new Map() };
function Continuity() {
  const [current, setView] = useState(view), [published, publish] = useState(false);
  const [frontier, setFrontier] = useState(reference), [rowCount, setRows] = useState(100);
  const rows: TrajectoryLedgerRow[] = Array.from({ length: rowCount }, (_, index) => ({ display_key: 'row-' + index, kind: 'summary', height: 30, turnStart: false, callsOwner: 'message-' + index, summary: 'Record ' + index }));
  const content = useComposerContent(server.client, current);
  useEffect(() => {
    Object.assign(window, { continuity: {
      publish: () => publish(true),
      advancePrefix: () => setFrontier({ ...reference, content_revision: 2, readable_end: prefix.length + suffix.length }),
      finishPrefix: () => finishPrefix?.(),
      setRows,
      changeGoal: () => {
        server.held.add('conversation/checkpointPage'); server.held.add('conversation/checkpointValueRead');
        const goal: RuntimeClientGoalSnapshot = { reference: { id: 'goal-1', revision: '4' }, objective: 'Independent Goal update', objective_deferred: false, phase: 'blocked', blocked_reason: 'preview', blocked_reason_deferred: true, autonomous_round_budget: 4, autonomous_rounds_consumed: 1, origin: { kind: 'runtime_control' }, last_round_message_id_deferred: false, fields: [] };
        setView({ ...view, snapshot: { ...view.snapshot!, goal: { current: goal } } });
      },
      reads: () => server.requests.filter(row => row.request.method === 'conversation/checkpointPage').length,
    } });
  }, []);
  return <main style={{ width: 740, margin: '24px auto' }}>
    <TodoDock state={todoDock(content.snapshot, content.todoLoaded ? undefined : current.checkpoint?.collections.find(row => row.domain === 'todos.tasks'))}/>
    <AgentTranscript snapshot={published ? { ...live, transcript: { entries: [committed] } } : live}/>
    <section style={{ height: 400, display: 'flex', flexDirection: 'column' }}>
      <TrajectoryLedger ref={null} rows={rows} first="row-0" onFocused={() => {}} folded={new Set()} focusedIds={null} actions={rowActions} close={() => {}} loadEarlier={() => {}} canLoadEarlier={false} loading={false} searching={false}/>
    </section>
    <SourceContent bindings={[{ path: [], content_ref: frontier, deferred: true }]} owned={() => true} read={readPrefix} fallback={<p>Waiting for the live prefix</p>}>
      {text => <pre data-live-prefix style={{ maxHeight: 200, overflow: 'auto' }}>{text}</pre>}
    </SourceContent>
  </main>;
}
createRoot(document.getElementById('root')!).render(<Continuity/>);
