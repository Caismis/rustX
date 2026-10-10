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
if (new URL(location.href).searchParams.has('file-links')) {
  const message = { role: 'assistant' as const, id: 'files', content: [{ type: 'text' as const, text: '[モルガン 解説](docs/モルガン_解説.md) · [Same file](./docs/モルガン_解説.md) · [Source line](lines.py#L80) · [Missing](missing.md)' }] };
  server.snapshots.get('A')!.messages = [message];
  const text = { text: message.content[0].text, truncated: false };
  server.snapshots.get('A')!.trace = { records: [traceRecord(1, { kind: 'assistant', request: null, message_id: 'files', preview: text })] };
  server.traceDetails.set('trace:1', requestDetail(1, { kind: 'assistant', request: null, messages: [{ role: 'assistant', message_id: 'files', source: 'runtime', blocks: [{ type: 'text', text }], truncated: false }] }));
  server.snapshots.get('A')!.transcript = { entries: [{ cursor: '1', item: { type: 'message', message } }] };
}
if (new URL(location.href).searchParams.has('composer-layout')) {
  const workspace = catalog.workspaces[0]!;
  server.summaries.set('A', { cwd: workspace.location });
  server.workspaceHost.classifyLocations = async paths => paths.map(() => ({ authorized: true, workspaceId: workspace.id }));
  server.workspaceHost.configureWorkspace = async () => ({ kind: 'read', projection: { ...cfg3Source(), prospective_approval_mode: 'full_access', target: { kind: 'workspace', directory: workspace.location } } });
  server.handlers.set('session/settings', () => ({ type: 'settings', revision: '0', settings: { cwd: workspace.location } }));
  const saved = server.snapshots.get('A')!;
  const model = 'DeepSeek/deepseek-flash';
  saved.model = cfg3Effective().effective_model!;
  saved.model.configured = { model, reasoningProfile: 'on' };
  saved.model.effective = { ...saved.model.effective, model, reasoningProfile: 'on' };
  saved.transcript.statistics = agentMetrics.statistics;
  saved.context = { compaction_count: 0, compaction_in_progress: false, last_request_occupancy: agentMetrics.occupancy };
  saved.transcript.entries = Array.from({ length: 20 }, (_, index) => ({ cursor: String(index + 1), item: { type: 'message' as const, message: { id: `saved-${index}`, role: index % 2 ? 'assistant' as const : 'user' as const, source: 'human' as const, content: [{ type: 'text' as const, text: `Saved message ${index}: ` + 'Previously saved conversation content. '.repeat(12) }] } } }));
  const capabilities = { inputModalities: ['text' as const], outputModalities: ['text' as const], toolCalls: true, reasoning: false };
  const models = [model, 'Short'].map(model => ({ model, protocol: 'openai_responses' as const, contextWindow: 8192, maxOutputTokens: 1024, credentialSource: { type: 'literal' as const }, declaredCapabilities: capabilities, effectiveCapabilities: capabilities, reasoningProfiles: [{ id: 'on', enabled: true }], defaultReasoningProfile: 'on' }));
  server.handlers.set('session/model', () => ({ type: 'model', model: saved.model! }));
  server.handlers.set('session/models', () => ({ type: 'models', catalog: { models } }));
  server.handlers.set('session/setModel', request => {
    if (request.method !== 'session/setModel') throw Error('Wrong method');
    saved.model = { ...saved.model!, configured: request.params.config, effective: { ...saved.model!.effective, model: request.params.config.model, reasoningProfile: request.params.config.reasoningProfile } };
    return { type: 'model', model: saved.model! };
  });
}
await server.attached('A');
localStorage.setItem('rustx-console-view-v2', JSON.stringify({ endpoint, openViews: ['A'] }));
createRoot(document.getElementById('root')!).render(<App client={server.client} workspaceHost={server.workspaceHost}/>);
