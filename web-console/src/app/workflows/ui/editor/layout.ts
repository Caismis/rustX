import { edgesOf, nodeAt, type Block } from '../../document/model';
import { edgePort, outputPorts } from '../../document/ports';

export interface Point { readonly x: number; readonly y: number }
export const NODE_WIDTH = 256;
const COLUMN = 320;
const ROW = 170;

const PORT_ORDER = ['next', 'true', 'satisfied', 'false', 'exhausted'];

/** A deterministic layered layout of one block: columns by shortest
 * distance from the entry, rows by first discovery, following outgoing
 * ports in native order. Unreachable nodes follow in id order. */
export function defaultLayout(block: Block): Record<string, Point> {
  const ids = Object.keys(block.nodes).sort();
  const outgoing = new Map<string, string[]>();
  for (const edge of [...edgesOf(block)].sort((a, b) => PORT_ORDER.indexOf(edgePort(a)) - PORT_ORDER.indexOf(edgePort(b)))) {
    if (!nodeAt(block, edge.from) || !nodeAt(block, edge.to)) continue;
    outgoing.set(edge.from, [...outgoing.get(edge.from) ?? [], edge.to]);
  }
  const depth = new Map<string, number>();
  const order: string[] = [];
  const queue = nodeAt(block, block.entry) ? [block.entry] : [];
  if (queue.length) depth.set(block.entry, 0);
  while (queue.length) {
    const id = queue.shift()!;
    order.push(id);
    for (const next of outgoing.get(id) ?? []) {
      if (depth.has(next)) continue;
      depth.set(next, depth.get(id)! + 1);
      queue.push(next);
    }
  }
  const last = Math.max(-1, ...depth.values());
  for (const id of ids) if (!depth.has(id)) { depth.set(id, last + 1); order.push(id); }
  const columns = new Map<number, string[]>();
  for (const id of order) columns.set(depth.get(id)!, [...columns.get(depth.get(id)!) ?? [], id]);
  const positions: Record<string, Point> = {};
  for (const [column, members] of columns) {
    members.forEach((id, row) => { positions[id] = { x: column * COLUMN, y: (row - (members.length - 1) / 2) * ROW }; });
  }
  return positions;
}

/** Stored positions for still-existing node ids, defaults for the rest. */
export function layout(block: Block, stored: Readonly<Record<string, Point>> | undefined): Record<string, Point> {
  const defaults = defaultLayout(block);
  return Object.fromEntries(Object.keys(block.nodes).map(id => [id, stored?.[id] ?? defaults[id]]));
}

/** Where a node added after `from`'s `port` goes: one column right, offset by port. */
export function after(block: Block, positions: Readonly<Record<string, Point>>, from: string, port: string): Point {
  const origin = positions[from] ?? { x: 0, y: 0 };
  const source = nodeAt(block, from);
  const ports = source ? outputPorts(source.type) : [];
  const index = Math.max(0, ports.indexOf(port as never));
  return { x: origin.x + COLUMN, y: origin.y + (index - (ports.length - 1) / 2) * ROW };
}
