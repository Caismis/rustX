/** The native 256-node aggregate bound, with real Start nodes and a Loop
 * whose branch tree ends in Exit Loop nodes. */
import type { WorkflowBlock, WorkflowFileDocument } from '../../../protocol/app-server/v62';
const empty = { type: 'object', properties: {}, additionalProperties: false };
const returned = { type: 'return' as const, outputs: [] };
function branchTree(): WorkflowBlock {
  const nodes: WorkflowBlock['nodes'] = { repeat_start: { type: 'start' } }, edges: NonNullable<WorkflowBlock['edges']> = [{ from: 'repeat_start', to: 'node_0' }];
  for (let index = 0; index < 251; index++) {
    const id = `node_${index}`;
    if (index < 125) {
      nodes[id] = { type: 'branch', cases: [{ id: 'true', condition: { type: 'boolean', value: { type: 'literal', value: true } } }] };
      edges.push({ from: id, to: `node_${index * 2 + 1}`, port: 'true' }, { from: id, to: `node_${index * 2 + 2}`, port: 'false' });
    } else nodes[id] = { type: 'loop_end' };
  }
  return { nodes, edges };
}
export function scaleWorkflow(): WorkflowFileDocument {
  return { description: 'Native 256-node editor capacity fixture.', block: { input: empty, nodes: {
    start: { type: 'start' },
    repeat: { type: 'loop', loop_count: 1, body: branchTree() },
    model: { type: 'model', model: { mode: 'session' }, messages: [{ role: 'system', content: [{ type: 'literal', value: 'Return text.' }] }, { role: 'user', content: [{ type: 'literal', value: 'Go.' }] }], output: { type: 'text' } }, done: returned,
  }, edges: [{ from: 'start', to: 'repeat' }, { from: 'start', to: 'model' }, { from: 'model', to: 'done' }, { from: 'repeat', to: 'done' }] }, editor: { notes: [{ id: 'manual', block: [], text: 'Manual note: organize preserves this position.', theme: 'neutral', position: { x: -120, y: 300 }, dimensions: { width: 280, height: 120 } }] } };
}
