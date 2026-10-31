import { branchGeometry } from '../../document/branch';
import type { Node, Edge } from '@xyflow/react';
import type { WorkflowNote } from '../../../../../../protocol/app-server/v62';
import { nodePresentation, START_POSITION } from '../../document/geometry';
import { edgesOf, isContainer, nodeAt, startOf, sameBlockPath, type Block, type BlockPath, type NodeDefinition, type WorkflowDocument } from '../../document/model';
import { edgePort } from '../../document/ports';
import { schemaObject } from '../../document/schema';
import { orderedKeys } from '../../document/presentation';
import { viewId } from './selection';
import { previewRowCount } from './summary';
import type { VariableSources } from './VariableLabel';

export const NODE_WIDTH = 256;
/** As Dify's LoopStart/IterationStart: a 44px square at (24, 68). */
export const BODY_START_SIZE = 44;
const BODY_START_POSITION = { x: 24, y: 68 };
export interface Size { width: number; height: number }
export interface SceneData extends Record<string, unknown> {
  view: 'node' | 'start' | 'note';
  path: BlockPath;
  local: string;
  definition?: NodeDefinition;
  note?: WorkflowNote;
  title: string;
  description?: string;
  fields?: { name: string; type: string; required: boolean }[];
  /** Loop and Iteration show their body inside them, as Dify's. */
  container?: boolean;
  /** A container whose body holds only its Start: that Start's
   * position, where Dify's "Add node" button follows it. */
  emptyBody?: { start: string; position: { x: number; y: number } };
  /** Titles for the references a card shows, shared by the block's nodes. */
  sources?: VariableSources;
  minSize: Size;
}
export type SceneNode = Node<SceneData, 'workflow'>;
export interface SceneEdgeData extends Record<string, unknown> { path: BlockPath; index: number }
export type SceneEdge = Edge<SceneEdgeData>;

const CARD_DESCRIPTION_HEIGHT = 42;
const ENTRY_BADGE_HEIGHT = 20;
const ERROR_ROW_HEIGHT = 32;
const ROUTE_ROW_HEIGHT = 28;
const ROUTE_ROWS_MARGIN = 4;

/** The first six fields of an object schema, in `order` (sorted by default). */
export function schemaFields(value: unknown, order = (keys: string[]) => keys.sort()): { name: string; type: string; required: boolean }[] {
  const schema = schemaObject(value), properties = schemaObject(schema?.properties);
  if (!properties) return [];
  const required = Array.isArray(schema?.required) ? schema.required : [];
  return order(Object.keys(properties)).slice(0, 6)
    .map(name => ({ name, type: String(schemaObject(properties[name])?.type ?? '?'), required: required.includes(name) }));
}

/** One recursive scene over the native document. Parent relationships express
 * presentation only; every command still addresses the native Block path. */
export function projectScene(document: WorkflowDocument, measured: ReadonlyMap<string, Size> = new Map()): { nodes: SceneNode[]; edges: SceneEdge[] } {
  const nodes: SceneNode[] = [], edges: SceneEdge[] = [];
  const visit = (block: Block, path: BlockPath, parentId?: string, around?: VariableSources): Size => {
    const local: SceneNode[] = [];
    const entryId = viewId(path, 'start'), startId = startOf(block);
    const start = startId ? nodePresentation(document, path, startId) : undefined;
    // As Dify, a body also reads the variables around it, and `args` is
    // always the workflow input of the root Start.
    const sources: VariableSources = { ...around, ...Object.fromEntries(Object.entries(block.nodes).map(([id, node]) => [id, { kind: node.type, title: nodePresentation(document, path, id)?.title || id }])),
      args: around?.args ?? { kind: 'start', title: start?.title ?? '' } };
    const fields = schemaFields(block.input, keys => orderedKeys(document, { kind: 'block_input', block: [...path], path: [] }, keys));
    // Start renders at its intrinsic height, as Dify's EntryNodeContainer does.
    // Before measurement, estimate its header/padding, field rows and at most
    // two description lines; Organize uses the actual measured footprint.
    const startSize = path.length ? { width: BODY_START_SIZE, height: BODY_START_SIZE } : { width: NODE_WIDTH, height: 57 + (fields.length ? 8 + fields.length * 26 : 0)
      + (start?.description?.trim() ? 42 : 0) + ENTRY_BADGE_HEIGHT };
    if (startId) local.push({ id: entryId, type: 'workflow', parentId, position: start?.position ?? (path.length ? BODY_START_POSITION : START_POSITION), width: startSize.width, height: path.length ? startSize.height : measured.get(entryId)?.height ?? startSize.height, deletable: false,
      data: { view: 'start', path, local: startId, title: start?.title ?? '', description: start?.description ?? undefined, fields, minSize: startSize } });
    let x = 360;
    const ids = [...new Set([...edgesOf(block).flatMap(edge => [edge.from, edge.to]), ...Object.keys(block.nodes).sort()])].filter(id => nodeAt(block, id) && block.nodes[id].type !== 'start');
    for (const id of ids) {
      const node = block.nodes[id], qualified = viewId(path, 'node', id), metadata = nodePresentation(document, path, id);
      const container = isContainer(node);
      // A shown description (two clamped lines) extends a card.
      const described = !container && node.type !== 'branch' && !!metadata?.description?.trim();
      const preview = node.type === 'branch' ? 0 : previewRowCount(node);
      // A Model or Agent handling errors shows its strategy on one more row,
      // and each Agent route is a row carrying its exit.
      const handled = (node.type === 'model' || node.type === 'tool' || node.type === 'agent') && !!node.on_error && node.on_error.type !== 'stop';
      const routes = node.type === 'agent' ? node.routes?.length ?? 0 : 0;
      // Estimates are for initial placement only. Ordinary cards render at
      // their intrinsic height, and Organize uses their measured footprint.
      const minimum: Size = { width: NODE_WIDTH, height: (node.type === 'branch' ? branchGeometry(node).height : 57 + (preview ? 8 + preview * 27 : 0)) + (described ? CARD_DESCRIPTION_HEIGHT : 0) + (handled ? ERROR_ROW_HEIGHT : 0) + (routes ? ROUTE_ROWS_MARGIN + routes * ROUTE_ROW_HEIGHT : 0) };
      const projected: SceneNode = { id: qualified, type: 'workflow', parentId, position: metadata?.position ?? { x, y: 80 }, width: NODE_WIDTH, height: minimum.height,
        data: { view: 'node', path, local: id, definition: node, title: metadata?.title || id, description: metadata?.description ?? undefined, container, sources, minSize: minimum } };
      // Parents precede children, as React Flow requires.
      nodes.push(projected);
      if (isContainer(node)) {
        const inner: BlockPath = [...path, { node: id }], body = visit(node.body, inner, qualified, sources), bodyStart = startOf(node.body);
        if (bodyStart && Object.keys(node.body.nodes).length === 1) projected.data.emptyBody = { start: viewId(inner, 'start'), position: nodePresentation(document, inner, bodyStart)?.position ?? BODY_START_POSITION };
        minimum.width = Math.max(480, body.width + 32); minimum.height = Math.max(260, body.height + 32);
      }
      const size = container ? metadata?.dimensions : measured.get(qualified);
      projected.width = Math.max(minimum.width, size?.width ?? 0);
      projected.height = container ? Math.max(minimum.height, size?.height ?? 0) : size?.height ?? minimum.height;
      local.push(projected); x += projected.width + 96;
    }
    for (const note of document.editor?.notes ?? []) if (sameBlockPath(note.block, path)) local.push({ id: viewId(path, 'note', note.id), type: 'workflow', parentId,
      position: note.position, width: note.dimensions.width, height: note.dimensions.height, connectable: false,
      data: { view: 'note', path, local: note.id, title: note.id, note, minSize: { width: 160, height: 88 } } });
    // Nodes already pushed are container parents, so append only ordinary local
    // projections here. The later stable topological ordering is independent
    // of map insertion order and never changes executable edge order.
    for (const item of local) if (!nodes.includes(item)) nodes.push(item);
    for (const [index, edge] of edgesOf(block).entries()) if (nodeAt(block, edge.from) && nodeAt(block, edge.to)) edges.push({ id: viewId(path, 'edge', String(index)), source: edge.from === startId ? entryId : viewId(path, 'node', edge.from), target: viewId(path, 'node', edge.to), sourceHandle: edgePort(edge), targetHandle: 'in', type: 'workflow', data: { path, index } });
    return { width: Math.max(400, ...local.map(node => node.position.x + (node.width ?? NODE_WIDTH))), height: Math.max(200, ...local.map(node => node.position.y + (node.height ?? 128))) };
  };
  visit(document.block, []);
  // A child container may have been emitted before its ancestor was appended.
  // Stable depth order guarantees parent-before-child, including private lanes.
  const depth = (node: SceneNode): number => node.parentId ? 1 + depth(nodes.find(parent => parent.id === node.parentId)!) : 0;
  nodes.sort((a, b) => depth(a) - depth(b));
  return { nodes, edges };
}
