import { useEffect, useState } from 'react';
import { createRoot } from 'react-dom/client';
import { AgentTranscript } from '../../src/app/agent/AgentTranscript';
import { WorkflowRunPanel } from '../../src/app/agent/WorkflowRunPanel';
import { ArtifactPreview } from '../../src/app/components/ArtifactPreview';
import { TaskManagerPage } from '../../src/app/scheduler/TaskManagerPage';
import { FilePreviewCoordinator, FilePreviewLease, type PreviewSource } from '../../src/client/session-files';
import { PreviewContentOwner } from '../../src/client/preview-content';
import { WorkspaceAuthority } from '../../src/workspaces/authority';
import { schedulerFixture, task } from '../scheduler-fixture';
import { workflowRun } from '../workflow-data';
import { App } from '../../src/app/App';
import { ConversationLive } from '../../src/app/agent/ConversationLive';
import { firstSubmissions } from '../../src/app/new-conversation/first-submit';
import { SourceContent } from '../../src/app/agent/SourceContent';
import { JournalSourceProvider } from '../../src/app/agent/ObservationSource';
import { TrajectoryLedger } from '../../src/app/trajectory/TrajectoryLedger';
import type { TrajectoryLedgerRow } from '../../src/app/trajectory/layout';
import type { TrajectoryRowActions } from '../../src/app/trajectory/TrajectoryRow';
import { useComposerContent } from '../../src/app/agent/useComposerContent';
import { TodoDock } from '../../src/app/composer/TodoDock';
import { todoDock } from '../../src/bindings/composer-context';
import { conversationPreferences } from '../../src/app/conversation-preferences';
import type { ContentChunk, ContentRef, MessageBlock, ForegroundToolExecution, RuntimeClientGoalSnapshot, RuntimeClientMessageDescriptor, RuntimeClientTranscriptEntry, TurnProcessView } from '../../../protocol/app-server/v61';
import { Server, snapshot, checkpoint, projectionHead } from '../fixture';
import { foldRuntimeEvent } from '../../../protocol/app-server/projection';
import '../../src/presentation/theme/base.css';
import '../../src/presentation/theme/design-platform.css';
import '../../src/presentation/theme/reset.css';

const mode = new URLSearchParams(location.search).get('mode');
const scene = new URLSearchParams(location.search).get('scene');
const descriptorScene = scene === 'tools-descriptor' || scene === 'tools-descriptor-compaction';
const compactedTools = scene === 'tools-compaction' || scene === 'tools-descriptor-compaction';
const deferredReading = scene === 'reading-deferred-user' || scene === 'reading-deferred-assistant';
const deferredText = 'Released deferred text. '.repeat(2000).slice(0, 35000);
if (scene === 'first') await import('../../src/app/console.css');
conversationPreferences().update({ transcriptMode: mode === 'compact' ? 'compact' : 'standard' }, false);
const server = new Server(), tasks = [1, 2, 3].map(id => ({ id: String(id), subject: `Original task ${id}`, status: 'pending' as const }));
if (scene === 'first') server.workspaceHost.classifyLocations = async cwds => cwds.map(cwd => ({ authorized: true, workspaceId: cwd.endsWith('/B') ? 'workspace-b' : 'workspace-a' }));
const readingSnapshot = (id = 'A') => {
  const value = snapshot(id);
  const messages: MessageBlock[] = Array.from({ length: 50 }, (_, index) => {
    const deferred = deferredReading && id === 'A' && index === 5, messageId = 'reading-' + id + '-' + index;
    const content = [{ type: 'text' as const, text: deferred ? '' : 'Retained conversation reading paragraph ' + index }];
    return deferred && scene === 'reading-deferred-assistant' ? { role: 'assistant', id: messageId, content } : { role: 'user', source: 'human', id: messageId, content };
  });
  return { ...value, messages, transcript: { entries: messages.map((message, index) => ({ cursor: String(index + 1), item: { type: 'message' as const, message } })) } };
};
server.snapshots.set('A', { ...snapshot(), todos: { tasks: [], next_id: '4' } });
if (scene === 'reading' || deferredReading) server.snapshots.set('A', readingSnapshot());
if (deferredReading) {
  const ref: ContentRef = { conversation_id: 'conversation-A', source: { type: 'canonical_message_block', component: 'payload', locator: { type: 'by_message_id', message_id: 'reading-A-5' }, block_index: 0 }, representation: 'utf8_text', version: 1, content_revision: 1, readable_end: deferredText.length };
  server.handlers.set('session/checkpoint', request => { if (request.method !== 'session/checkpoint') throw Error('checkpoint'); const value = request.params.target.conversation_id === 'conversation-A' ? server.snapshots.get('A')! : readingSnapshot('B'); return { type: 'checkpoint', checkpoint: { ...checkpoint(value), content: value.conversation_id === 'conversation-A' ? [{ path: ['messages', '5', 'content', '0', 'text'], content_ref: ref, deferred: true }] : [] } }; });
  server.handlers.set('conversation/contentRead', request => { if (request.method !== 'conversation/contentRead') throw Error('content'); const offset = request.params.offset, part = deferredText.slice(offset, offset + request.params.max_bytes), end = offset + part.length; return { type: 'content_chunk', chunk: { content_ref: request.params.content_ref, offset, end_offset: end, readable_end: deferredText.length, data_base64: btoa(part), next_offset: end < deferredText.length ? end : null, complete: end === deferredText.length } }; });
}
server.handlers.set('workflow/list', request => { if (request.method !== 'workflow/list') throw Error('library'); return { type: 'workflow_library', library: { target: request.params.target, directory: '/workflows', absent_revision: 'empty', files: [], diagnostic: null } }; });
await server.attached('A');
if (scene === 'first') {
  localStorage.setItem('rustx-console-view-v2', JSON.stringify({ endpoint: server.client.getSnapshot().endpoint, openViews: ['A'] }));
  server.held.add('session/attach');
}
const view = server.client.getSnapshot().views.A, cut = view.checkpoint!.read_cut;
view.checkpoint!.collections.push({ domain: 'todos.tasks', revision: '1', total: '3', loaded: '0', next_offset: '0' });
server.handlers.set('conversation/checkpointPage', () => ({ type: 'checkpoint_page', page: { read_cut: cut, cursor: cut.revision, domain: 'todos.tasks', total: '3', next_offset: null, collections: [], items: tasks.map((value, index) => ({ offset: String(index), identity: value.id, value, content: [], deferred: false })) } }));
const text = Array.from({ length: 80 }, (_, index) => `Reasoning paragraph ${index + 1}: retain the reader's place as this live message is published.`).join('\n\n');
const process: TurnProcessView = { attempt_id_source: null, conversation_id: descriptorScene ? 'conversation-A' : 'c', attempt_id: 'a', final_message_locator: null, final_message_id_deferred: false, final_message_id_source: null, final_message_id: null, outcome: 'running', control_cursor: '100', message_count: 1, tool_call_count: 0 };
const committed: RuntimeClientTranscriptEntry = { cursor: '100', turn_process: process, item: { type: 'message', message: { role: 'assistant', id: 'live-message', content: [{ type: 'reasoning', text }] } } };
const live = { ...snapshot(), conversation_id: 'c', attempt: { attempt_id: 'a', phase: { type: 'running' as const }, turn: 1, in_flight: { message_id: 'live-message', blocks: [{ type: 'reasoning' as const, block_index: 0, text }] } } };
const prefix = 'Released paragraph.\n'.repeat(1500), suffix = 'An extended prefix.\n';
const reference: ContentRef = { conversation_id: 'c', source: { type: 'live_publication_prefix', component: 'payload', stream_id: 'stream', message_id: 'message', attempt_id: 'a', turn_id: 't', publication_request_id: 'request', block_index: 0 }, representation: 'utf8_text', version: 1, content_revision: 1, readable_end: prefix.length };
const contentChunk = (ref: ContentRef): ContentChunk => ({ content_ref: ref, offset: 0, end_offset: ref.readable_end, readable_end: ref.readable_end, data_base64: btoa(prefix + (ref.content_revision === 2 ? suffix : '')), next_offset: null, complete: true });
let finishPrefix: (() => void) | undefined;
const readPrefix = async (ref: ContentRef) => ref.content_revision === 1 ? contentChunk(ref) : new Promise<ContentChunk>(resolve => { finishPrefix = () => resolve(contentChunk(ref)); });
const rowActions: TrajectoryRowActions = { select: () => {}, toggleTurn: () => {}, toggleCalls: () => {}, foldableTurns: new Set(), callOwners: new Set(), requestNumbers: new Map() };
const execution = (message: string, id: string): ForegroundToolExecution => ({ occurrence: { locator: { type: 'by_message_id', message_id: message }, block_index: 0 }, message_id: message, block_index: 0, call_id: id, tool_id: 'tool-read', name: 'read', assembly_complete: true, committed_result_deferred: false, state: { type: 'running', arguments: JSON.stringify({ path: message + '.txt' }) } });
const firstTool = execution('first', 'first-call'), secondTool = execution('second', 'second-call');
const secondTools: ForegroundToolExecution[] = descriptorScene ? Array.from({ length: 20 }, (_, index) => ({ ...execution('second', index === 0 ? 'second-call' : 'extra-call-' + index), block_index: index, occurrence: { locator: { type: 'by_message_id', message_id: 'second' }, block_index: index }, state: { type: 'running', arguments: JSON.stringify({ path: 'x'.repeat(920) }) } })) : [secondTool];
const firstToolEntry: RuntimeClientTranscriptEntry = { cursor: '99', turn_process: process, tool_calls: [firstTool], item: { type: 'message', message: { role: 'assistant', id: 'first', content: [{ type: 'tool_call', id: 'first-call', tool_id: 'tool-read', name: 'read', arguments: { path: 'first.txt' } }] } } };
const secondToolEntry: RuntimeClientTranscriptEntry = { cursor: '101', turn_process: process, tool_calls: secondTools, item: { type: 'message', message: { role: 'assistant', id: 'second', content: secondTools.map(tool => ({ type: 'tool_call', id: tool.call_id!, tool_id: tool.tool_id!, name: tool.name, arguments: JSON.parse(tool.state.arguments) })) } } };
const toolResultEntry: RuntimeClientTranscriptEntry = { cursor: '100', turn_process: process, item: { type: 'message', message: { role: 'tool', id: 'first-result', occurrence: { assistant_message_id: 'first', block_index: 0 }, tool_call_id: 'first-call', tool_id: 'tool-read', result: { status: { type: 'success' }, duration_ms: '1', content: [] } } } };
const toolsLive = { ...snapshot(), conversation_id: process.conversation_id, transcript: { entries: compactedTools ? [firstToolEntry, toolResultEntry] : [firstToolEntry] }, attempt: { attempt_id: 'a', phase: { type: 'running' as const }, turn: 1, foreground: secondTools, in_flight: { message_id: 'second', blocks: secondTools.map(tool => ({ type: 'tool_call' as const, block_index: tool.block_index, call_id: tool.call_id, tool_id: tool.tool_id, name: tool.name, arguments: tool.state.arguments })) } } };
if (secondToolEntry.item.type !== 'message') throw Error('Expected fixture message');
const toolsDescriptor: RuntimeClientMessageDescriptor = { identity: 'second', locator: { type: 'by_message_id', message_id: 'second' }, role: 'assistant', kind: 'message', block_count: String(secondTools.length), tool_call_count: String(secondTools.length), tool_calls: [], turn_process: process, content: [], collections: [{ domain: 'messages.message:second.blocks', revision: '1', total: String(secondTools.length), loaded: '0', next_offset: '0' }] };
const toolsCanonical = descriptorScene ? foldRuntimeEvent(toolsLive, { type: 'message_descriptor_committed', descriptor: toolsDescriptor, head: [toolsDescriptor] })
  : foldRuntimeEvent(toolsLive, { type: 'message_committed', attempt_id: 'a', message: secondToolEntry.item.message, transcript_cursor: '101' });
const toolsHydrated = { ...toolsCanonical, message_descriptors: [{ ...toolsDescriptor, tool_calls: secondTools }] };
const laterToolResults: RuntimeClientTranscriptEntry[] = secondTools.map((tool, index) => ({ cursor: String(102 + index), turn_process: process, item: { type: 'message', message: { role: 'tool', id: `result-${index}`, occurrence: { assistant_message_id: 'second', block_index: tool.block_index }, tool_call_id: tool.call_id!, tool_id: tool.tool_id!, result: { status: { type: 'success' }, duration_ms: '1', content: [] } } } }));
const laterReply: RuntimeClientTranscriptEntry = { cursor: '122', turn_process: process, item: { type: 'message', message: { role: 'assistant', id: 'third-reply', content: [{ type: 'text', text: 'Later reply' }] } } };
const toolsDeferredReply = { ...toolsHydrated, transcript: { entries: [firstToolEntry, toolResultEntry, ...laterToolResults, laterReply], deferred_entries: [{ offset: '101', identity: 'second', value: toolsDescriptor, content: [], deferred: true }] } };
const toolsDurable = foldRuntimeEvent(toolsCanonical, { type: 'read_domains_updated', transcript: { entries: [...toolsLive.transcript.entries, secondToolEntry] }, trace: toolsCanonical.trace });
const toolsCompactions = compactedTools ? [{ head: { ...projectionHead('Derived summary'), conversation_id: process.conversation_id, activation_id: '1', transcript_anchor: '100' }, content: <p data-tool-compaction>Compaction boundary</p> }] : [];
const toolPresentation = Symbol('native process display');
const composerPage = server.handlers.get('conversation/checkpointPage')!;
if (descriptorScene) server.handlers.set('conversation/checkpointPage', request => {
  if (request.method !== 'conversation/checkpointPage') throw Error('Expected checkpoint page');
  if (request.params.domain !== 'messages.message:second.blocks') return composerPage(request);
  const cut = server.client.getSnapshot().views.A.checkpoint!.read_cut;
  return { type: 'checkpoint_page', page: { domain: request.params.domain, read_cut: cut, cursor: cut.revision, total: String(secondTools.length), items: secondTools.map((tool, index) => ({ offset: String(index), identity: 'tool_call', value: { id: tool.call_id, tool_id: tool.tool_id, name: tool.name, arguments: tool.state.arguments }, content: [], deferred: false })), collections: [], next_offset: null } };
});
const completedWorkflow = workflowRun(); completedWorkflow.state = { type: 'settled', outcome: 'completed' }; completedWorkflow.instances = completedWorkflow.instances.slice(0, 3).map(instance => ({ ...instance, state: { type: 'settled', outcome: 'completed' } }));
const tallTask = task('Tall task'), shortTask = task('Short task', 'A', tallTask.binding.node_id, '2'); shortTask.next_due_utc = '2028-11-02T01:00:00Z';
const tasksFixture = schedulerFixture([tallTask, shortTask]); if (scene === 'tasks') await tasksFixture.server.connect();
const authority = new WorkspaceAuthority(server.workspaceHost); await authority.observe();
const previews = new FilePreviewCoordinator(server.client, 'A', server.workspaceHost, authority);
const previewSource: PreviewSource = { kind: 'artifact', id: 'stable-code' };
let previewHint: (() => void) | undefined, finishProbe: ((version: string) => void) | undefined, previewPages = 0;
const previewOwner = new PreviewContentOwner({ current: () => previews.sourceCurrent(previewSource), immutable: false,
  page: async offset => { previewPages++; return { text: 'const answer = 1;', version: 'v1', offset, lines: 1, eof: true }; },
  bytes: async () => { throw Error('Only text is expected'); }, probe: () => new Promise<string>(resolve => { finishProbe = resolve; }),
  document: async () => { throw Error('Only text is expected'); },
  observe: async (_signal, changed) => { previewHint = changed; return { version: 'v1', done: new Promise<void>(() => {}), close: async () => {} }; },
});
if (scene === 'preview') previewOwner.activate();
const previewLease = new FilePreviewLease(previews, 1, previewSource, previewOwner);
const codeText = '```ts\nconst answer = 42\n```', codeLive = { ...live, attempt: { ...live.attempt, in_flight: { message_id: 'code-message', blocks: [{ type: 'text' as const, block_index: 0, text: codeText }] } } };
const codeEntry: RuntimeClientTranscriptEntry = { cursor: '100', turn_process: process, item: { type: 'message', message: { role: 'assistant', id: 'code-message', content: [{ type: 'text', text: codeText }] } } };
function Continuity() {
  const [current, setView] = useState(view), [published, publish] = useState(false);
  const [frontier, setFrontier] = useState(reference), [rowCount, setRows] = useState(100);
  const [toolsStage, setToolsStage] = useState(0), [workflowHydrated, hydrateWorkflow] = useState(false);
  const [codeCompleted, completeCode] = useState(false);
  const rows: TrajectoryLedgerRow[] = Array.from({ length: rowCount }, (_, index) => ({ display_key: 'row-' + index, kind: 'summary', height: 30, turnStart: false, callsOwner: 'message-' + index, summary: 'Record ' + index }));
  const content = useComposerContent(server.client, current);
  useEffect(() => {
    Object.assign(window, { continuity: {
      publish: () => publish(true),
      advancePrefix: () => setFrontier({ ...reference, content_revision: 2, readable_end: prefix.length + suffix.length }),
      finishPrefix: () => finishPrefix?.(),
      setRows,
      publishTools: () => setToolsStage(descriptorScene ? 3 : 2), commitTools: () => { if (descriptorScene) server.held.add('conversation/checkpointPage'); setToolsStage(1); },
      hydrateTools: () => { setToolsStage(2); server.held.delete('conversation/checkpointPage'); for (const row of server.requests) if (row.socket === server.socket && row.request.method === 'conversation/checkpointPage' && row.request.params.domain === 'messages.message:second.blocks') server.reply(row.request); },
      publishDeferredReply: () => setToolsStage(4),
      toolState: () => ({ live: !!toolsCanonical.attempt?.in_flight, messages: toolsCanonical.messages.length, transcript: toolsCanonical.transcript.entries?.length ?? 0, blockReads: server.requests.filter(row => row.request.method === 'conversation/checkpointPage' && row.request.params.domain === 'messages.message:second.blocks').length }),
      hydrateWorkflow: () => hydrateWorkflow(true),
      reorderTasks: () => { tasksFixture.rows.set(tallTask.id.ordinal, { ...tallTask, revision: String(BigInt(tallTask.revision) + 1n), next_due_utc: '2028-11-03T01:00:00Z' }); tasksFixture.notify(); },
      previewState: () => ({ loading: previewOwner.getSnapshot().loading, watched: previewHint !== undefined, pages: previewPages }),
      probePreview: () => previewHint?.(), finishPreview: () => finishProbe?.('v1'),
      completeCode: () => completeCode(true),
      disconnectReading: () => server.socket.close(), reconnectReading: () => server.connect(),
      holdReadingCheckpoint: () => { server.held.add('session/checkpoint'); server.loaded.delete('A'); },
      finishReadingCheckpoint: () => { const request = server.requests.filter(row => row.socket === server.socket && row.request.method === 'session/checkpoint' && row.request.params.target.session_id === 'A').at(-1)?.request; if (!request) throw Error('No pending reading checkpoint'); server.held.delete('session/checkpoint'); server.reply(request); },
      readingState: () => { const view = server.client.getSnapshot().views.A; return { connection: server.client.getSnapshot().connection, ready: server.client.isAttachmentControlCurrent('A', view?.attachmentObservation), historyReady: !!view?.history && !view.history.initialLoading && !view.history.loading, checkpointPending: view?.observationState === 'checkpointLoading' && !view.snapshot, incarnation: view?.target?.runtime_incarnation }; },
      deferredState: () => ({ reads: server.requests.filter(row => row.request.method === 'conversation/contentRead').length }),
      holdDeferredRead: () => server.held.add('conversation/contentRead'),
      finishDeferredRead: () => { const request = server.requests.filter(row => row.socket === server.socket && row.request.method === 'conversation/contentRead').at(-1)?.request; if (!request) throw Error('No pending content read'); server.held.delete('conversation/contentRead'); server.reply(request); },
      replaceReadingAuthority: async () => { server.authorityId = 'replacement-server'; server.socket.close(); await server.connect(); await server.client.attach('A'); },
      otherReading: async () => { server.nodeSnapshots.set('node-other', readingSnapshot('B')); await server.client.switchNode('A', 'node-other'); await server.client.attach('A', 'node-other'); },
      holdFirst: () => server.held.add('turn/start'),
      attachFirst: () => { server.held.delete('session/attach'); for (const row of server.requests) if (row.request.method === 'session/attach' && row.request.params.session_id === 'B') server.reply(row.request); },
      ackFirst: () => { const request = server.requests.find(row => row.request.method === 'turn/start')?.request; if (!request) throw Error('No first input'); server.reply(request); },
      firstState: () => ({ phase: firstSubmissions(server.client).session('B')?.phase, fileReferences: firstSubmissions(server.client).session('B')?.draft.files.length, pending: server.client.getSnapshot().views.B?.submissions?.length ?? 0, requests: server.requests.filter(row => row.request.method === 'turn/start').length }),
      observeFirst: async () => { const message = { role: 'user' as const, source: 'human' as const, id: 'accepted-user', content: [{ type: 'text' as const, text: 'First message stays visible' }] }; server.snapshots.set('B', { ...snapshot('B'), messages: [message], transcript: { entries: [{ cursor: '1', item: { type: 'message', message } }] } }); await server.client.refresh('B'); },
      changeGoal: () => {
        server.held.add('conversation/checkpointPage'); server.held.add('conversation/checkpointValueRead');
        const goal: RuntimeClientGoalSnapshot = { reference: { id: 'goal-1', revision: '4' }, objective: 'Independent Goal update', objective_deferred: false, phase: 'blocked', blocked_reason: 'preview', blocked_reason_deferred: true, autonomous_round_budget: 4, autonomous_rounds_consumed: 1, origin: { kind: 'runtime_control' }, last_round_message_id_deferred: false, fields: [] };
        setView({ ...view, snapshot: { ...view.snapshot!, goal: { current: goal } } });
      },
      reads: () => server.requests.filter(row => row.request.method === 'conversation/checkpointPage').length,
    } });
  }, []);
  if (scene === 'code-complete') return <main style={{ width: 740, margin: '24px auto' }}><AgentTranscript snapshot={codeCompleted ? { ...codeLive, transcript: { entries: [codeEntry] } } : codeLive}/></main>;
  if (scene === 'reading' || deferredReading) return <main style={{ width: 740, height: 420, overflow: 'auto', margin: '24px auto' }} data-conversation-scroll><ConversationLive client={server.client} sessionId="A" mode="chat" disabled={false} onHistorical={() => {}}/></main>;
  if (scene === 'first') return <App client={server.client} workspaceHost={server.workspaceHost}/>;
  if (descriptorScene) return <main style={{ width: 740, margin: '24px auto' }}><JournalSourceProvider presentationOwner={toolPresentation} source={{ client: server.client, target: server.client.target('A'), cut: () => server.client.getSnapshot().views.A.checkpoint?.read_cut, owned: () => server.client.isAttachmentObservationCurrent('A', server.client.getSnapshot().views.A.attachmentObservation) }}><AgentTranscript snapshot={toolsStage === 4 ? toolsDeferredReply : toolsStage === 3 ? toolsDurable : toolsStage === 2 ? toolsHydrated : toolsStage === 1 ? toolsCanonical : toolsLive} compactions={toolsCompactions}/></JournalSourceProvider></main>;
  if (scene === 'tools' || scene === 'tools-compaction') return <main style={{ width: 740, margin: '24px auto' }}><AgentTranscript snapshot={toolsStage === 2 ? toolsDurable : toolsStage === 1 ? toolsCanonical : toolsLive} compactions={toolsCompactions}/></main>;
  if (scene === 'workflow') return <main style={{ width: 740, margin: '24px auto' }}><WorkflowRunPanel run={{ ...completedWorkflow, instances: workflowHydrated ? completedWorkflow.instances : completedWorkflow.instances.slice(0, 2) }}/></main>;
  if (scene === 'tasks') return <main style={{ width: 740, height: 700, margin: '24px auto' }}><TaskManagerPage client={tasksFixture.server.client} newTask={() => {}} openSession={() => {}}/></main>;
  if (scene === 'preview') return <main style={{ width: 740, height: 500, margin: '24px auto' }}><ArtifactPreview artifact={{ source: previewSource, name: 'example.ts', image: false }} resources={previewLease} viewState={{ viewer: 'code' }} onViewStateChange={() => {}} onDownload={() => {}}/></main>;
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
