import { useSyncExternalStore } from 'react';
import { createRoot } from 'react-dom/client';
import { Trajectory } from '../../src/app/trajectory/Trajectory';
import { Server, snapshot } from '../fixture';
import { traceRecord } from '../trace-fixture';
import { workflowTraceFixture } from '../workflow-trace-fixture';
import '../../src/presentation/theme/base.css';
import '../../src/presentation/theme/gradient-shadow-text.css';
import '../../src/presentation/theme/design-platform.css';
import '../../src/presentation/theme/reset.css';

// Exercise the actual cold-preview client path: Load Earlier creates a fresh
// field reader while prependTrace preserves the cache epoch and selected owner.
const server = new Server(), source = workflowTraceFixture(73), root = source.roots[0];
source.cut.conversation_id = 'conversation-A';
root.location = {}; root.originating_tool_call_id = null;
root.preview = { text: 'scheduled_flow', truncated: false };
for (const node of source.nodes.get(root.collections[0].domain)!) {
  node.instance.block.run.conversation_id = source.cut.conversation_id;
  node.instance.block.run.origin = { type: 'scheduled', start_id: 'scheduled-start' };
}
server.snapshots.set('A', { ...snapshot(), trace: { records: [root], read_cut: source.cut, next_cursor: 'trace:30' } });
server.held.add('thread/resume');
server.held.add('session/tracePage');
server.held.add('session/traceValueRead');
server.handlers.set('session/traceHistory', request => {
  if (request.method !== 'session/traceHistory') throw Error('Unexpected history method');
  return { type: 'session_trace_history', conversation_id: source.cut.conversation_id,
    page: { read_cut: source.cut, records: request.params.before
      ? [traceRecord(2, { kind: 'user', request: null, location: {}, preview: { text: 'Earlier cold history', truncated: false } })]
      : [root], next_cursor: request.params.before ? null : 'trace:30' } };
});
const waiting = new Map<number, (failed?: boolean) => void>();
let rounds = 0;
void (async () => {
  for (let count = 1; ; count++) {
    const request = await server.waitFor('session/tracePage', count);
    if (request.method !== 'session/tracePage') throw Error('Unexpected page method');
    const { cut, domain, offset } = request.params;
    if (!offset) rounds++;
    if (rounds > 1) {
      const failed = await new Promise<boolean | undefined>(resolve => waiting.set(offset, resolve));
      if (failed) {
        server.socket.deliver({ jsonrpc: '2.0', id: request.id,
          error: { code: -32000, message: 'Replacement Workflow page rejected', data: { kind: 'operation_failed' } } });
        continue;
      }
    }
    const page = await source.reader.page(cut, domain, offset, () => true);
    server.socket.deliver({ jsonrpc: '2.0', id: request.id, result: { type: 'checkpoint_page', page } });
  }
})();
void (async () => {
  for (let count = 1; ; count++) {
    const request = await server.waitFor('session/traceValueRead', count);
    if (request.method !== 'session/traceValueRead') throw Error('Unexpected value method');
    const { cut, domain, item_offset, path, offset } = request.params;
    const chunk = await source.reader.value(cut, { domain, item_offset, path }, offset, () => true);
    server.socket.deliver({ jsonrpc: '2.0', id: request.id, result: { type: 'checkpoint_value_chunk', chunk } });
  }
})();
window.workflowColdTrace = {
  waiting: () => [...waiting.keys()],
  epoch: () => server.client.getSnapshot().views.A.tracePreview?.cache.epoch,
  release: offset => {
    const resolve = waiting.get(offset);
    if (!resolve) throw Error('Cold Workflow page has not reached its gate');
    waiting.delete(offset); resolve();
  },
  reject: offset => {
    const resolve = waiting.get(offset);
    if (!resolve) throw Error('Cold Workflow page has not reached its gate');
    waiting.delete(offset); resolve(true);
  },
};
function Fixture() {
  const state = useSyncExternalStore(server.client.subscribe, server.client.getSnapshot);
  const preview = state.views.A?.tracePreview;
  return <main style={{ height: '100dvh', display: 'flex', flexDirection: 'column' }}>
    {preview && <Trajectory key="A" cache={preview.cache} onSelect={id => server.client.selectTrace('A', id)}
      onLoadDetail={id => { void server.client.loadTraceDetail('A', id); }}
      loadEarlier={() => { void server.client.loadEarlierTrace('A'); }} />}
  </main>;
}
createRoot(document.getElementById('root')!).render(<Fixture />);
await server.connect();
void server.client.attach('A');

declare global { interface Window { workflowColdTrace: { waiting(): number[]; epoch(): number | undefined; release(offset: number): void; reject(offset: number): void } } }
