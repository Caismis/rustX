import type { Node, Edge } from '@xyflow/react';
import type { WorkflowNote } from '../../../../../../protocol/app-server/v62';
import { nodePresentation } from '../../document/geometry';
import { blockKey, edgesOf, nodeAt, sameBlockPath, type Block, type BlockPath, type NodeDefinition, type WorkflowDocument } from '../../document/model';
import { edgePort } from '../../document/ports';
import { schemaObject } from '../../document/schema';
import { orderedKeys } from '../../document/presentation';
import { viewId } from './selection';

export const NODE_WIDTH = 256;
export interface Size { width: number; height: number }
export interface SceneData extends Record<string, unknown> {
  view: 'node' | 'start' | 'lane' | 'note';
  path: BlockPath;
  local: string;
  definition?: NodeDefinition;
  note?: WorkflowNote;
  title: string;
  description?: string;
  fields?: { name: string; type: string }[];
  expanded?: boolean;
  minSize: Size;
}
export type SceneNode = Node<SceneData, 'workflow'>;
export interface SceneEdgeData extends Record<string, unknown> { path: BlockPath; index?: number; entry?: boolean }
export type SceneEdge = Edge<SceneEdgeData>;

export function schemaFields(value: unknown): { name: string; type: string }[] {
  const schema = schemaObject(value), properties = schemaObject(schema?.properties);
  return properties ? Object.keys(properties).sort().slice(0, 6).map(name => ({ name, type: String(schemaObject(properties[name])?.type ?? '?') })) : [];
}

/** One recursive scene over the native document. Parent relationships express
 * presentation only; every command still addresses the native Block path. */
export function projectScene(document: WorkflowDocument, collapsed: ReadonlySet<string> = new Set(), measured: ReadonlyMap<string, Size> = new Map()): { nodes: SceneNode[]; edges: SceneEdge[] } {
  const nodes: SceneNode[] = [], edges: SceneEdge[] = [];
  const visit = (block: Block, path: BlockPath, parentId?: string): Size => {
    const local: SceneNode[] = [];
    const entryId = viewId(path, 'start');
    const start = document.editor?.entries?.find(entry => sameBlockPath(entry.block, path));
    const fields = schemaFields(block.input);
    local.push({ id: entryId, type: 'workflow', parentId, position: start?.position ?? { x: 32, y: 80 }, width: NODE_WIDTH, height: Math.max(112, 76 + fields.length * 22), deletable: false,
      data: { view: 'start', path, local: '', title: path.length ? blockKey(path) : '', fields, minSize: { width: NODE_WIDTH, height: 112 } } });
    let x = 360;
    const ids = [...new Set([block.entry, ...edgesOf(block).flatMap(edge => [edge.from, edge.to]), ...Object.keys(block.nodes).sort()])].filter(id => nodeAt(block, id));
    for (const id of ids) {
      const node = block.nodes[id], qualified = viewId(path, 'node', id), metadata = nodePresentation(document, path, id);
      const expanded = (node.type === 'loop' || node.type === 'parallel') && !collapsed.has(qualified);
      const minimum: Size = { width: NODE_WIDTH, height: node.type === 'branch' ? 174 : 128 };
      const projected: SceneNode = { id: qualified, type: 'workflow', parentId, position: metadata?.position ?? { x, y: 80 }, width: NODE_WIDTH, height: minimum.height,
        data: { view: 'node', path, local: id, definition: node, title: metadata?.title || id, description: metadata?.description ?? undefined, expanded, minSize: minimum } };
      // Parents precede children, as React Flow requires.
      nodes.push(projected);
      if (expanded && node.type === 'loop') {
        const body = visit(node.body, [...path, { node: id }], qualified);
        minimum.width = Math.max(480, body.width + 32); minimum.height = Math.max(260, body.height + 32);
      }
      if (expanded && node.type === 'parallel') {
        const frames: SceneNode[] = [], positioned = new Set<string>();
        const keys = orderedKeys(document, { kind: 'branches', address: { block: [...path], node: id } }, Object.keys(node.branches));
        for (const branch of keys) {
          const childPath = [...path, { node: id, branch }], laneId = viewId(childPath, 'lane');
          const lane = document.editor?.branches?.find(entry => entry.address.node === id && entry.branch === branch && sameBlockPath(entry.address.block, path));
          const frame: SceneNode = { id: laneId, type: 'workflow', parentId: qualified, position: lane?.position ?? { x: 24, y: 72 }, draggable: false, deletable: false,
            data: { view: 'lane', path: childPath, local: branch, title: branch, minSize: { width: 400, height: 220 } } };
          nodes.push(frame);
          const size = visit(node.branches[branch].block, childPath, laneId);
          frame.width = Math.max(size.width + 24, lane?.dimensions.width ?? 0); frame.height = Math.max(size.height + 24, lane?.dimensions.height ?? 0);
          frame.data.minSize = { width: size.width + 24, height: size.height + 24 };
          frames.push(frame); if (lane) positioned.add(laneId);
        }
        // Reserve existing positions first. New lanes and growth of an earlier
        // lane must not cover siblings before the author next organizes.
        const placed: SceneNode[] = [];
        const saved = frames.filter(frame => positioned.has(frame.id)).sort((a, b) => a.position.y - b.position.y || a.position.x - b.position.x);
        for (const frame of [...saved, ...frames.filter(frame => !positioned.has(frame.id))]) {
          let collisions: SceneNode[];
          while ((collisions = placed.filter(other => frame.position.x < other.position.x + other.width! && frame.position.x + frame.width! > other.position.x
            && frame.position.y < other.position.y + other.height! + 24 && frame.position.y + frame.height! + 24 > other.position.y)).length)
            frame.position = { ...frame.position, y: Math.max(...collisions.map(other => other.position.y + other.height! + 24)) };
          placed.push(frame);
        }
        for (const frame of frames) {
          minimum.width = Math.max(minimum.width, frame.position.x + frame.width! + 24);
        }
        minimum.height = Math.max(280, ...frames.map(frame => frame.position.y + frame.height! + 24));
      }
      const size = expanded ? metadata?.dimensions : measured.get(qualified);
      projected.width = Math.max(minimum.width, size?.width ?? 0); projected.height = Math.max(minimum.height, size?.height ?? 0);
      local.push(projected); x += projected.width + 96;
      if (node.type === 'agent' || node.type === 'model' && node.output.type === 'json_schema') projected.data.fields = schemaFields(node.type === 'agent' ? node.output : node.output.type === 'json_schema' ? node.output.schema : undefined);
      if (!expanded && projected.data.fields?.length) { minimum.height = Math.max(minimum.height, 110 + projected.data.fields.length * 22); projected.height = Math.max(projected.height, minimum.height); }
    }
    for (const note of document.editor?.notes ?? []) if (sameBlockPath(note.block, path)) local.push({ id: viewId(path, 'note', note.id), type: 'workflow', parentId,
      position: note.position, width: note.dimensions.width, height: note.dimensions.height, connectable: false,
      data: { view: 'note', path, local: note.id, title: note.id, note, minSize: { width: 160, height: 88 } } });
    // Nodes already pushed are container parents, so append only ordinary local
    // projections here. The later stable topological ordering is independent
    // of map insertion order and never changes executable edge order.
    for (const item of local) if (!nodes.includes(item)) nodes.push(item);
    for (const [index, edge] of edgesOf(block).entries()) if (nodeAt(block, edge.from) && nodeAt(block, edge.to)) edges.push({ id: viewId(path, 'edge', String(index)), source: viewId(path, 'node', edge.from), target: viewId(path, 'node', edge.to), sourceHandle: edgePort(edge), targetHandle: 'in', data: { path, index } });
    if (nodeAt(block, block.entry)) edges.push({ id: viewId(path, 'entry'), source: entryId, target: viewId(path, 'node', block.entry), sourceHandle: 'entry', targetHandle: 'in', deletable: false, reconnectable: true, data: { path, entry: true } });
    return { width: Math.max(400, ...local.map(node => node.position.x + (node.width ?? NODE_WIDTH))), height: Math.max(200, ...local.map(node => node.position.y + (node.height ?? 128))) };
  };
  visit(document.block, []);
  // A child container may have been emitted before its ancestor was appended.
  // Stable depth order guarantees parent-before-child, including private lanes.
  const depth = (node: SceneNode): number => node.parentId ? 1 + depth(nodes.find(parent => parent.id === node.parentId)!) : 0;
  nodes.sort((a, b) => depth(a) - depth(b));
  return { nodes, edges };
}
