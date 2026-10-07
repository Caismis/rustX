import { createRoot } from 'react-dom/client';
import { traceRecord, requestDetail } from '../trace-fixture';
import { App } from '../../src/app/App';
import { HttpWorkspaceHost } from '../../src/workspaces/http-host';
import { Server, endpoint } from '../fixture';
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
await server.attached('A');
localStorage.setItem('rustx-console-view-v2', JSON.stringify({ endpoint, openViews: ['A'] }));
createRoot(document.getElementById('root')!).render(<App client={server.client} workspaceHost={server.workspaceHost}/>);
