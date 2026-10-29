import { branchPresentation, deleteNodePresentation, renameNodePresentation, requireVacantBranchPresentation, requireVacantNodePresentation } from './presentation';
import {
  blockAt, branchAt, childSteps, edgesOf, nodeAt, sameBlockPath, updateBlock, type Block, type BlockPath, type Edge, type NodeDefinition, type NodeKind,
  type WorkflowDocument,
} from './model';
import { authoredEdge, edgePort, outputPorts, type ControlPort } from './ports';
import { renameBranchOutput, renameProducer, rewriteBlockReferences } from './references';

/** Why an editing command was refused. The document is unchanged. These are
 * canvas ergonomics only; the native compiler remains the graph authority. */
export type RejectionCode =
  | 'invalid_identifier' | 'reserved_identifier' | 'duplicate_identifier' | 'missing_node' | 'missing_branch'
  | 'missing_edge' | 'last_branch' | 'return_has_no_successor' | 'invalid_port' | 'port_occupied' | 'self_edge'
  | 'last_node' | 'missing_field' | 'invalid_fragment' | 'cross_scope' | 'limit_exceeded' | 'invalid_geometry' | 'unsupported_insertion' | 'pending_input';

export class CommandRejected extends Error {
  constructor(readonly code: RejectionCode) { super(code); this.name = 'CommandRejected'; }
}

/** Native local identity rule of node ids and Parallel branch keys. */
const IDENTIFIER = /^[A-Za-z0-9_-]{1,64}$/;
export function validIdentifier(value: string): RejectionCode | undefined {
  if (!IDENTIFIER.test(value)) return 'invalid_identifier';
  if (value === 'args') return 'reserved_identifier';
  return undefined;
}

const EMPTY_OBJECT = { type: 'object', properties: {}, additionalProperties: false } as const;

/** Native aggregate bounds, enforced before accepting structural growth. */
export function boundedDocument(document: WorkflowDocument): WorkflowDocument {
  let count = 0;
  const visit = (block: Block, depth: number) => {
    if (depth > 8) throw new CommandRejected('limit_exceeded');
    for (const [id, node] of Object.entries(block.nodes)) {
      if (++count > 256 || node.type === 'parallel' && Object.keys(node.branches).length > 32) throw new CommandRejected('limit_exceeded');
      for (const step of childSteps(id, node)) visit(step.branch === undefined ? (node as Extract<NodeDefinition, { type: 'loop' }>).body : (node as Extract<NodeDefinition, { type: 'parallel' }>).branches[step.branch].block, depth + 1);
    }
  };
  visit(document.block, 0);
  return document;
}

/** A private nested block that returns an empty object. */
export function emptyBlock(): Block {
  return { input: { ...EMPTY_OBJECT }, output: { ...EMPTY_OBJECT }, entry: 'done', nodes: { done: { type: 'return', output: { type: 'literal', value: {} } } }, edges: [] };
}

/** The editable starting definition of each native node kind. Incomplete
 * fields (an unchosen profile or Tool) are left for the author and reported
 * by the native check; nothing is invented to make a graph pass. */
export function nodeTemplate(kind: NodeKind): NodeDefinition {
  switch (kind) {
    case 'agent': return { type: 'agent', profile: '', task: '', input: {}, output: { ...EMPTY_OBJECT } };
    case 'model': return { type: 'model', model: { mode: 'session' }, instruction: '', input: {}, output: { type: 'text' } };
    case 'tool': return { type: 'tool', selector: { origin: 'builtin', name: '' }, arguments: { type: 'literal', value: {} }, result: { type: 'text', part: 0 } };
    case 'branch': return { type: 'branch', condition: { type: 'boolean', value: { type: 'literal', value: true } } };
    case 'parallel': return { type: 'parallel', branches: { main: { input: { type: 'literal', value: {} }, block: emptyBlock() } } };
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
  if (!outputPorts(source.type).includes(port)) throw new CommandRejected('invalid_port');
  if (from === to) throw new CommandRejected('self_edge');
  if (edgesOf(block).some((edge, index) => index !== ignore && edge.from === from && edgePort(edge) === port)) throw new CommandRejected('port_occupied');
}

export interface Connection { readonly from: string; readonly port: ControlPort }

/** Add one node, optionally connected from an eligible free port. */
export function addNode(document: WorkflowDocument, path: BlockPath, kind: NodeKind, id: string, from?: Connection): WorkflowDocument {
  const block = require(blockAt(document, path));
  const invalid = validIdentifier(id);
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
  if (outputPorts(kind).length !== 1 || outputPorts(kind)[0] !== 'next') throw new CommandRejected('unsupported_insertion');
  let next = disconnect(document, path, [index]);
  next = addNode(next, path, kind, id, { from: edge.from, port: edgePort(edge) });
  return connect(next, path, id, 'next', edge.to);
}

/** Delete nodes and the edges that touch them. The entry marker and any
 * reference to a deleted value stay as authored, for the native check to
 * report; nothing is rewired. */
export function deleteNodes(document: WorkflowDocument, path: BlockPath, ids: readonly string[]): WorkflowDocument {
  const block = require(blockAt(document, path));
  const doomed = new Set(ids.filter(id => nodeAt(block, id)));
  if (!doomed.size) throw new CommandRejected('missing_node');
  if (doomed.size === Object.keys(block.nodes).length) throw new CommandRejected('last_node');
  const next = updateBlock(document, path, current => ({
    ...current,
    nodes: Object.fromEntries(Object.entries(current.nodes).filter(([id]) => !doomed.has(id))),
    edges: edgesOf(current).filter(edge => !doomed.has(edge.from) && !doomed.has(edge.to)),
  }));
  return deleteNodePresentation(next, path, doomed);
}

/** Rename one node and every structured identity naming it in its lexical
 * block: edges, entry and references. Task text is never touched. */
export function renameNode(document: WorkflowDocument, path: BlockPath, from: string, to: string): WorkflowDocument {
  const block = require(blockAt(document, path));
  if (!nodeAt(block, from)) throw new CommandRejected('missing_node');
  if (from === to) return document;
  const invalid = validIdentifier(to);
  if (invalid) throw new CommandRejected(invalid);
  if (nodeAt(block, to)) throw new CommandRejected('duplicate_identifier');
  requireVacantNodePresentation(document, path, to);
  const next = updateBlock(document, path, current => {
    const renamed = rewriteBlockReferences(current, renameProducer(from, to));
    return {
      ...renamed,
      entry: renamed.entry === from ? to : renamed.entry,
      nodes: Object.fromEntries(Object.entries(renamed.nodes).map(([id, node]) => [id === from ? to : id, node])),
      edges: edgesOf(renamed).map(edge => ({ ...edge, from: edge.from === from ? to : edge.from, to: edge.to === from ? to : edge.to })),
    };
  });
  return renameNodePresentation(next, path, from, to);
}

export function setEntry(document: WorkflowDocument, path: BlockPath, id: string): WorkflowDocument {
  const block = require(blockAt(document, path));
  if (!nodeAt(block, id)) throw new CommandRejected('missing_node');
  return updateBlock(document, path, current => ({ ...current, entry: id }));
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

function parallel(block: Block, id: string) {
  const node = nodeAt(block, id);
  if (!node || node.type !== 'parallel') throw new CommandRejected('missing_node');
  return node;
}

export function addBranch(document: WorkflowDocument, path: BlockPath, id: string, key: string): WorkflowDocument {
  const node = parallel(require(blockAt(document, path)), id);
  const invalid = validIdentifier(key);
  if (invalid) throw new CommandRejected(invalid);
  if (branchAt(node.branches, key)) throw new CommandRejected('duplicate_identifier');
  requireVacantBranchPresentation(document, path, id, key);
  return boundedDocument(updateBlock(document, path, block => ({ ...block, nodes: { ...block.nodes, [id]: { ...node, branches: { ...node.branches, [key]: { input: { type: 'literal', value: {} }, block: emptyBlock() } } } } })));
}

/** Rename a branch key and every reference into its keyed output in the
 * owning block. */
export function renameBranch(document: WorkflowDocument, path: BlockPath, id: string, from: string, to: string): WorkflowDocument {
  const node = parallel(require(blockAt(document, path)), id);
  if (!branchAt(node.branches, from)) throw new CommandRejected('missing_branch');
  if (from === to) return document;
  const invalid = validIdentifier(to);
  if (invalid) throw new CommandRejected(invalid);
  if (branchAt(node.branches, to)) throw new CommandRejected('duplicate_identifier');
  requireVacantBranchPresentation(document, path, id, to);
  const next = updateBlock(document, path, block => {
    const renamed = rewriteBlockReferences(block, renameBranchOutput(id, from, to));
    const owner = parallel(renamed, id);
    return { ...renamed, nodes: { ...renamed.nodes, [id]: { ...owner, branches: Object.fromEntries(Object.entries(owner.branches).map(([key, branch]) => [key === from ? to : key, branch])) } } };
  });
  return branchPresentation(next, path, id, from, to);
}

export function removeBranch(document: WorkflowDocument, path: BlockPath, id: string, key: string): WorkflowDocument {
  const node = parallel(require(blockAt(document, path)), id);
  if (!branchAt(node.branches, key)) throw new CommandRejected('missing_branch');
  if (Object.keys(node.branches).length === 1) throw new CommandRejected('last_branch');
  const next = updateBlock(document, path, block => ({ ...block, nodes: { ...block.nodes, [id]: { ...node, branches: Object.fromEntries(Object.entries(node.branches).filter(([branch]) => branch !== key)) } } }));
  return branchPresentation(next, path, id, key);
}

/** Set the Workflow description and total timeout. An omitted timeout keeps
 * the native default. */
export function setWorkflow(document: WorkflowDocument, fields: { description: string; timeout_ms?: string }): WorkflowDocument {
  const { timeout_ms: _previous, ...rest } = document;
  return fields.timeout_ms === undefined ? { ...rest, description: fields.description } : { ...rest, description: fields.description, timeout_ms: fields.timeout_ms };
}
