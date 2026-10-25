import type {
  WorkflowBlock, WorkflowBranch, WorkflowDefinition, WorkflowEdgeDefinition, WorkflowNodeDefinition,
  WorkflowPredicate, WorkflowValue,
} from '../../../../../protocol/app-server/v57';

/** The one browser editing document: exactly the native authoring type.
 * Canvas nodes, edges and inspector forms are projections of it. */
export type WorkflowDocument = WorkflowDefinition;
export type Block = WorkflowBlock;
export type NodeDefinition = WorkflowNodeDefinition;
export type NodeKind = NodeDefinition['type'];
export type Value = WorkflowValue;
export type Predicate = WorkflowPredicate;
export type Edge = WorkflowEdgeDefinition;
export type Branch = WorkflowBranch;

/** The eight native node kinds, in palette order. */
export const NODE_KINDS = ['agent', 'model', 'tool', 'branch', 'parallel', 'review', 'loop', 'return'] as const satisfies readonly NodeKind[];

/** One lexical step into a nested block: a Parallel branch (`branch`) or a
 * Loop body (no `branch`). */
export interface BlockStep { readonly node: string; readonly branch?: string }
/** The lexical block being edited; empty is the root block. */
export type BlockPath = readonly BlockStep[];

export const ROOT: BlockPath = [];

/** Stable presentation key of a block path: `root`, `reviews/quality`,
 * `repair/body`. Node ids and branch keys contain no `/`. */
export function blockKey(path: BlockPath): string {
  return path.length ? path.map(step => `${step.node}/${step.branch ?? 'body'}`).join('/') : 'root';
}

/** The native authored path of a block: `block`, `block.nodes.r.body`, … */
export function nativeBlockPath(path: BlockPath): string {
  return path.reduce((native, step) => step.branch === undefined
    ? `${native}.nodes.${step.node}.body`
    : `${native}.nodes.${step.node}.branches.${step.branch}.block`, 'block');
}

export function sameBlockPath(left: BlockPath, right: BlockPath): boolean {
  return left.length === right.length && left.every((step, index) => step.node === right[index].node && step.branch === right[index].branch);
}

/** The node authored under `id`. Authored maps are plain objects, so only
 * an own entry counts: a valid identifier such as `constructor` is never
 * mistaken for an inherited property. */
export function nodeAt(block: Block, id: string): NodeDefinition | undefined {
  return Object.hasOwn(block.nodes, id) ? block.nodes[id] : undefined;
}

/** The Parallel branch authored under `key`, by own entry only. */
export function branchAt<B>(branches: Readonly<Record<string, B>>, key: string): B | undefined {
  return Object.hasOwn(branches, key) ? branches[key] : undefined;
}

function child(block: Block, step: BlockStep): Block | undefined {
  const node = nodeAt(block, step.node);
  if (!node) return undefined;
  if (step.branch === undefined) return node.type === 'loop' ? node.body : undefined;
  return node.type === 'parallel' ? branchAt(node.branches, step.branch)?.block : undefined;
}

/** The block at `path`, or `undefined` when an ancestor no longer owns it. */
export function blockAt(document: WorkflowDocument, path: BlockPath): Block | undefined {
  let block: Block | undefined = document.block;
  for (const step of path) {
    block = block && child(block, step);
  }
  return block;
}

/** The deepest prefix of `path` that still names a block. */
export function existingPath(document: WorkflowDocument, path: BlockPath): BlockPath {
  let block: Block | undefined = document.block;
  let depth = 0;
  for (const step of path) {
    block = block && child(block, step);
    if (!block) break;
    depth++;
  }
  return path.slice(0, depth);
}

/** Replace the block at `path`, copying only its ancestors. */
export function updateBlock(document: WorkflowDocument, path: BlockPath, update: (block: Block) => Block): WorkflowDocument {
  const rewrite = (block: Block, depth: number): Block => {
    if (depth === path.length) return update(block);
    const step = path[depth];
    const node = nodeAt(block, step.node);
    if (!node) throw new Error('The edited block no longer exists.');
    let replaced: NodeDefinition;
    if (step.branch === undefined) {
      if (node.type !== 'loop') throw new Error('The edited block no longer exists.');
      replaced = { ...node, body: rewrite(node.body, depth + 1) };
    } else {
      const branch = node.type === 'parallel' ? branchAt(node.branches, step.branch) : undefined;
      if (node.type !== 'parallel' || !branch) throw new Error('The edited block no longer exists.');
      replaced = { ...node, branches: { ...node.branches, [step.branch]: { ...branch, block: rewrite(branch.block, depth + 1) } } };
    }
    return { ...block, nodes: { ...block.nodes, [step.node]: replaced } };
  };
  return { ...document, block: rewrite(document.block, 0) };
}

export function edgesOf(block: Block): Edge[] {
  return block.edges ?? [];
}

/** The nested blocks a node owns, as single steps from its block. */
export function childSteps(id: string, node: NodeDefinition): BlockStep[] {
  if (node.type === 'loop') return [{ node: id }];
  if (node.type === 'parallel') return Object.keys(node.branches).sort().map(branch => ({ node: id, branch }));
  return [];
}

/** Total node count of a block, including nested blocks. */
export function deepNodeCount(block: Block): number {
  return Object.entries(block.nodes).reduce((count, [id, node]) => count + 1
    + childSteps(id, node).reduce((nested, step) => nested + deepNodeCount(child(block, step)!), 0), 0);
}

/** Deterministic structural equality of two documents. Object key order is
 * irrelevant to native semantics, so it is irrelevant here. */
export function sameDocument(left: unknown, right: unknown): boolean {
  if (left === right) return true;
  if (typeof left !== typeof right || left === null || right === null || typeof left !== 'object') return false;
  if (Array.isArray(left) !== Array.isArray(right)) return false;
  if (Array.isArray(left)) {
    const other = right as unknown[];
    return left.length === other.length && left.every((item, index) => sameDocument(item, other[index]));
  }
  const a = Object.entries(left as Record<string, unknown>).filter(([, value]) => value !== undefined);
  const b = right as Record<string, unknown>;
  return a.length === Object.values(b).filter(value => value !== undefined).length
    && a.every(([key, value]) => Object.hasOwn(b, key) && sameDocument(value, b[key]));
}
