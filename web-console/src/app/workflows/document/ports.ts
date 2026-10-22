import type { Edge, NodeDefinition, NodeKind } from './model';

/** A native control port. `next` is the ordinary single successor, written
 * by omitting `port`. */
export type ControlPort = 'next' | 'true' | 'false' | 'satisfied' | 'exhausted';

/** The exact native outgoing ports of each node kind. Return ends its own
 * block and has none. */
export function outputPorts(kind: NodeKind): readonly ControlPort[] {
  switch (kind) {
    case 'branch': return ['true', 'false'];
    case 'loop': return ['satisfied', 'exhausted'];
    case 'return': return [];
    default: return ['next'];
  }
}

/** The port an authored edge leaves through. Native accepts YAML booleans
 * for Branch ports and an explicit `next`; both read as their port. */
export function edgePort(edge: Edge): ControlPort {
  const port = edge.port;
  if (port === undefined || port === null || port === 'next') return 'next';
  if (port === true) return 'true';
  if (port === false) return 'false';
  return port;
}

/** The authored form of a new edge on `port`. */
export function authoredEdge(from: string, port: ControlPort, to: string): Edge {
  return port === 'next' ? { from, to } : { from, to, port };
}

export function portsOf(node: NodeDefinition): readonly ControlPort[] {
  return outputPorts(node.type);
}
