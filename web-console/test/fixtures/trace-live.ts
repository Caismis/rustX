import type { Request } from '../../../protocol/app-server/v57';
import { checkpoint, type Server } from '../fixture';
import { nativeTraceFixture } from '../native-trace-fixture';
import { requestDetail, traceRecord } from '../trace-fixture';

/** Real App/Trace cache with controlled native events and deferred field reads. */
export function liveTrace(server: Server) {
  let revision = 1, record = traceRecord(4, { state: 'running' });
  const make = (model: string) => {
    const detail = requestDetail(4); detail.request!.model = model;
    detail.request!.options = Array.from({ length: 40 }, (_, n) => ({ name: `native_option_${n}`, value: { value: `Original frozen value ${n}`, truncated: false } }));
    return nativeTraceFixture(record, detail, { ...checkpoint(server.snapshots.get('A')!).read_cut, revision: String(revision) });
  };
  let native = make('ORIGINAL_MODEL');
  const sources = new Map([[native.page.read_cut.revision, native]]);
  const saved = server.snapshots.get('A')!;
  saved.trace = { records: [record], next_cursor: null };
  saved.attempt = { attempt_id: 'attempt-a', phase: { type: 'running' }, turn: 1, execution_settings: { resource_revision: '1', approval_mode: 'policy' } };
  server.handlers.set('session/traceDetail', () => ({ type: 'domain_page', page: native.page }));
  server.handlers.set('session/trace', () => ({ type: 'trace', page: { read_cut: native.page.read_cut, records: [record], next_cursor: null } }));
  server.handlers.set('conversation/checkpointPage', request => {
    if (request.method !== 'conversation/checkpointPage') throw Error('page');
    return { type: 'checkpoint_page', page: sources.get(request.params.cut.revision)!.original.page(request.params.cut, request.params.domain, request.params.offset, () => true) };
  });
  server.handlers.set('conversation/checkpointValueRead', request => {
    if (request.method !== 'conversation/checkpointValueRead') throw Error('value');
    return { type: 'checkpoint_value_chunk', chunk: sources.get(request.params.cut.revision)!.original.value(request.params.cut, request.params, request.params.offset, () => true) };
  });
  const publish = () => server.update('A', { ...server.snapshots.get('A')!, trace: { records: [record], next_cursor: null } });
  const pending = new Set<Request>();
  let older: typeof record | undefined;
  const hold = (method: Request['method']) => {
    for (const row of server.requests) if (row.request.method === method) pending.add(row.request);
    server.held.add(method);
  };
  window.traceLiveFixture = {
    holdSummary() {
      older = record; hold('session/trace');
      server.socket.deliver({ jsonrpc: '2.0', method: 'session/event', params: { target: server.target('A'), cursor: String(++server.cursor), event: { type: 'trace_changed' } } });
    },
    releaseSummary() {
      server.held.delete('session/trace');
      const request = server.requests.find(row => row.request.method === 'session/trace' && !pending.has(row.request))!.request;
      pending.add(request); server.socket.success(request, { type: 'trace', page: { read_cut: native.page.read_cut, records: [older!], next_cursor: null } });
    },
    async run(count: number) {
      for (let index = 0; index < count; index++) {
        record = { ...record, timing: { ...record.timing, duration_ms: String(++revision) } };
        await publish(); await new Promise(resolve => setTimeout(resolve, 16));
      }
    },
    async change() {
      hold('session/traceDetail');
      record = { ...record, preview: { text: 'Native content changed', truncated: false } };
      revision++; native = make('UPDATED_MODEL'); sources.set(native.page.read_cut.revision, native);
      await publish();
    },
    releaseHeader() {
      hold('conversation/checkpointValueRead');
      const request = server.requests.find(row => row.request.method === 'session/traceDetail' && !pending.has(row.request))!.request;
      pending.add(request); server.reply(request);
    },
    releaseContent() {
      server.held.delete('conversation/checkpointValueRead');
      for (const { request } of server.requests) if (request.method === 'conversation/checkpointValueRead' && !pending.has(request)) { pending.add(request); server.reply(request); }
    },
    reads: () => server.requests.filter(row => row.request.method === 'session/traceDetail').length,
    summaries: () => server.requests.filter(row => row.request.method === 'session/trace').length,
    values: () => server.requests.filter(row => row.request.method === 'conversation/checkpointValueRead').length,
  };
}
declare global { interface Window { traceLiveFixture: { holdSummary(): void; releaseSummary(): void; summaries(): number; run(count: number): Promise<void>; change(): Promise<void>; releaseHeader(): void; releaseContent(): void; reads(): number; values(): number } } }
