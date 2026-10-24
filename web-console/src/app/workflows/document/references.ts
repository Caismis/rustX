import type { Block, NodeDefinition, Predicate, Value } from './model';

/** Rewrite one structured reference path, never text. */
export type PathRewrite = (path: readonly string[]) => string[] | undefined;

function value(input: Value, rewrite: PathRewrite): Value {
  switch (input.type) {
    case 'reference': {
      const next = rewrite(input.path);
      return next ? { ...input, path: next } : input;
    }
    case 'literal': return input;
    case 'object': return { ...input, fields: Object.fromEntries(Object.entries(input.fields).map(([key, field]) => [key, value(field, rewrite)])) };
    case 'array': return { ...input, items: input.items.map(item => value(item, rewrite)) };
  }
}

function predicate(input: Predicate, rewrite: PathRewrite): Predicate {
  switch (input.type) {
    case 'boolean': return { ...input, value: value(input.value, rewrite) };
    case 'equal':
    case 'not_equal': return { ...input, left: value(input.left, rewrite), right: value(input.right, rewrite) };
    case 'not': return { ...input, predicate: predicate(input.predicate, rewrite) };
    case 'and':
    case 'or': return { ...input, predicates: input.predicates.map(item => predicate(item, rewrite)) };
  }
}

/** Rewrite every binding a node evaluates in its owning block's lexical
 * scope. Nested blocks are private scopes that only see their projected
 * `args`, and a Loop's `until`/`carry` see only its `result`; none of those
 * can name a sibling of the owning block, so none are rewritten. */
function node(input: NodeDefinition, rewrite: PathRewrite): NodeDefinition {
  switch (input.type) {
    case 'agent':
    case 'model': return { ...input, input: Object.fromEntries(Object.entries(input.input ?? {}).map(([key, binding]) => [key, value(binding, rewrite)])) };
    case 'tool': return { ...input, arguments: value(input.arguments, rewrite) };
    case 'branch': return { ...input, condition: predicate(input.condition, rewrite) };
    case 'parallel': return { ...input, branches: Object.fromEntries(Object.entries(input.branches).map(([key, branch]) => [key, { ...branch, input: value(branch.input, rewrite) }])) };
    case 'review': return { ...input, subject: { ...input.subject, value: value(input.subject.value, rewrite) }, context: input.context.map(item => value(item, rewrite)) };
    case 'loop': return { ...input, input: value(input.input, rewrite) };
    case 'return': return { ...input, output: value(input.output, rewrite) };
  }
}

/** Rewrite every reference evaluated in `block`'s own scope. */
export function rewriteBlockReferences(block: Block, rewrite: PathRewrite): Block {
  return { ...block, nodes: Object.fromEntries(Object.entries(block.nodes).map(([id, definition]) => [id, node(definition, rewrite)])) };
}

/** References to `id`'s value: its own output, or a field of it. */
export const renameProducer = (from: string, to: string): PathRewrite =>
  path => path[0] === from ? [to, ...path.slice(1)] : undefined;

/** References into Parallel `node`'s keyed output under branch `from`. */
export const renameBranchOutput = (node: string, from: string, to: string): PathRewrite =>
  path => path[0] === node && path[1] === from ? [node, to, ...path.slice(2)] : undefined;

/** Every reference path a block evaluates in its own scope. */
export function blockReferences(block: Block): string[][] {
  const found: string[][] = [];
  rewriteBlockReferences(block, path => { found.push([...path]); return undefined; });
  return found;
}
