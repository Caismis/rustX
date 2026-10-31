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
    case 'template': return { ...input, parts: input.parts.map(part => value(part, rewrite)) };
  }
}

function predicate(input: Predicate, rewrite: PathRewrite): Predicate {
  switch (input.type) {
    case 'boolean': return { ...input, value: value(input.value, rewrite) };
    case 'equal':
    case 'not_equal':
    case 'contains':
    case 'not_contains':
    case 'starts_with':
    case 'ends_with': return { ...input, left: value(input.left, rewrite), right: value(input.right, rewrite) };
    case 'empty':
    case 'not_empty': return { ...input, value: value(input.value, rewrite) };
    case 'not': return { ...input, predicate: predicate(input.predicate, rewrite) };
    case 'and':
    case 'or': return { ...input, predicates: input.predicates.map(item => predicate(item, rewrite)) };
  }
}

/** Rewrite every binding a node evaluates, including those of its body. As
 * in Dify, a body reads the variables around it and node IDs never hide one
 * another, so one rewrite covers the whole subtree. An Assigner's written
 * variable is a path too. */
function node(input: NodeDefinition, rewrite: PathRewrite): NodeDefinition {
  switch (input.type) {
    case 'start': case 'loop_end': return input;
    case 'agent': return { ...input, task: input.task.map(part => value(part, rewrite)) };
    case 'model': return { ...input, messages: input.messages.map(message => ({ ...message, content: message.content?.map(part => value(part, rewrite)) })) };
    case 'tool': return { ...input, arguments: value(input.arguments, rewrite) };
    case 'branch': return { ...input, cases: input.cases.map(item => ({ ...item, condition: predicate(item.condition, rewrite) })) };
    case 'review': return { ...input, content: input.content.map(part => value(part, rewrite)) };
    case 'loop': return { ...input, loop_variables: input.loop_variables?.map(item => ({ ...item, value: value(item.value, rewrite) })),
      ...(input.break_condition ? { break_condition: predicate(input.break_condition, rewrite) } : {}), body: rewriteBlockReferences(input.body, rewrite) };
    case 'iteration': return { ...input, iterator: value(input.iterator, rewrite), output: value(input.output, rewrite), body: rewriteBlockReferences(input.body, rewrite) };
    case 'assigner': return { ...input, items: input.items.map(item => ({ ...item, variable: rewrite(item.variable) ?? item.variable, ...(item.value ? { value: value(item.value, rewrite) } : {}) })) };
    case 'return': return { ...input, output: value(input.output, rewrite) };
  }
}

/** Rewrite every reference evaluated in `block` or a body inside it. */
export function rewriteBlockReferences(block: Block, rewrite: PathRewrite): Block {
  return { ...block, nodes: Object.fromEntries(Object.entries(block.nodes).map(([id, definition]) => [id, node(definition, rewrite)])) };
}

/** A container's fields that read its body after a round: the Loop's break
 * condition and the Iteration's output. */
export function rewriteRoundReferences(container: NodeDefinition, rewrite: PathRewrite): NodeDefinition {
  if (container.type === 'loop' && container.break_condition) return { ...container, break_condition: predicate(container.break_condition, rewrite) };
  if (container.type === 'iteration') return { ...container, output: value(container.output, rewrite) };
  return container;
}

/** Node IDs declared in the bodies inside `block`, at any depth. */
function bodyIdentifiers(block: Block, found = new Set<string>()): Set<string> {
  for (const definition of Object.values(block.nodes)) if (definition.type === 'loop' || definition.type === 'iteration') {
    for (const id of Object.keys(definition.body.nodes)) found.add(id);
    bodyIdentifiers(definition.body, found);
  }
  return found;
}

/** References to `id`'s value: its own output, or a field of it. */
export const renameProducer = (from: string, to: string): PathRewrite =>
  path => path[0] === from ? [to, ...path.slice(1)] : undefined;



/** Every reference a block reads from outside it: references to its
 * bodies' own nodes stay inside. */
export function blockReferences(block: Block): string[][] {
  const found: string[][] = [], inside = bodyIdentifiers(block);
  rewriteBlockReferences(block, path => { if (!inside.has(path[0])) found.push([...path]); return undefined; });
  return found;
}
