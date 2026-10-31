import type { ElkNode } from 'elkjs/lib/elk-api';
import { blockAt, childSteps, edgesOf, sameBlockPath, type BlockPath, type WorkflowDocument } from '../../document/model';
import { setNodePresentation } from '../../document/geometry';
import { edgePort, portsOf } from '../../document/ports';
import { projectScene, type Size } from './scene';
import { viewId } from './selection';

export interface Point { readonly x: number; readonly y: number }
let engine: Promise<import('elkjs/lib/elk-api').ELK> | undefined;
function elk() {
  return engine ??= import('elkjs/lib/elk.bundled.js').then(({ default: ELK }) => new ELK());
}
const OPTIONS = {
  'elk.algorithm': 'layered', 'elk.direction': 'RIGHT', 'elk.randomSeed': '1',
  'elk.spacing.nodeNode': '72', 'elk.layered.spacing.nodeNodeBetweenLayers': '96',
  'elk.layered.considerModelOrder.strategy': 'NODES_AND_EDGES',
  'elk.padding': '[top=80,left=32,bottom=32,right=32]',
};

/** The sole organize implementation. Lay out private Blocks before measuring
 * their containers. Notes stay put; no execution field is changed. */
export async function organizeDocument(document: WorkflowDocument, measured: ReadonlyMap<string, Size> = new Map()): Promise<WorkflowDocument> {
  const layout = await elk();
  let next = document;
  const visit = async (path: BlockPath) => {
    const block = blockAt(next, path)!;
    for (const [id, node] of Object.entries(block.nodes).sort(([a], [b]) => a.localeCompare(b))) for (const step of childSteps(id, node)) await visit([...path, step]);
    const scene = projectScene(next, measured);
    const children: ElkNode[] = scene.nodes.filter(node => (node.data.view === 'start' || node.data.view === 'node') && sameBlockPath(node.data.path, path)).map(node => ({
      id: node.id, width: node.width, height: node.height,
      layoutOptions: { 'elk.portConstraints': 'FIXED_ORDER' },
      ports: [{ id: `${node.id}:in`, width: 0, height: 0, layoutOptions: { 'elk.port.side': 'WEST' } },
        ...(node.data.definition ? portsOf(node.data.definition) : ['next']).map((port, index) => ({ id: `${node.id}:${port}`, width: 0, height: 0, layoutOptions: { 'elk.port.side': 'EAST', 'elk.port.index': String(index) } }))],
    }));
    const ids = new Set(children.map(node => node.id));
    const edges = edgesOf(block).flatMap((edge, index) => {
      const source = block.nodes[edge.from]?.type === 'start' ? viewId(path, 'start') : viewId(path, 'node', edge.from), target = viewId(path, 'node', edge.to);
      return ids.has(source) && ids.has(target) ? [{ id: `edge-${index}`, sources: [`${source}:${edgePort(edge)}`], targets: [`${target}:in`] }] : [];
    });
    const result = await layout.layout({ id: JSON.stringify(path), layoutOptions: OPTIONS, children, edges });
    for (const child of result.children ?? []) {
      if (!Number.isFinite(child.x) || !Number.isFinite(child.y)) throw new Error('Layout did not return finite positions.');
      const original = scene.nodes.find(node => node.id === child.id)!;
      const point = { x: child.x!, y: child.y! };
      next = setNodePresentation(next, path, original.data.local, { position: point });
    }
  };
  await visit([]);
  return next;
}

export interface LayoutOwner { readonly document?: WorkflowDocument; readonly generation: number }
/** New requests, edits and drags revoke older completions. */
export class LayoutSession {
  private version = 0;
  cancel() { this.version++; }
  async run(owner: LayoutOwner, current: () => LayoutOwner, compute: () => Promise<WorkflowDocument>): Promise<WorkflowDocument | undefined> {
    const version = ++this.version;
    const active = () => { const now = current(); return version === this.version && now.document === owner.document && now.generation === owner.generation; };
    try {
      const result = await compute();
      return active() ? result : undefined;
    } catch (error) {
      if (!active()) return undefined;
      throw error;
    }
  }
}
