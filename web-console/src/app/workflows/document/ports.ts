import type { Edge, NodeDefinition } from './model';

/** A native control port. `next` is the ordinary outgoing port, written
 * by omitting `port`. */
export type BuiltinPort = 'next' | 'true' | 'false' | 'fail';
export type ControlPort = string;

/** The exact native outgoing ports of each node kind. */
export function outputPorts(kind: NodeDefinition['type']): readonly ControlPort[] {
  switch (kind) {
    case 'branch': return ['true', 'false'];
    // As Dify's Exit Loop, it ends its round; Return ends the workflow.
    case 'loop_end': case 'return': return [];
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

/** Exits a Branch case or an Agent route cannot be named after. */
export const RESERVED_PORTS: readonly string[] = ['false', 'next', 'fail'];

export function portsOf(node: NodeDefinition): readonly ControlPort[] {
  if (node.type === 'branch') return [...node.cases.map(item => item.id), 'false'];
  // As Dify's Agent output routes: each route replaces `next` as an exit.
  const exits = node.type === 'agent' && node.routes?.length ? node.routes.map(route => route.id) : outputPorts(node.type);
  // As Dify's fail branch: a Model, Tool or Agent handling errors that way also exits on `fail`.
  return (node.type === 'model' || node.type === 'tool' || node.type === 'agent') && node.on_error?.type === 'fail_branch' ? [...exits, 'fail'] : exits;
}
