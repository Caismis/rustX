import { childNavigation } from './child-navigation';
import { agentMetrics } from '../agent-statistics-fixture';
import { createRoot } from 'react-dom/client';
import { App } from '../../src/app/App';
import { Server, snapshot, childConversation } from '../fixture';
import { cfg3Source, cfg3Effective } from '../cfg3-data';
import { traceRecord, traceTool, requestDetail, toolDetail } from '../trace-fixture';
import { RpcFailure } from '../../src/client/app-server';
import type { Request } from '../../../protocol/app-server/v44';
import '../../src/presentation/theme/base.css';
import '../../src/presentation/theme/gradient-shadow-text.css';
import '../../src/presentation/theme/design-platform.css';
import '../../src/presentation/theme/shiki.css';
import '../../src/presentation/theme/reset.css';
import '../../src/app/console.css';

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
  return { type: 'model', model };
});
server.workspaceHost.resolveWorkspace = async () => ({ cwd: '/workspace/A' });
server.workspaceHost.classifyLocations = async paths => paths.map(() => ({ authorized: true, workspaceId: 'workspace-a' }));
const capabilities = { inputModalities: ['text' as const], outputModalities: ['text' as const], toolCalls: true, reasoning: false };
server.workspaceHost.configureWorkspace = async () => ({ kind: 'read', projection: { ...cfg3Source(), session_models: { kind: 'available', default_model: { model: 'fixture/native' }, catalog: { models: ['fixture/native', 'fixture/second'].map(model => ({ model, protocol: 'openai_responses' as const, contextWindow: 8192, maxOutputTokens: 1024, credentialSource: { type: 'environment', variable: 'KEY' }, declaredCapabilities: capabilities, effectiveCapabilities: capabilities, reasoningProfiles: [] })) } }, prospective_approval_mode: 'policy', target: { kind: 'workspace', directory: '/workspace/A' } } });
if (new URL(location.href).searchParams.has('models')) {
  const saved = server.snapshots.get('A')!;
  saved.model = cfg3Effective().effective_model;
  saved.transcript.entries = Array.from({ length: 20 }, (_, index) => ({ cursor: String(index + 1), item: { type: 'message' as const, message: { id: `saved-${index}`, role: index % 2 ? 'assistant' as const : 'user' as const, source: 'human' as const, content: [{ type: 'text' as const, text: `Saved message ${index}: ` + 'Previously saved conversation content. '.repeat(12) }] } } }));
  server.handlers.set('session/models', () => ({ type: 'models', catalog: { models: [{ model: saved.model!.configured.model, protocol: 'openai_responses' as const, contextWindow: 8192, maxOutputTokens: 1024, credentialSource: { type: 'literal' }, declaredCapabilities: capabilities, effectiveCapabilities: capabilities, reasoningProfiles: [] }] } }));
}
if (new URL(location.href).searchParams.has('cold-model')) {
  const models = ['fixture/native', 'fixture/second'].map(model => ({ model, protocol: 'openai_responses' as const, contextWindow: 8192, maxOutputTokens: 1024, credentialSource: { type: 'literal' as const }, declaredCapabilities: capabilities, effectiveCapabilities: capabilities, reasoningProfiles: [{ id: 'low', enabled: true }, { id: 'high', enabled: true }], defaultReasoningProfile: 'high' }));
  const configure = server.workspaceHost.configureWorkspace;
  server.workspaceHost.configureWorkspace = async (...args) => {
    const result = await configure(...args);
    if (result.kind === 'read') result.projection.session_models = { kind: 'available', default_model: { model: 'fixture/native' }, catalog: { models } };
    return result;
  };
  server.handlers.set('session/models', () => ({ type: 'models', catalog: { models } }));
  server.handlers.set('session/setModel', request => {
    if (request.method !== 'session/setModel') throw Error('Wrong method');
    const saved = server.snapshots.get(request.params.target.session_id)!;
    saved.model = { ...saved.model!, configured: request.params.config, effective: { ...saved.model!.effective, model: request.params.config.model, reasoningProfile: request.params.config.reasoningProfile } };
    return { type: 'model', model: saved.model };
  });
}
if (new URL(location.href).searchParams.has('trajectory')) {
  server.snapshots.get('A')!.trace.records = Array.from({ length: 160 }, (_, index) => traceRecord(index, { kind: 'user', request: null, location: {}, preview: { text: `Saved trace ${index}`, truncated: false } }));
}
if (new URL(location.href).searchParams.has('subagents')) {
  const saved = server.snapshots.get('A')!;
  saved.agents = ['Research sources', 'Verify findings', 'Write report'].map((agent, index) => ({ title: agent, agent, agent_id: `child-${index}`, parent_agent_id: 'root', child_conversation_id: `child-conversation-${index}`, activation_id: `activation-${index}`, current_activation: index === 0 ? 'activation-0' : null, state: index === 0 ? 'active' : 'inactive', activation_state: index === 0 ? 'running' : 'succeeded', definition_digest: 'definition', profile_digest: 'profile', started_at: '2026-10-08T08:00:00Z', observation: { attempt_id: null, revision: '1', activity: { type: 'awaiting_activity' }, counters: { model_requests: 3, model_retries: 0, tool_executions: 2 } }, workspace: { logical_workspace: '/workspace/A', isolation: { type: 'shared' }, resource_state: 'none' } }));
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
    ] } }, tool_calls: invocations.map((invocation, index) => ({ message_id: 'delegation', block_index: index + 1, call_id: `agent-call-${index}`, tool_id: `tool-${invocation.name}`, name: invocation.name, state: { type: 'settled', arguments: JSON.stringify(invocation.arguments), result: { status: { type: 'success' }, content: [{ type: 'json', value: invocation.result }], duration_ms: 100 } } })) });
  }
  server.handlers.set('agent/statistics', () => ({ type: 'agent_statistics', metrics: agentMetrics }));
  if (new URL(location.href).searchParams.has('nested-subagents')) saved.agents.push({ ...saved.agents[1]!, title: 'Verify original documents', agent: 'Verify original documents', agent_id: 'grandchild', parent_agent_id: 'child-1', child_conversation_id: 'grandchild-conversation' });
  server.handlers.set('agent/sendMessage', request => {
    if (request.method !== 'agent/sendMessage') throw Error('Wrong method');
    return { type: 'agent_message', agent_id: request.params.agent_id, activation_id: 'resumed-activation', resumed: true };
  });
  server.handlers.set('agent/trace', request => {
    if (request.method !== 'agent/trace') throw Error('Wrong method');
    const { agent_id, before, limit } = request.params;
    const records = Array.from({ length: 70 }, (_, n) => n % 3 === 0
      ? traceTool(n + 1, { preview: { text: `${agent_id} tool ${n + 1}`, truncated: false } })
      : traceRecord(n + 1, { preview: { text: `${agent_id} request ${n + 1}`, truncated: false } }));
    const end = before ? records.findIndex(record => record.position === before) : records.length;
    const start = Math.max(0, end - limit);
    return { type: 'trace', page: { records: records.slice(start, end), next_cursor: start > 0 ? records[start]!.position : null } };
  });
  server.handlers.set('agent/traceDetail', request => {
    if (request.method !== 'agent/traceDetail') throw Error('Wrong method');
    const n = Number(request.params.record_id.slice(6));
    return { type: 'trace_detail', detail: ((n - 1) % 3 === 0 ? toolDetail : requestDetail)(n, { messages: [{ message_id: `${request.params.agent_id}-evidence`, role: 'assistant', blocks: [{ type: 'text', text: { text: `${request.params.agent_id} independent evidence`, truncated: false } }], truncated: false }] }) };
  });
  server.handlers.set('agent/conversation', request => {
    if (request.method !== 'agent/conversation') throw Error('Wrong method');
    return childConversation({ entries: [{ cursor: '1', item: { type: 'message', message: { role: 'assistant', id: `${request.params.agent_id}-reply`, content: [{ type: 'text', text: `# Child research report\n\nSelected agent: ${request.params.agent_id}\n\nSources have been checked.\n\n` + 'Detailed findings and supporting evidence. '.repeat(120) }] } } }] }, request.params.agent_id, 'activation-a', saved.agents!.find(agent => agent.agent_id === request.params.agent_id)!.child_conversation_id);
  });
}
if (new URL(location.href).searchParams.has('child-turn-navigation')) childNavigation(server);
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
let root = createRoot(document.getElementById('root')!);
const render = () => root.render(<App client={server.client} workspaceHost={server.workspaceHost}/>);
render();
const handled = new Set<Request>(server.requests.filter(row => row.request.method === 'initialize' || row.request.method === 'session/list').map(row => row.request));
(window as any).startupFixture = {
  requests: () => server.requests.map(({ request }) => request),
  async contextReading(input: number | null, running = false) {
    const next = { ...server.snapshots.get('A')! };
    next.context = { compaction_count: 0, compaction_in_progress: false, occupancy: input === null ? null : { estimated: false,
      input_tokens: input, context_window_tokens: 100000, model: 'measured/model',
      breakdown: { system_tokens: 1000, tool_tokens: 2000, message_tokens: input - 3000 },
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
  operation: () => server.client.firstSubmissions.session('created'),
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
  hold(method: Request['method']) { server.held.add(method); },
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
