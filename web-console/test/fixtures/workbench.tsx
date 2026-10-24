import { nativeTraceFixture } from '../native-trace-fixture';
import { checkpoint, domainReceipt, settingsReceipt } from '../fixture';
import { createRoot } from 'react-dom/client';
import { traceRecord, requestDetail } from '../trace-fixture';
import { App } from '../../src/app/App';
import { HttpWorkspaceHost } from '../../src/workspaces/http-host';
import { Server, endpoint } from '../fixture';
import { cfg3Source, cfg3Effective } from '../cfg3-data';
import { agentMetrics } from '../agent-statistics-fixture';
import '../../src/presentation/theme/base.css';
import '../../src/presentation/theme/design-platform.css';
import '../../src/presentation/theme/gradient-shadow-text.css';
import '../../src/presentation/theme/scrollbar.css';
import '../../src/presentation/theme/corner-shape.css';
import '../../src/presentation/theme/shiki.css';
import '../../src/presentation/theme/reset.css';
import '../../src/app/console.css';
const server = new Server(), http = new HttpWorkspaceHost('/product-host/workbench-fixture');
const catalog = await http.listWorkspaces();
server.workspaceHost.listWorkspaces = async () => ({ ...catalog, endpoint });
server.workspaceHost.workbench = (_scope, call, signal) => http.workbench(catalog, call, signal);
server.workspaceHost.observeWorkspaceFile = (_scope, call, changed, signal) => http.observeWorkspaceFile(catalog, call, changed, signal);
if (new URL(location.href).searchParams.has('file-links')) {
  const message = { role: 'assistant' as const, id: 'files', content: [{ type: 'text' as const, text: '[モルガン 解説](docs/モルガン_解説.md) · [Same file](./docs/モルガン_解説.md) · [Source line](lines.py#L80) · [Missing](missing.md)' }] };
  server.snapshots.get('A')!.messages = [message];
  const text = { text: message.content[0].text, truncated: false };
  const record = traceRecord(1, { kind: 'assistant', request: null, message_id: 'files', preview: text });
  const detail = requestDetail(1, { kind: 'assistant', request: null, messages: [{ role: 'assistant', message_id: 'files', source: 'runtime', blocks: [{ type: 'text', text }], truncated: false }] });
  const native = nativeTraceFixture(record, detail, checkpoint(server.snapshots.get('A')!).read_cut);
  server.snapshots.get('A')!.trace = { records: [record] };
  server.handlers.set('session/traceDetail', () => ({ type: 'domain_page', page: native.page }));
  server.handlers.set('conversation/checkpointPage', request => {
    if (request.method !== 'conversation/checkpointPage') throw Error('Unexpected page request');
    return { type: 'checkpoint_page', page: native.original.page(request.params.cut, request.params.domain, request.params.offset, () => true) };
  });
  server.handlers.set('conversation/contentRead', request => {
    if (request.method !== 'conversation/contentRead') throw Error('Unexpected content request');
    return { type: 'content_chunk', chunk: native.original.content(native.page.read_cut, request.params.content_ref, request.params.offset, () => true) };
  });
  server.snapshots.get('A')!.transcript = { entries: [{ cursor: '1', item: { type: 'message', message } }] };
}
if (new URL(location.href).searchParams.has('composer-layout')) {
  const workspace = catalog.workspaces[0]!;
  server.summaries.set('A', { cwd: workspace.location });
  server.workspaceHost.classifyLocations = async paths => paths.map(() => ({ authorized: true, workspaceId: workspace.id }));
  server.workspaceHost.configureWorkspace = async () => ({ kind: 'read', projection: { ...cfg3Source(), prospective_approval_mode: 'full_access', target: { kind: 'workspace', directory: workspace.location } } });
  server.handlers.set('session/settings', () => (settingsReceipt({ cwd: workspace.location },'0')));
  const saved = server.snapshots.get('A')!;
  const model = 'DeepSeek/deepseek-flash';
  saved.model = cfg3Effective().effective_model!;
  saved.model.configured = { model, profile: 'on' };
  saved.model.effective = { ...saved.model.effective!, model, profile: 'on' };
  saved.transcript.statistics = agentMetrics.statistics;
  saved.context = { compaction_count: 0, compaction_in_progress: false, occupancy: agentMetrics.occupancy };
  saved.transcript.entries = Array.from({ length: 20 }, (_, index) => ({ cursor: String(index + 1), item: { type: 'message' as const, message: { id: `saved-${index}`, role: index % 2 ? 'assistant' as const : 'user' as const, source: 'human' as const, content: [{ type: 'text' as const, text: `Saved message ${index}: ` + 'Previously saved conversation content. '.repeat(12) }] } } }));
  const capabilities = { inputModalities: ['text' as const], outputModalities: ['text' as const], toolCalls: true, reasoning: false, jsonSchemaOutput: false };
  const models = [model, 'Short'].map(model => ({ model, protocol: 'openai_responses' as const, contextWindow: 8192, maxOutputTokens: 1024, credentialSource: { type: 'literal' as const }, declaredCapabilities: capabilities, effectiveCapabilities: capabilities, profiles: [{ id: 'on', reasoningEnabled: false }], defaultProfile: 'on' }));
  server.handlers.set('session/model', () => (domainReceipt('model',[saved.model!])));
  server.handlers.set('session/models', () => (domainReceipt('model_catalog.models',({ models }).models??[])));
  server.handlers.set('session/setModel', request => {
    if (request.method !== 'session/setModel') throw Error('Wrong method');
    saved.model = { ...saved.model!, configured: request.params.config, effective: { ...saved.model!.effective!, model: request.params.config.model, profile: request.params.config.profile } };
    return {type:'model_set',settings_revision:'1'};
  });
}
if (new URL(location.href).searchParams.has('draft')) {
  server.workspaceHost.resolveWorkspace = async id => ({ cwd: catalog.workspaces.find(workspace => workspace.id === id)!.displayPath });
  server.workspaceHost.configureWorkspace = async id => ({ kind: 'read', projection: { ...cfg3Source(), prospective_approval_mode: 'full_access', target: { kind: 'workspace', directory: catalog.workspaces.find(workspace => workspace.id === id)!.displayPath } } });
  await server.connect();
  localStorage.setItem('rustx-console-view-v2', JSON.stringify({ endpoint, openViews: [] }));
} else {
  await server.attached('A');
  localStorage.setItem('rustx-console-view-v2', JSON.stringify({ endpoint, openViews: ['A'] }));
}
createRoot(document.getElementById('root')!).render(<App client={server.client} workspaceHost={server.workspaceHost}/>);
