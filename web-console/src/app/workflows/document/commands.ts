import { deleteNodePresentation, renameNodePresentation, requireVacantNodePresentation } from './presentation';
import {
  blockAt, edgesOf, nodeAt, sameBlockPath, updateBlock, type Block, type BlockPath, type Edge, type NodeDefinition, type NodeKind,
  type WorkflowDocument,
} from './model';
import { authoredEdge, edgePort, outputPorts, portsOf, RESERVED_PORTS, type ControlPort } from './ports';
import { renameProducer, rewriteBlockReferences } from './references';

/** Why an editing command was refused. The document is unchanged. These are
 * canvas ergonomics only; the native compiler remains the graph authority. */
export type RejectionCode =
  | 'invalid_identifier' | 'reserved_identifier' | 'reserved_port' | 'duplicate_identifier' | 'missing_node'
  | 'missing_edge' | 'return_has_no_successor' | 'invalid_port' | 'duplicate_edge' | 'self_edge'
  | 'last_node' | 'missing_field' | 'invalid_fragment' | 'cross_scope' | 'limit_exceeded' | 'invalid_geometry' | 'unsupported_insertion' | 'unsupported_schema_rename' | 'pending_input';

export class CommandRejected extends Error {
  constructor(readonly code: RejectionCode) { super(code); this.name = 'CommandRejected'; }
}

/** Native local keys; node IDs reserve args. */
export function validLocalKey(value: string): RejectionCode | undefined {
  return !value.length || value.length > 64 || /[^A-Za-z0-9_-]/.test(value) ? 'invalid_identifier' : undefined;
}
export function validNodeId(value: string): RejectionCode | undefined {
  return validLocalKey(value) ?? (value === 'args' ? 'reserved_identifier' : undefined);
}

const EMPTY_OBJECT = { type: 'object', properties: {}, additionalProperties: false } as const;

/** Native aggregate bounds, enforced before accepting structural growth. */
export function boundedDocument(document: WorkflowDocument): WorkflowDocument {
  let count = 0;
  const visit = (block: Block, depth: number) => {
    if (depth > 8) throw new CommandRejected('limit_exceeded');
    for (const node of Object.values(block.nodes)) {
      if (++count > 256) throw new CommandRejected('limit_exceeded');
      if (node.type === 'loop') visit(node.body, depth + 1);
    }
  };
  visit(document.block, 0);
  return document;
}

/** A private nested block that returns an empty object. */
export function emptyBlock(): Block {
  return { input: { ...EMPTY_OBJECT }, output: { ...EMPTY_OBJECT }, nodes: { start: { type: 'start' }, done: { type: 'return', output: { type: 'literal', value: {} } } }, edges: [{ from: 'start', to: 'done' }] };
}

/** The editable starting definition of each native node kind. Incomplete
 * fields (an unchosen profile or Tool) are left for the author and reported
 * by the native check; nothing is invented to make a graph pass. */
export function nodeTemplate(kind: NodeKind): NodeDefinition {
  switch (kind) {
    // As Dify's Agent: the final reply is its `text` until outputs are declared.
    case 'agent': return { type: 'agent', profile: '', task: [], output: { type: 'text' } };
    // As Dify's LLM node: an empty system prompt, then an empty user turn.
    case 'model': return { type: 'model', model: { mode: 'session' }, messages: [{ role: 'system', content: [] }, { role: 'user', content: [] }], output: { type: 'text' } };
    case 'tool': return { type: 'tool', selector: { origin: 'builtin', name: '' }, arguments: { type: 'object', fields: {} }, result: { type: 'text' } };
    case 'branch': return { type: 'branch', cases: [{ id: 'true', condition: { type: 'and', predicates: [] } }] };
    case 'review': return { type: 'review', subject: { type: 'plan', value: { type: 'reference', path: ['args'] } }, context: [] };
    case 'loop': return { type: 'loop', input: { type: 'literal', value: {} }, body: emptyBlock(), until: { type: 'boolean', value: { type: 'literal', value: true } }, carry: { type: 'reference', path: ['result'] }, max_iterations: 3 };
    case 'return': return { type: 'return', output: { type: 'literal', value: {} } };
  }
}

/** The first free `<kind>`, `<kind>_2`, … id in a block. */
export function freshIdentifier(block: Block, base: string): string {
  if (!nodeAt(block, base)) return base;
  for (let index = 2; ; index++) if (!nodeAt(block, `${base}_${index}`)) return `${base}_${index}`;
}

function require(block: Block | undefined): Block {
  if (!block) throw new CommandRejected('missing_node');
  return block;
}

function withEdges(block: Block, edges: Edge[]): Block {
  return { ...block, edges };
}

function checkConnection(block: Block, from: string, port: ControlPort, to: string, ignore?: number) {
  const source = nodeAt(block, from);
  if (!source || !nodeAt(block, to)) throw new CommandRejected('missing_node');
  if (source.type === 'return') throw new CommandRejected('return_has_no_successor');
  if (!portsOf(source).includes(port)) throw new CommandRejected('invalid_port');
  if (nodeAt(block, to)?.type === 'start') throw new CommandRejected('invalid_port');
  if (from === to) throw new CommandRejected('self_edge');
  if (edgesOf(block).some((edge, index) => index !== ignore && edge.from === from && edgePort(edge) === port && edge.to === to)) throw new CommandRejected('duplicate_edge');
}

export interface Connection { readonly from: string; readonly port: ControlPort }

/** Add one node, optionally connected from an eligible port. */
export function addNode(document: WorkflowDocument, path: BlockPath, kind: NodeKind, id: string, from?: Connection): WorkflowDocument {
  const block = require(blockAt(document, path));
  const invalid = validNodeId(id);
  if (invalid) throw new CommandRejected(invalid);
  if (nodeAt(block, id)) throw new CommandRejected('duplicate_identifier');
  requireVacantNodePresentation(document, path, id);
  const added = { ...block, nodes: { ...block.nodes, [id]: nodeTemplate(kind) } };
  if (from) checkConnection(added, from.from, from.port, id);
  return boundedDocument(updateBlock(document, path, () => from ? withEdges(added, [...edgesOf(added), authoredEdge(from.from, from.port, id)]) : added));
}

/** Inserting into an edge is a single transaction. A single-exit node can
 * preserve the existing continuation; multi-exit and terminal nodes cannot. */
export function insertNode(document: WorkflowDocument, path: BlockPath, index: number, kind: NodeKind, id: string): WorkflowDocument {
  const block = require(blockAt(document, path)), edge = edgesOf(block)[index];
  if (!edge) throw new CommandRejected('missing_edge');
  requireSingleExit(kind);
  let next = disconnect(document, path, [index]);
  next = addNode(next, path, kind, id, { from: edge.from, port: edgePort(edge) });
  return connect(next, path, id, 'next', edge.to);
}

/** As Dify's input-side "+": a new single-exit node continues to `to`. */
export function addBefore(document: WorkflowDocument, path: BlockPath, kind: NodeKind, id: string, to: string): WorkflowDocument {
  requireSingleExit(kind);
  if (!nodeAt(require(blockAt(document, path)), to)) throw new CommandRejected('missing_node');
  return connect(addNode(document, path, kind, id), path, id, 'next', to);
}

function requireSingleExit(kind: NodeKind) {
  if (outputPorts(kind).length !== 1 || outputPorts(kind)[0] !== 'next') throw new CommandRejected('unsupported_insertion');
}

/** Delete nodes and the edges that touch them. References to deleted
 * values stay as authored for the native check to report; nothing is rewired. */
export function deleteNodes(document: WorkflowDocument, path: BlockPath, ids: readonly string[]): WorkflowDocument {
  const block = require(blockAt(document, path));
  const doomed = new Set(ids.filter(id => nodeAt(block, id)));
  if (!doomed.size) throw new CommandRejected('missing_node');
  if ([...doomed].some(id => block.nodes[id].type === 'start')) throw new CommandRejected('invalid_port');
  if (doomed.size === Object.keys(block.nodes).length) throw new CommandRejected('last_node');
  const next = updateBlock(document, path, current => ({
    ...current,
    nodes: Object.fromEntries(Object.entries(current.nodes).filter(([id]) => !doomed.has(id))),
    edges: edgesOf(current).filter(edge => !doomed.has(edge.from) && !doomed.has(edge.to)),
  }));
  return deleteNodePresentation(next, path, doomed);
}

/** Rename one node and every structured identity naming it in its lexical
 * block: edges and references. Task text is never touched. */
export function renameNode(document: WorkflowDocument, path: BlockPath, from: string, to: string): WorkflowDocument {
  const block = require(blockAt(document, path));
  if (!nodeAt(block, from)) throw new CommandRejected('missing_node');
  if (from === to) return document;
  const invalid = validNodeId(to);
  if (invalid) throw new CommandRejected(invalid);
  if (nodeAt(block, to)) throw new CommandRejected('duplicate_identifier');
  requireVacantNodePresentation(document, path, to);
  const next = updateBlock(document, path, current => {
    const renamed = rewriteBlockReferences(current, renameProducer(from, to));
    return {
      ...renamed,
      nodes: Object.fromEntries(Object.entries(renamed.nodes).map(([id, node]) => [id === from ? to : id, node])),
      edges: edgesOf(renamed).map(edge => ({ ...edge, from: edge.from === from ? to : edge.from, to: edge.to === from ? to : edge.to })),
    };
  });
  return renameNodePresentation(next, path, from, to);
}

export function connect(document: WorkflowDocument, path: BlockPath, from: string, port: ControlPort, to: string): WorkflowDocument {
  const block = require(blockAt(document, path));
  checkConnection(block, from, port, to);
  return updateBlock(document, path, current => withEdges(current, [...edgesOf(current), authoredEdge(from, port, to)]));
}

/** Addressed endpoints cannot bind across private native Blocks, even when
 * both scopes contain the same local id or the same renderer parent. */
export function connectAddressed(document: WorkflowDocument, from: { block: BlockPath; node: string; port: ControlPort }, to: { block: BlockPath; node: string }): WorkflowDocument {
  if (!sameBlockPath(from.block, to.block)) throw new CommandRejected('cross_scope');
  return connect(document, from.block, from.node, from.port, to.node);
}

export function disconnect(document: WorkflowDocument, path: BlockPath, indexes: readonly number[]): WorkflowDocument {
  const block = require(blockAt(document, path));
  const removed = new Set(indexes);
  if (indexes.some(index => !edgesOf(block)[index])) throw new CommandRejected('missing_edge');
  return updateBlock(document, path, current => withEdges(current, edgesOf(current).filter((_, index) => !removed.has(index))));
}

/** Move one edge's source port or destination, keeping its position. */
export function reconnect(document: WorkflowDocument, path: BlockPath, index: number, from: string, port: ControlPort, to: string): WorkflowDocument {
  const block = require(blockAt(document, path));
  if (!edgesOf(block)[index]) throw new CommandRejected('missing_edge');
  checkConnection(block, from, port, to, index);
  return updateBlock(document, path, current => withEdges(current, edgesOf(current).map((edge, at) => at === index ? authoredEdge(from, port, to) : edge)));
}

/** Replace one node's definition from its inspector. A change of kind is
 * not an edit; the inspector edits only the node's own fields. */
export function replaceNode(document: WorkflowDocument, path: BlockPath, id: string, node: NodeDefinition): WorkflowDocument {
  const block = require(blockAt(document, path));
  const current = nodeAt(block, id);
  if (!current || current.type !== node.type) throw new CommandRejected('missing_node');
  return updateBlock(document, path, owner => ({ ...owner, nodes: { ...owner.nodes, [id]: node } }));
}

/** Set the Workflow description and total timeout. An omitted timeout uses the native default. */
export function setWorkflow(document: WorkflowDocument, fields: { description: string; timeout_ms?: string }): WorkflowDocument {
  const { timeout_ms: _previous, ...rest } = document;
  return fields.timeout_ms === undefined ? { ...rest, description: fields.description } : { ...rest, description: fields.description, timeout_ms: fields.timeout_ms };
}

/** Cases keep stable exit IDs while their order defines IF / ELIF precedence. */
export function addBranchCase(document: WorkflowDocument, path: BlockPath, id: string): WorkflowDocument {
  return updateBlock(document, path, block => {
    const node = nodeAt(block, id);
    if (node?.type !== 'branch') throw new CommandRejected('missing_node');
    if (node.cases.length >= 32) throw new CommandRejected('limit_exceeded');
    let index = 2; while (node.cases.some(item => item.id === `case_${index}`)) index++;
    return { ...block, nodes: { ...block.nodes, [id]: { ...node, cases: [...node.cases, { id: `case_${index}`, condition: { type: 'and', predicates: [] } }] } } };
  });
}
export function removeBranchCase(document: WorkflowDocument, path: BlockPath, id: string, caseId: string): WorkflowDocument {
  return updateBlock(document, path, block => {
    const node = nodeAt(block, id);
    if (node?.type !== 'branch' || node.cases.length < 2 || !node.cases.some(item => item.id === caseId)) throw new CommandRejected('missing_field');
    return { ...block, nodes: { ...block.nodes, [id]: { ...node, cases: node.cases.filter(item => item.id !== caseId) } },
      edges: edgesOf(block).filter(edge => edge.from !== id || edgePort(edge) !== caseId) };
  });
}

/** Replace one node and drop its connections on exits it no longer has, as
 * Dify does when routes or a fail branch go away. */
export function replaceNodeExits(document: WorkflowDocument, path: BlockPath, id: string, node: NodeDefinition): WorkflowDocument {
  const next = replaceNode(document, path, id, node), ports = new Set(portsOf(node));
  return updateBlock(next, path, block => withEdges(block, edgesOf(block).filter(edge => edge.from !== id || ports.has(edgePort(edge)))));
}

/** A route ID is what the child answers with, so it moves its connections. */
export function renameAgentRoute(document: WorkflowDocument, path: BlockPath, id: string, from: string, to: string): WorkflowDocument {
  return updateBlock(document, path, block => {
    const node = nodeAt(block, id);
    if (node?.type !== 'agent' || !node.routes?.some(route => route.id === from)) throw new CommandRejected('missing_field');
    if (from === to) return block;
    const invalid = validLocalKey(to);
    if (invalid) throw new CommandRejected(invalid);
    if (RESERVED_PORTS.includes(to)) throw new CommandRejected('reserved_port');
    if (node.routes.some(route => route.id === to)) throw new CommandRejected('duplicate_identifier');
    return { ...block, nodes: { ...block.nodes, [id]: { ...node, routes: node.routes.map(route => route.id === from ? { ...route, id: to } : route) } },
      edges: edgesOf(block).map(edge => edge.from === id && edgePort(edge) === from ? authoredEdge(id, to, edge.to) : edge) };
  });
}
