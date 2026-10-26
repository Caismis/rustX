import type { CheckpointReadCut, TraceRecord, TraceToolDefinition } from '../../../protocol/app-server/v59';
import { checkpointReadCutKey } from '../../../protocol/app-server/content-identity';
import { traceAttemptKey, traceFieldKey } from '../../../protocol/app-server/trace-identity';
import type { TraceFieldReader } from './trace';
import { readTraceValue } from './trace-content';

export function requestDomain(record: TraceRecord): string {
  const sequence = /^trace:(0|[1-9][0-9]*)$/.exec(record.id)?.[1];
  if (!sequence || record.kind !== 'request') throw Error('Original request identity is unavailable.');
  return `trace.records.${sequence}.detail.request`;
}

export function isRequestPredecessor(record: TraceRecord, candidate: TraceRecord) {
  const previous = record.request?.predecessor, request = candidate.request;
  return previous?.availability === 'available' && request != null && (previous.request_id != null
    ? request.request_id === previous.request_id : previous.request_id_source != null && request.request_id_source != null
      && traceFieldKey(previous.request_id_source) === traceFieldKey(request.request_id_source));
}

/** The native Trace cursor is an exclusive boundary, not a collection rank. */
export async function readSystemTracePage(reader: TraceFieldReader, cut: CheckpointReadCut, before: number, current: () => boolean) {
  if (!current()) throw Error('Original Trace scope changed.');
  const page = await reader.page(cut, 'trace.records', before, current);
  if (!current() || page.domain !== 'trace.records' || checkpointReadCutKey(page.read_cut) !== checkpointReadCutKey(cut)) throw Error('Original Trace scope changed.');
  const records: TraceRecord[] = [];
  for (const item of page.items) {
    const offset = Number(item.offset);
    if (item.deferred && !Number.isSafeInteger(offset)) throw Error('Original Trace header offset is unavailable.');
    const value: unknown = item.deferred ? JSON.parse((await readTraceValue(reader, cut, { domain: page.domain, item_offset: offset, path: [] }, current)).text) : item.value;
    if (!value || typeof value !== 'object' || !('id' in value) || value.id !== item.identity || !('location' in value) || !('kind' in value)) throw Error('Original Trace header is unavailable.');
    records.push(value as TraceRecord);
  }
  const next = page.next_offset == null ? undefined : Number(page.next_offset);
  if (next !== undefined && (!Number.isSafeInteger(next) || next <= 0 || before !== 0 && next >= before)) throw Error('Native Trace history did not advance.');
  return { records, next };
}

/** Resolve only the explicitly recorded predecessor, including outside the UI window. */
export async function readRequestPredecessor(reader: TraceFieldReader, cut: CheckpointReadCut, record: TraceRecord, current: () => boolean) {
  if (record.request?.predecessor.availability !== 'available') return;
  let before = 0;
  do {
    const page = await readSystemTracePage(reader, cut, before, current);
    const found = page.records.find(candidate => isRequestPredecessor(record, candidate));
    if (found) return found;
    if (page.next === undefined) break;
    before = page.next;
  } while (current());
  throw Error('Original preceding request is unavailable.');
}

/** Read requests and input anchors for represented native Attempts, stopping
 * after their start rows and the visible ordinary inputs have been covered.
 * The result is replaceable view data, never a browser-authored change history. */
export async function readSystemRequests(reader: TraceFieldReader, cut: CheckpointReadCut, owners: ReadonlySet<string>, current: () => boolean, inputIds: ReadonlySet<string> = new Set()) {
  if (!owners.size) return [];
  const pending = new Set(owners), inputs = new Set(inputIds), requests: TraceRecord[] = [];
  let before = 0;
  do {
    const page = await readSystemTracePage(reader, cut, before, current);
    requests.unshift(...page.records.filter(record => (record.kind === 'request' || record.kind === 'user') && owners.has(traceAttemptKey(record.location) ?? '')));
    for (const record of page.records) {
      if (record.kind === 'attempt') pending.delete(traceAttemptKey(record.location) ?? '');
      if (record.kind === 'user' && record.message_id != null) inputs.delete(record.message_id);
    }
    if (!pending.size && !inputs.size || page.next === undefined) break;
    before = page.next;
  } while (current());
  return requests;
}

export function completeTools(tools: readonly TraceToolDefinition[]) {
  return tools.every(tool => !tool.description.truncated && !tool.input_schema.truncated);
}

export function toolChanges(previous: readonly TraceToolDefinition[], current: readonly TraceToolDefinition[]) {
  const before = new Map(previous.map(tool => [tool.name, tool])), after = new Map(current.map(tool => [tool.name, tool]));
  return {
    added: current.filter(tool => !before.has(tool.name)).map(tool => tool.name),
    removed: previous.filter(tool => !after.has(tool.name)).map(tool => tool.name),
    updated: current.filter(tool => before.has(tool.name) && JSON.stringify(before.get(tool.name)) !== JSON.stringify(tool)).map(tool => tool.name),
  };
}
