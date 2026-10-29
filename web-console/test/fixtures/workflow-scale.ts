/** Native-valid aggregate-bound fixture. No production mock or execution API. */
import type { WorkflowBlock, WorkflowFileDocument } from '../../../protocol/app-server/v62';
const empty = { type: 'object', properties: {}, additionalProperties: false };
const returned = { type: 'return' as const, output: { type: 'literal' as const, value: {} } };
function branchTree(): WorkflowBlock {
  const nodes: WorkflowBlock['nodes'] = {}, edges: NonNullable<WorkflowBlock['edges']> = [];
  for (let index = 0; index < 253; index++) {
    const id = `node_${index}`;
    if (index < 126) {
      nodes[id] = { type: 'branch', condition: { type: 'boolean', value: { type: 'literal', value: true } } };
      edges.push({ from: id, to: `node_${index * 2 + 1}`, port: 'true' }, { from: id, to: `node_${index * 2 + 2}`, port: 'false' });
    } else nodes[id] = returned;
  }
  return { input: empty, output: empty, entry: 'node_0', nodes, edges };
}
export function scaleWorkflow(): WorkflowFileDocument {
  return { description: 'Native 256-node editor capacity fixture.', block: { input: empty, output: empty, entry: 'fork', nodes: {
    fork: { type: 'parallel', branches: { main: { input: { type: 'literal', value: {} }, block: branchTree() }, other: { input: { type: 'literal', value: {} }, block: { input: empty, output: empty, entry: 'done', nodes: { done: returned }, edges: [] } } } },
    done: returned,
  }, edges: [{ from: 'fork', to: 'done' }] }, editor: { notes: [{ id: 'manual', block: [], text: 'Manual note: organize preserves this position.', theme: 'neutral', position: { x: -120, y: 300 }, dimensions: { width: 280, height: 120 } }] } };
}
