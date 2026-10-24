import{traceReceipt}from'../fixture';
import { domainReceipt } from '../fixture';
import { createActor } from 'xstate';
import { shellNavigationMachine } from '../../src/app/navigation/machine';
import { settingsNavigationMachine, createOwnerLookup } from '../../src/app/settings/machines/navigation';
import { WorkspaceAuthority } from '../../src/workspaces/authority';
import { WorkspaceSessionAdmission } from '../../src/workspaces/session-admission';
import { ConnectionController } from '../../src/connection/controller';
import { childNavigation } from './child-navigation';
import { childMessageContinuation } from './child-message-continuation';
import { childObservationRecovery } from './child-observation-recovery';
import { childParentMessages } from './child-parent-messages';
import { messageAttachments } from './message-attachments';
import { liveTrace } from './trace-live';
import { agentMetrics } from '../agent-statistics-fixture';
import { createRoot } from 'react-dom/client';
import { ComposerDraft } from '../../src/app/composer/draft';
import { AppSurface } from '../../src/app/App';
import { workflowSurfaceMachine } from '../../src/app/workflows/machines/surface';
import { createWorkflowPort } from '../../src/app/workflows/port';
import { inspectionAcknowledger, RepairAcknowledgements } from '../../src/app/repair-notices';
import { Server, snapshot, childConversation } from '../fixture';
import { cfg3Source, cfg3Effective } from '../cfg3-data';
import { traceRecord, traceTool, requestDetail, toolDetail } from '../trace-fixture';
import { RpcFailure } from '../../src/client/errors';
import type { CheckpointReadCut, Request, MessageBlock, GoalSnapshot, RuntimeClientTranscriptEntry } from '../../../protocol/app-server/v57';
import '../../src/presentation/theme/base.css';
import '../../src/presentation/theme/gradient-shadow-text.css';
import '../../src/presentation/theme/design-platform.css';
import '../../src/presentation/theme/shiki.css';
import '../../src/presentation/theme/reset.css';
import '../../src/app/console.css';
import { firstSubmissions } from '../../src/app/new-conversation/first-submit';

// Only the transport is controlled. App, navigation, first-submit owner, client,
// Product Host admission and all presentation are the production implementations.
const server = new Server();
if (!new URL(location.href).searchParams.has('existing')) server.snapshots.clear();
else server.snapshots.get('A')!.transcript.entries = [{ cursor: '1', item: { type: 'message', message: { id: 'saved-user', role: 'user', source: 'human', content: [{ type: 'text', text: 'Previously saved message' }] } } }];
for (const saved of server.snapshots.values()) saved.model = cfg3Effective().effective_model;
server.handlers.set('session/model', request => {
  if (request.method !== 'session/model') throw new Error('Wrong operation');
  const model = server.snapshots.get(request.params.target.session_id)?.model;
  if (!model) throw new Error('Missing native fixture model');
  return domainReceipt('model',[model]);
});
server.workspaceHost.resolveWorkspace = async () => ({ cwd: '/workspace/A' });
server.workspaceHost.classifyLocations = async paths => paths.map(() => ({ authorized: true, workspaceId: 'workspace-a' }));
const capabilities = { inputModalities: ['text' as const], outputModalities: ['text' as const], toolCalls: true, reasoning: false, jsonSchemaOutput: false };
server.workspaceHost.configureWorkspace = async () => ({ kind: 'read', projection: { ...cfg3Source(), session_models: { kind: 'available', default_model: { model: 'fixture/native' }, catalog: { models: ['fixture/native', 'fixture/second'].map(model => ({ model, protocol: 'openai_responses' as const, contextWindow: 8192, maxOutputTokens: 1024, credentialSource: { type: 'environment', variable: 'KEY' }, declaredCapabilities: capabilities, effectiveCapabilities: capabilities, profiles: [] })) } }, prospective_approval_mode: 'policy', target: { kind: 'workspace', directory: '/workspace/A' } } });
if (new URL(location.href).searchParams.has('models')) {
  const saved = server.snapshots.get('A')!;
  saved.model = cfg3Effective().effective_model;
  saved.transcript.entries = Array.from({ length: 20 }, (_, index) => ({ cursor: String(index + 1), item: { type: 'message' as const, message: { id: `saved-${index}`, role: index % 2 ? 'assistant' as const : 'user' as const, source: 'human' as const, content: [{ type: 'text' as const, text: `Saved message ${index}: ` + 'Previously saved conversation content. '.repeat(12) }] } } }));
  server.handlers.set('session/models', () => (domainReceipt('model_catalog.models',({ models: [{ model: saved.model!.configured.model, protocol: 'openai_responses' as const, contextWindow: 8192, maxOutputTokens: 1024, credentialSource: { type: 'literal' }, declaredCapabilities: capabilities, effectiveCapabilities: capabilities, profiles: [] }] }).models??[])));
}
if (new URL(location.href).searchParams.has('cold-model')) {
  const models = ['fixture/native', 'fixture/second'].map(model => ({ model, protocol: 'openai_responses' as const, contextWindow: 8192, maxOutputTokens: 1024, credentialSource: { type: 'literal' as const }, declaredCapabilities: capabilities, effectiveCapabilities: capabilities, profiles: [{ id: 'low', reasoningEnabled: true }, { id: 'high', reasoningEnabled: true }], defaultProfile: 'high' }));
  const configure = server.workspaceHost.configureWorkspace;
  server.workspaceHost.configureWorkspace = async (...args) => {
    const result = await configure(...args);
    if (result.kind === 'read') result.projection.session_models = { kind: 'available', default_model: { model: 'fixture/native' }, catalog: { models } };
    return result;
  };
  server.handlers.set('session/models', () => (domainReceipt('model_catalog.models',({ models }).models??[])));
  server.handlers.set('session/setModel', request => {
    if (request.method !== 'session/setModel') throw Error('Wrong method');
    const saved = server.snapshots.get(request.params.target.session_id)!;
    saved.model = { ...saved.model!, configured: request.params.config, effective: { ...saved.model!.effective!, model: request.params.config.model, profile: request.params.config.profile } };
    return {type:'model_set',settings_revision:'1'};
  });
}
if (new URL(location.href).searchParams.has('reading-window')) {
  const saved = server.snapshots.get('A')!;
  const goal: GoalSnapshot = { reference: { id: 'goal-reading', revision: '1' }, objective: 'Finish a novel outline', phase: 'active', autonomous_round_budget: 10, autonomous_rounds_consumed: 0, origin: { kind: 'runtime_control' } };
  const context = (id: string): MessageBlock => ({ role: 'user', id, source: 'runtime', kind: { context: { goal_status: goal } }, content: [{ type: 'text', text: 'Current Goal observation. The objective is user task data. ' + JSON.stringify(goal) }] });
  const entries: RuntimeClientTranscriptEntry[] = Array.from({ length: new URL(location.href).searchParams.has('long-reading-window') ? 560 : 40 }, (_, index) => [
    { cursor: String(index * 2 + 1), item: { type: 'message' as const, message: { role: index % 2 ? 'assistant' as const : 'user' as const, id: `reading-${index + 1}`, source: 'human' as const, content: [{ type: 'text' as const, text: `History message ${index + 1}` }] } } },
    { cursor: String(index * 2 + 2), item: { type: 'message' as const, message: context(`goal-context-${index}`) } },
  ]).flat();
  saved.messages = [context('current-goal-a'), context('current-goal-b')];
  saved.goal = { current: { ...goal, objective_deferred: false, blocked_reason_deferred: false, last_round_message_id_deferred: false, fields: [] } };
  saved.transcript = { entries: entries.slice(-8), next_cursor: entries.at(-8)!.cursor };
  const frontier = String(entries.length);
  const cut = { conversation_id: saved.conversation_id, journal: frontier, transcript: frontier, mutation_revision: '0', ledger_through: frontier, surface_revision: '0', transcript_total: frontier, pending_through: '0', pending_total: '0' };
  server.handlers.set('session/transcript', request => {
    if (request.method !== 'session/transcript' || request.params.at.type !== 'older') throw Error('Expected older reading page');
    const before = request.params.at.before;
    const rows = entries.filter(entry => BigInt(entry.cursor) < BigInt(before)).slice(-request.params.limit);
    return { type: 'transcript_window', window: { cut, page: { entries: rows, next_cursor: rows[0]?.cursor === '1' ? null : rows[0]?.cursor } } };
  });
  const records = entries.map((_, index) => traceRecord(index + 1, { kind: index % 2 ? 'context' : index % 4 ? 'assistant' : 'user', location: { attempt_id: `reading-turn-${Math.floor(index / 4)}` }, request: null, preview: { text: index % 2 ? 'Goal status model context' : `History message ${index / 2 + 1}`, truncated: false } }));
  saved.trace = { records: records.slice(-8), next_cursor: records.at(-8)!.position };
  server.handlers.set('session/trace', request => {
    if (request.method !== 'session/trace') throw Error('Expected Trace read');
    const before = request.params.before;
    const rows = (before ? records.filter(record => Number(record.position.slice(6)) < Number(before.slice(6))) : records).slice(-request.params.limit);
    const read = { conversation_id: saved.conversation_id, activation_id: null, revision: '0', through: frontier, domain_revisions: {}, collection_cuts: {}, source_cut: { read: cut, ledger_through: cut.ledger_through, surface_revision: cut.surface_revision, transcript_total: cut.transcript_total, inherited_through: '0' } };
    return { type: 'trace', page: { read_cut: read, records: rows, next_cursor: rows[0]?.id === 'trace:1' ? null : rows[0]?.position } };
  });
}
let advanceHistory: (() => Promise<void>) | undefined;
if (new URL(location.href).searchParams.has('history-refresh')) {
  const saved = server.snapshots.get('A')!;
  const question: RuntimeClientTranscriptEntry = { cursor: '1', item: { type: 'message', message: { id: 'repair-question', role: 'user', source: 'human', content: [{ type: 'text', text: 'Run a background task and keep my conversation visible.' }] } } };
  const tool: RuntimeClientTranscriptEntry = { cursor: '2', response_pending: true, item: { type: 'message', message: { id: 'repair-call', role: 'assistant', content: [{ type: 'tool_call', id: 'repair-bash', tool_id: 'tool-bash', name: 'bash', arguments: { command: 'printf repaired-result' } }] } }, tool_calls: [{ committed_result_deferred: false, occurrence: { locator: { type: 'by_message_id', message_id: 'repair-call' }, block_index: 0 }, assembly_complete: true, message_id: 'repair-call', block_index: 0, call_id: 'repair-bash', tool_id: 'tool-bash', name: 'bash', state: { type: 'assembled', arguments: '{"command":"printf repaired-result"}' } }] };
  const answer: RuntimeClientTranscriptEntry = { cursor: '3', item: { type: 'message', message: { id: 'repair-answer', role: 'assistant', content: [{ type: 'text', text: 'Your conversation stays available.\n\n' + Array.from({ length: 30 }, (_, n) => `Reading paragraph ${n + 1}: earlier content stays in place during native completion.`).join('\n\n') }] } } };
  saved.messages = [];
  saved.transcript = { entries: [question, tool, answer], next_cursor: null };
  server.handlers.set('session/transcript', request => {
    if (request.method !== 'session/transcript' || request.params.at.type !== 'message' || request.params.at.id !== 'repair-call') throw Error('Expected exact old message refresh');
    const settled = structuredClone(tool); settled.response_pending = false;
    settled.tool_calls![0].state = { type: 'settled', arguments: '{"command":"printf repaired-result"}', result: { status: { type: 'success' }, duration_ms: '30', content: [{ type: 'text', text: 'repaired-result' }] } };
    return { type: 'transcript_window', window: { cut: { conversation_id: saved.conversation_id, journal: '4', transcript: '4', mutation_revision: '0', ledger_through: '4', surface_revision: '0', transcript_total: '4', pending_through: '0', pending_total: '0' }, page: { entries: [settled] }, target_cursor: '2' } };
  });
  advanceHistory = async () => {
    server.held.add('session/transcript');
    const next = structuredClone(server.snapshots.get('A')!);
    next.transcript = { entries: [answer, { cursor: '4', item: { type: 'message', message: { id: 'repair-final', role: 'assistant', content: [{ type: 'text', text: 'The native task has finished.' }] } } }], next_cursor: '3' };
    await server.update('A', next);
  };
}
if (new URL(location.href).searchParams.has('trajectory')) {
  server.snapshots.get('A')!.trace.records = Array.from({ length: 160 }, (_, index) => traceRecord(index, { kind: 'user', request: null, location: {}, preview: { text: `Saved trace ${index}`, truncated: false } }));
}
if (new URL(location.href).searchParams.has('subagents')) {
  const saved = server.snapshots.get('A')!;
  saved.agents = ['Research sources', 'Verify findings', 'Write report'].map((agent, index) => ({ title: agent, agent, agent_id: `child-${index}`, parent_agent_id: 'root', child_conversation_id: `child-conversation-${index}`, activation_id: `activation-${index}`, current_activation: index === 0 ? 'activation-0' : null, state: index === 0 ? 'active' : 'inactive', activation_state: index === 0 ? 'running' : 'succeeded', definition_digest: 'definition', profile_digest: 'profile', started_at: '2026-10-08T08:00:00Z', observation: { attempt_id: null, revision: '1', activity: { type: 'awaiting_activity' }, counters: { model_requests: 3, model_retries: 0, tool_executions: 2 } }, workspace: '/workspace/A' }));
  saved.transcript.entries!.push({ cursor: '21', item: { type: 'message', message: { role: 'user', id: 'agent-report', source: { agent: { agent_id: 'child-1' } }, content: [{ type: 'text', text: 'Verified report: the original sources agree.\n\n**Evidence**\n\n- Source one\n- Source two' }] } } });
  if (new URL(location.href).searchParams.has('subagent-details')) {
    const task = 'Inspect the original sources, verify the claims and return a concise report with evidence.';
    const invocations = [
      { name: 'subagent', arguments: { agent: 'explore', title: 'Verify findings', task }, result: { agent_id: 'child-1', activation_id: 'activation-1', state: 'active', agent: 'Verify findings' } },
      { name: 'list_agents', arguments: {}, result: { returned: saved.agents.length, matched: saved.agents.length, truncated: false, limit: 64, agents: saved.agents.map(agent => ({ agent_id: agent.agent_id, title: agent.title, agent: agent.agent, state: agent.state })) } },
      { name: 'wait_agent', arguments: { agent_id: 'child-1' }, result: { agent_id: 'child-1', activation_id: 'activation-1', outcome: 'succeeded' } },
    ];
    saved.transcript.entries!.push({ cursor: '22', item: { type: 'message', message: { role: 'assistant', id: 'delegation', content: [
      { type: 'text', text: 'I will delegate the source verification and review the evidence when it returns.' },
      ...invocations.map((invocation, index) => ({ type: 'tool_call' as const, id: `agent-call-${index}`, tool_id: `tool-${invocation.name}`, name: invocation.name, arguments: invocation.arguments })),
    ] } }, tool_calls: invocations.map((invocation, index) => ({committed_result_deferred:false,occurrence:{locator:{type:'by_message_id' as const,message_id:'delegation'},block_index:index + 1},assembly_complete:true, message_id: 'delegation', block_index: index + 1, call_id: `agent-call-${index}`, tool_id: `tool-${invocation.name}`, name: invocation.name, state: { type: 'settled', arguments: JSON.stringify(invocation.arguments), result: { status: { type: 'success' }, content: [{ type: 'json', value: invocation.result }], duration_ms: '100' } } })) });
  }
  server.handlers.set('agent/statistics', () => ({ type: 'agent_statistics', metrics: agentMetrics }));
  if (new URL(location.href).searchParams.has('nested-subagents')) saved.agents.push({ ...saved.agents[1]!, title: 'Verify original documents', agent: 'Verify original documents', agent_id: 'grandchild', parent_agent_id: 'child-1', child_conversation_id: 'grandchild-conversation' });
  server.handlers.set('agent/sendMessage', request => {
    if (request.method !== 'agent/sendMessage') throw Error('Wrong method');
    return { type: 'agent_message', agent_id: request.params.agent_id, activation_id: 'resumed-activation', resumed: true };
  });
  const childTraceCut = (agent_id: string): CheckpointReadCut => {
    const agent = saved.agents!.find(agent => agent.agent_id === agent_id)!;
    const read = { conversation_id: agent.child_conversation_id, journal: '70', transcript: '0', mutation_revision: '0', ledger_through: '0', surface_revision: '0', transcript_total: '0', pending_through: '0', pending_total: '0' };
    return { conversation_id: agent.child_conversation_id, activation_id: agent.activation_id, revision: '0', through: read.journal, domain_revisions: {}, collection_cuts: {}, source_cut: { read, ledger_through: read.ledger_through, surface_revision: read.surface_revision, transcript_total: read.transcript_total, inherited_through: '0' } };
  };
  server.handlers.set('agent/trace', request => {
    if (request.method !== 'agent/trace') throw Error('Wrong method');
    const { agent_id, before, limit } = request.params;
    const records = Array.from({ length: 70 }, (_, n) => n % 3 === 0
      ? traceTool(n + 1, { preview: { text: `${agent_id} tool ${n + 1}`, truncated: false } })
      : traceRecord(n + 1, { preview: { text: `${agent_id} request ${n + 1}`, truncated: false } }));
    const end = before ? records.findIndex(record => record.position === before) : records.length;
    const start = Math.max(0, end - limit);
    return { type: 'trace', page: { read_cut: childTraceCut(agent_id), records: records.slice(start, end), next_cursor: start > 0 ? records[start]!.position : null } };
  });
  server.handlers.set('agent/traceDetail', request => {
    if (request.method !== 'agent/traceDetail') throw Error('Wrong method');
    const n = Number(request.params.record_id.slice(6));
    const result = traceReceipt(((n - 1) % 3 === 0 ? toolDetail : requestDetail)(n, { messages: [{ message_id: `${request.params.agent_id}-evidence`, role: 'assistant', blocks: [{ type: 'text', text: { text: `${request.params.agent_id} independent evidence`, truncated: false } }], truncated: false }] }));
    result.page.domain = `trace.records.${n}.detail`;
    result.page.read_cut = childTraceCut(request.params.agent_id);
    result.page.cursor = result.page.read_cut.revision;
    return result;
  });
  server.handlers.set('agent/observe', request => {
    if (request.method !== 'agent/observe') throw Error('Wrong method');
    return childConversation({ entries: [{ cursor: '1', item: { type: 'message', message: { role: 'assistant', id: `${request.params.agent_id}-reply`, content: [{ type: 'text', text: `# Child research report\n\nSelected agent: ${request.params.agent_id}\n\nSources have been checked.\n\n` + 'Detailed findings and supporting evidence. '.repeat(120) }] } } }] }, request.params.agent_id, saved.agents!.find(agent => agent.agent_id === request.params.agent_id)!.activation_id, saved.agents!.find(agent => agent.agent_id === request.params.agent_id)!.child_conversation_id);
  });
}
const releaseChildObservation = new URL(location.href).searchParams.has('subagent-observation-failure') ? childObservationRecovery(server) : undefined;
if (new URL(location.href).searchParams.has('parent-messages')) childParentMessages(server);
if (new URL(location.href).searchParams.has('message-attachments')) messageAttachments(server);
if (new URL(location.href).searchParams.has('trace-live')) liveTrace(server);
if (new URL(location.href).searchParams.has('child-turn-navigation')) childNavigation(server);
if (new URL(location.href).searchParams.has('child-message-continuation')) childMessageContinuation(server);
let nativeDefault = 'fixture/native';
server.handlers.set('session/create', request => {
  if (request.method !== 'session/create') throw Error('Wrong method');
  const model = request.params.settings.model ?? { model: nativeDefault };
  server.snapshots.set('created', { ...snapshot('created'), model: { ...cfg3Effective().effective_model!, configured: model, effective: { ...cfg3Effective().effective_model!.effective, model: model.model } } as NonNullable<ReturnType<typeof snapshot>['model']> });
  server.invalidateSummary('created', server.socket, true);
  return { type: 'session_transition', session: { id: 'created', active_node: 'node-created', active_conversation_id: 'conversation-created', node_count: 1, created_at: '0', updated_at: '0' } };
});
await server.connect();
for (const method of ['session/create', 'session/attach', 'session/list', 'session/summary', 'turn/start'] as const) server.held.add(method);
const workspaceAuthority = new WorkspaceAuthority(server.workspaceHost);
server.client.setSessionAdmission(new WorkspaceSessionAdmission(workspaceAuthority, server.client).admit);
const repairAcknowledgements = new RepairAcknowledgements();
const settings = createActor(settingsNavigationMachine, { input: { lookup: createOwnerLookup(server.workspaceHost, server.client), inspected: inspectionAcknowledger(server.client, repairAcknowledgements) } }).start();
const navigation = createActor(shellNavigationMachine, { input: { client: server.client, settings } }).start();
const connection = new ConnectionController(server.client);
let root = createRoot(document.getElementById('root')!);
const composerDraft = new ComposerDraft();
const workflows = createActor(workflowSurfaceMachine, { input: { ports: scope => createWorkflowPort({ client: server.client, host: () => server.workspaceHost, hostAuthority: workspaceAuthority.capture(), scope }) } }).start();
const render = () => root.render(<AppSurface composerDraft={composerDraft} client={server.client} workspaceHost={server.workspaceHost} workspaceAuthority={workspaceAuthority} connection={connection} navigation={navigation} repairAcknowledgements={repairAcknowledgements} workflows={workflows}/>);
render();
const handled = new Set<Request>(server.requests.filter(row => row.request.method === 'initialize' || row.request.method === 'session/list').map(row => row.request));
(window as any).startupFixture = {
  releaseChildObservation: () => releaseChildObservation?.(),
  advanceHistory: () => advanceHistory?.(),
  history: () => server.client.getSnapshot().views.A?.history,
  requests: () => server.requests.map(({ request }) => request),
  async contextReading(input: number | null, running = false) {
    const next = { ...server.snapshots.get('A')! };
    next.context = { compaction_count: 0, compaction_in_progress: false, occupancy: input === null ? null : { estimated: false, model_deferred:false, model_source:null,
      input_tokens: String(input), context_window_tokens: "100000", model: 'measured/model',
      breakdown: { system_tokens: "1000", tool_tokens: "2000", message_tokens: String(input - 3000) },
    } };
    next.attempt = running ? { attempt_id: 'attempt-A', phase: { type: 'running' }, turn: 1, execution_settings: { resource_revision: '1', approval_mode: 'policy' } } : null;
    await server.update('A', next);
  },
  async appendSavedReply() {
    const entry = server.snapshots.get('A')!.transcript.entries!.at(-1)!;
    if (entry.item.type !== 'message' || entry.item.message.role !== 'assistant') throw Error('Expected saved reply');
    entry.item.message.content.push({ type: 'text', text: '\n\nLive continuation. ' + 'More response content. '.repeat(80) });
    await server.client.refresh('A');
  },
  defaultModel(model: string) { nativeDefault = model; },
  model: (id = 'created') => server.client.getSnapshot().views[id]?.snapshot?.model,
  observation: (id = 'created') => {
    const view = server.client.getSnapshot().views[id];
    return { target: view?.target, cursor: view?.checkpoint?.cursor, revision: view?.checkpoint?.read_cut.revision, state: view?.observationState };
  },
  operation: () => firstSubmissions(server.client).session('created'),
  async release(method: Request['method'], failure?: string) {
    const request = server.requests.find(row => row.request.method === method && !handled.has(row.request))?.request;
    if (!request) throw Error(`No pending ${method}`);
    handled.add(request);
    if (failure) server.handlers.set(method, () => { throw new RpcFailure({ code: -32000, message: failure }); });
    server.reply(request);
  },
  resumeCatalog() {
    server.held.delete('session/list');
    for (const { request } of server.requests) if (request.method === 'session/list' && !handled.has(request)) { handled.add(request); server.reply(request); }
  },
  hold(method: Request['method']) {
    // A new gate owns future requests, not earlier automatically replied ones.
    if (!server.held.has(method)) for (const { request } of server.requests) if (request.method === method) handled.add(request);
    server.held.add(method);
  },
  async lose(method: Request['method']) {
    const request = server.requests.find(row => row.request.method === method && !handled.has(row.request))?.request;
    if (!request) throw Error(`No pending ${method}`);
    handled.add(request); server.commit(request); server.socket.close();
  },
  invalidateCatalog() { server.handlers.delete('session/list'); server.invalidateSummary('created', server.socket, true); },
  allow(method: Request['method']) { server.held.delete(method); },
  remount() { root.unmount(); root = createRoot(document.getElementById('root')!); render(); },
  disconnect() { server.socket.close(); },
  async reconnect() { server.held.delete('session/list'); server.held.delete('session/attach'); server.held.delete('session/summary'); await server.connect(); },
};
