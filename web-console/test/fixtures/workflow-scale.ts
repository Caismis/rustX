/** The native 256-node aggregate bound, with real Start nodes and a Loop. */
import type { WorkflowBlock, WorkflowFileDocument } from '../../../protocol/app-server/v62';
const empty = { type: 'object', properties: {}, additionalProperties: false };
const returned = { type: 'return' as const, output: { type: 'literal' as const, value: {} } };
function branchTree(): WorkflowBlock {
  const nodes: WorkflowBlock['nodes'] = { start: { type: 'start' } }, edges: NonNullable<WorkflowBlock['edges']> = [{ from: 'start', to: 'node_0' }];
  for (let index = 0; index < 251; index++) {
    const id = `node_${index}`;
    if (index < 125) {
      nodes[id] = { type: 'branch', condition: { type: 'boolean', value: { type: 'literal', value: true } } };
      edges.push({ from: id, to: `node_${index * 2 + 1}`, port: 'true' }, { from: id, to: `node_${index * 2 + 2}`, port: 'false' });
    } else nodes[id] = returned;
  }
  return { input: empty, output: empty, nodes, edges };
}
export function scaleWorkflow(): WorkflowFileDocument {
  return { description: 'Native 256-node editor capacity fixture.', block: { input: empty, output: empty, nodes: {
    start: { type: 'start' },
    repeat: { type: 'loop', input: { type: 'literal', value: {} }, body: branchTree(), until: { type: 'boolean', value: { type: 'literal', value: true } }, carry: { type: 'reference', path: ['result'] }, max_iterations: 1 },
    model: { type: 'model', model: { mode: 'session' }, instruction: 'Return text.', input: {}, output: { type: 'text' } }, done: returned,
  }, edges: [{ from: 'start', to: 'repeat' }, { from: 'start', to: 'model' }, { from: 'model', to: 'done' }, { from: 'repeat', to: 'done', port: 'satisfied' }, { from: 'repeat', to: 'done', port: 'exhausted' }] }, editor: { notes: [{ id: 'manual', block: [], text: 'Manual note: organize preserves this position.', theme: 'neutral', position: { x: -120, y: 300 }, dimensions: { width: 280, height: 120 } }] } };
}
