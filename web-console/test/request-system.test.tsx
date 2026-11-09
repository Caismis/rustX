import { expect, it, vi } from 'vitest';
import type { CheckpointPage, CheckpointReadCut, TraceRecord, TraceToolDefinition } from '../../protocol/app-server/v76';
import { completeTools, readRequestPredecessor } from '../src/client/request-system';
import type { TraceFieldReader } from '../src/client/trace';
import { traceRecord, requestDetail } from './trace-fixture';
import { nativeTraceFixture } from './native-trace-fixture';
import { checkpoint, snapshot, readCut } from './fixture';

const cut: CheckpointReadCut = { ...checkpoint(snapshot()).read_cut, source_cut: { read: readCut(), ledger_through: '0', transcript_total: '0', surface_revision: '0', inherited_through: '0' } };
function tracePage(cut: CheckpointReadCut, records: TraceRecord[], next?: number): CheckpointPage {
  return { read_cut: cut, cursor: cut.revision, domain: 'trace.records', total: '3', next_offset: next == null ? null : String(next), collections: [],
    items: records.map(record => ({ offset: record.id.slice(6), identity: record.id, value: record, deferred: false, content: [] })) };
}
function fixture() {
  const before = traceRecord(2), after = traceRecord(4), boundary = traceRecord(1, { kind: 'attempt', request: null });
  after.request!.predecessor = { availability: 'available', request_id: before.request!.request_id };
  after.request!.retry_number = 0; after.request!.system_prompt.state = 'changed'; after.request!.tool_catalog = 'changed';
  const previous = requestDetail(2), current = requestDetail(4);
  previous.request!.effective_system_prompt.text = 'OLD SYSTEM'; current.request!.effective_system_prompt.text = 'NEW SYSTEM\n' + 'x'.repeat(70_000) + '\nPROMPT_END';
  const oldTool = previous.request!.tools[0]!;
  current.request!.tools = [{ ...oldTool, input_schema: { value: { type: 'object', required: ['command'] }, truncated: false } }, { ...oldTool, name: 'search', tool_id: 'tool-search' }];
  const oldSource = nativeTraceFixture(before, previous), newSource = nativeTraceFixture(after, current);
  const reader: TraceFieldReader = {
    page: vi.fn(async (cut, domain, offset, owned) => {
      if (!owned()) throw Error('Retired source');
      if (domain === 'trace.records') return offset === 0 ? tracePage(cut, [after], 3) : tracePage(cut, [boundary, before]);
      return (domain.startsWith('trace.records.2.') ? oldSource : newSource).original.page(cut, domain, offset, owned);
    }),
    value: vi.fn(async (cut, field, offset, owned) => (field.domain.startsWith('trace.records.2.') ? oldSource : newSource).original.value(cut, field, offset, owned)),
    content: vi.fn(async () => { throw Error('No payload content expected'); }),
  };
  return { before, after, boundary, current, reader };
}

it('resolves the explicitly recorded predecessor rather than the adjacent request', async () => {
  const f = fixture(), wrong = traceRecord(3);
  const read = f.reader.page;
  f.reader.page = async (...args) => args[1] === 'trace.records' && args[2] === 0 ? tracePage(cut, [wrong, f.after], 3) : read(...args);
  expect((await readRequestPredecessor(f.reader, cut, f.after, () => true))?.id).toBe(f.before.id);
  f.after.request!.predecessor = { availability: 'unavailable' };
  expect(await readRequestPredecessor(f.reader, cut, f.after, () => true)).toBeUndefined();
});

it('rejects a foreign frozen scope and a cursor that fails to advance', async () => {
  const f = fixture();
  f.reader.page = async () => tracePage({ ...cut, conversation_id: 'foreign' }, []);
  await expect(readRequestPredecessor(f.reader, cut, f.after, () => true)).rejects.toThrow('scope changed');
  f.reader.page = async () => tracePage(cut, [], 3);
  await expect(readRequestPredecessor(f.reader, cut, f.after, () => true)).rejects.toThrow('did not advance');
});

it('rejects partial Tool catalogs', () => {
  const original = requestDetail(2).request!.tools, updated: TraceToolDefinition[] = [{ ...original[0]!, description: { text: 'New instructions', truncated: false } }];
  expect(completeTools(updated)).toBe(true);
  expect(completeTools([{ ...updated[0]!, input_schema: { value: {}, truncated: true } }])).toBe(false);
});
