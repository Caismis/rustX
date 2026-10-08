import { createRoot } from 'react-dom/client';
import { App } from '../../src/app/App';
import { Server, snapshot } from '../fixture';
import { cfg3Source, cfg3Effective } from '../cfg3-data';
import { traceRecord } from '../trace-fixture';
import { RpcFailure } from '../../src/client/app-server';
import type { Request } from '../../../protocol/app-server/v37';
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
server.workspaceHost.resolveWorkspace = async () => ({ cwd: '/workspace/A' });
server.workspaceHost.classifyLocations = async paths => paths.map(() => ({ authorized: true, workspaceId: 'workspace-a' }));
const capabilities = { inputModalities: ['text' as const], outputModalities: ['text' as const], toolCalls: true, reasoning: false };
server.workspaceHost.configureWorkspace = async () => ({ kind: 'read', projection: { ...cfg3Source(), session_models: { kind: 'available', default_model: { model: 'fixture/native' }, catalog: { models: ['fixture/native', 'fixture/second'].map(model => ({ model, protocol: 'openai_responses' as const, contextWindow: 8192, maxOutputTokens: 1024, credentialSource: { type: 'environment', variable: 'KEY' }, declaredCapabilities: capabilities, effectiveCapabilities: capabilities, reasoningProfiles: [] })) } }, prospective_approval_mode: 'policy', target: { kind: 'workspace', directory: '/workspace/A' } } });
if (new URL(location.href).searchParams.has('models')) {
  const saved = server.snapshots.get('A')!;
  saved.model = cfg3Effective().effective_model;
  saved.transcript.entries = Array.from({ length: 20 }, (_, index) => ({ cursor: String(index + 1), item: { type: 'message' as const, message: { id: `saved-${index}`, role: index % 2 ? 'assistant' as const : 'user' as const, source: 'human' as const, content: [{ type: 'text' as const, text: `Saved message ${index}: ` + 'Previously saved conversation content. '.repeat(12) }] } } }));
  server.handlers.set('session/models', () => ({ type: 'models', catalog: { models: [{ model: saved.model!.configured.model, protocol: 'openai_responses' as const, contextWindow: 8192, maxOutputTokens: 1024, credentialSource: { type: 'literal' }, declaredCapabilities: capabilities, effectiveCapabilities: capabilities, reasoningProfiles: [] }] } }));
  server.handlers.set('session/model', () => ({ type: 'model', model: saved.model! }));
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
