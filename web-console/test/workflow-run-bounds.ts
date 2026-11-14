import bounds from './fixtures/workflow-run-bounds.json';
import type { WorkflowFileDocument, WorkflowRunId, WorkflowTraceNode } from '../../protocol/app-server/v78';

/** Shared with the native compiler's inspection_bounds_fixture_is_admissible test.
 * JSON is decoded at this boundary; both documents must pass native admission. */
export function runBoundsDocument(kind: 'maximum' | 'long_history'): WorkflowFileDocument {
  return structuredClone(bounds[kind]) as WorkflowFileDocument;
}

/** A 42-node admitted graph runs 100 rounds: 3,903 concrete visits / 4,003 steps. */
export function longRunVisits(run: WorkflowRunId, workflowId: string): WorkflowTraceNode[] {
  const visit = (node: string, iteration?: number): WorkflowTraceNode => ({
    id: iteration == null ? `root-${node}` : `round-${iteration}-${node}`,
    instance: {
      block: { run, definition: { workflow_id: workflowId, blocks: iteration == null ? [] : ['repeat', 'body'] }, invocations: iteration == null ? [0] : [0, iteration] },
      node, visit: 0,
    },
    kind: node.endsWith('start') || node === 'start' ? 'start' : node === 'repeat' ? 'loop' : node === 'done' ? 'return' : 'template',
    state: { type: 'settled', outcome: 'completed' },
    timing: { started_at: '2026-11-14T00:00:00Z', ended_at: '2026-11-14T00:00:01Z', duration_ms: '1000' },
    input: null, output: null, input_preview: null, output_preview: null, input_omitted: false, output_omitted: false,
  });
  const nodes = [visit('start'), visit('repeat')];
  for (let round = 1; round <= 100; round++) {
    nodes.push(visit('body_start', round));
    for (let index = 0; index < 38; index++) nodes.push(visit(`worker_${index}`, round));
  }
  nodes.push(visit('done'));
  return nodes;
}
