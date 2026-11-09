import type { CheckpointPage, CheckpointReadCut, TraceRecord, WorkflowRunId } from '../../protocol/app-server/v75';
import type { TraceCache, TraceFieldReader } from '../src/client/trace';
import type { WorkflowTraceNode } from '../src/app/trajectory/workflow';
import { traceRecord, traceTool } from './trace-fixture';
import { replaceTrace } from '../src/client/trace';

export function workflowTraceFixture(count = 121) {
  const cut: CheckpointReadCut = { conversation_id: 'workflow-conversation', revision: '500', through: '500', domain_revisions: {}, collection_cuts: {} };
  const location = { attempt_id: 'attempt-a', step_id: '1' };
  const inputs = new Map<string, string>(), nodes = new Map<string, WorkflowTraceNode[]>();
  const roots: TraceRecord[] = [], calls: TraceRecord[] = [];
  for (const [index, outcome] of ['completed', 'cancelled', 'running'].entries()) {
    const call = traceTool(10 + index, { location, tool: { ...traceTool(10 + index).tool!, tool_id: 'tool-workflow-test_flow', name: 'test_flow' }, state: outcome as TraceRecord['state'] });
    const root = traceRecord(30 + index, { kind: 'workflow', request: null, location, originating_tool_call_id: call.tool!.call_id,
      preview: { text: 'test_flow', truncated: false }, state: call.state, collections: [{ domain: `trace.records.${30 + index}.workflow.nodes`, revision: '500', total: String(index ? 2 : count), loaded: '0', next_offset: '0' }] });
    const run: WorkflowRunId = { conversation_id: cut.conversation_id, origin: { type: 'tool', attempt_id: 'attempt-a' }, invocation: String(index + 1) };
    const rows = Array.from({ length: index ? 2 : count }, (_, ordinal): WorkflowTraceNode => {
      const id = `node-${index}-${ordinal}`, input = `journal.input-${id}`, output = `journal.output-${id}`;
      const inValue = ordinal === 0 ? '{"args":{"integer":18446744073709551615},"variables":{}}' : JSON.stringify({ args: { item: ordinal }, variables: { prepared: `input-${ordinal}` } });
      const outValue = JSON.stringify({ value: `output-${ordinal}`, returned: null, port: 'next' });
      inputs.set(input, inValue); inputs.set(output, outValue);
      const field = (domain: string, kind: 'arguments' | 'result', value: string) => ({ type: 'value' as const, field: kind, domain, item_offset: 0, path: ['event', 'value'], encoded_bytes: String(new TextEncoder().encode(JSON.stringify(value)).length) });
      return { id, instance: { block: { run, definition: { workflow_id: 'test_flow', blocks: ordinal > 2 ? ['each', 'body'] : [] }, invocations: ordinal > 2 ? [ordinal] : [] }, node: ordinal === 0 ? 'start' : ordinal === 1 ? 'transform' : 'visit', visit: 0 },
        kind: ordinal === 0 ? 'start' : 'template', state: outcome === 'running' && ordinal === 1 ? { type: 'waiting', reason: 'review' } : { type: 'settled', outcome: outcome === 'cancelled' && ordinal === 1 ? 'cancelled' : 'completed' },
        timing: { started_at: '2026-09-15T00:00:00Z', ended_at: '2026-09-15T00:00:01Z', duration_ms: '1000' },
        input: field(input, 'arguments', inValue), output: outcome === 'running' && ordinal === 1 ? null : field(output, 'result', outValue), input_preview: inValue, output_preview: outcome === 'running' && ordinal === 1 ? null : outValue,
        input_omitted: false, output_omitted: false };
    });
    nodes.set(root.collections[0].domain, rows); calls.push(call); roots.push(root);
  }
  const ordinary = traceTool(20, { tool: { ...traceTool(20).tool!, name: 'test_flow', tool_id: 'tool-bash' } });
  const records = [traceRecord(1, { kind: 'user', request: null, location, preview: { text: 'Run three workflows.', truncated: false } }), ...calls, ordinary, ...roots];
  const reads: { domain: string; offset: number }[] = [];
  const reader: TraceFieldReader = {
    page: async (given, domain, offset, current): Promise<CheckpointPage> => {
      if (!current() || JSON.stringify(given) !== JSON.stringify(cut) || !nodes.has(domain)) throw Error('Fixture original Workflow source mismatch');
      reads.push({ domain, offset }); const all = nodes.get(domain)!;
      return { cursor: cut.revision, read_cut: cut, domain, total: String(all.length), collections: [],
        items: all.slice(offset, offset + 32).map((node, i) => ({ offset: String(offset + i), identity: node.id, value: node, content: [], deferred: false, body_complete: true })), next_offset: offset + 32 < all.length ? String(offset + 32) : null };
    },
    value: async (given, field, offset, current) => {
      if (!current() || JSON.stringify(given) !== JSON.stringify(cut) || !inputs.has(field.domain)) throw Error('Fixture original Workflow field mismatch');
      const bytes = new TextEncoder().encode(JSON.stringify(inputs.get(field.domain))), end = Math.min(bytes.length, offset + 65_536);
      return { cut, domain: field.domain, item_offset: 0, path: field.path, offset, readable_end: bytes.length, bytes_base64: btoa(String.fromCharCode(...bytes.slice(offset, end))), next_offset: end < bytes.length ? end : null, representation: 'canonical_json_v1' };
    },
    content: async () => { throw Error('No guessed workflow content'); },
  };
  const cache: TraceCache = { ...replaceTrace({ records, next_cursor: null }), readCuts: Object.fromEntries(records.map(record => [record.id, cut])), fields: reader };
  return { cache, roots, calls, nodes, inputs, reader, reads, cut };
}
