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

/** Entry expressions read the enclosing block; round expressions read the
 * body. A separate body declaration binds references of that name locally.
 * An Assigner's written variable is a path too. */
function node(input: NodeDefinition, rewrite: PathRewrite): NodeDefinition {
  switch (input.type) {
    case 'start': case 'loop_end': return input;
    case 'code': case 'template': return { ...input, variables: Object.fromEntries(Object.entries(input.variables).map(([name,item]) => [name,value(item,rewrite)])) };
    case 'list': return { ...input, input: value(input.input,rewrite) };
    case 'classifier': return { ...input, input: value(input.input,rewrite), instruction: input.instruction?.map(part => value(part,rewrite)), classes: input.classes.map(item => ({...item,description:item.description.map(part => value(part,rewrite))})) };
    case 'aggregate': return { ...input, groups: input.groups.map(group => ({ ...group, variables: group.variables.map(path => rewrite(path) ?? path) })) };
    case 'agent': return { ...input, task: input.task.map(part => value(part, rewrite)) };
    case 'model': return { ...input, messages: input.messages.map(message => ({ ...message, content: message.content?.map(part => value(part, rewrite)) })) };
    case 'tool': return { ...input, arguments: value(input.arguments, rewrite) };
    case 'branch': return { ...input, cases: input.cases.map(item => ({ ...item, condition: predicate(item.condition, rewrite) })) };
    case 'review': return { ...input, content: input.content.map(part => value(part, rewrite)) };
    case 'loop':
    case 'iteration': {
      // Entry fields read the enclosing scope. Body declarations bind only
      // body reads and the container's round-completion expressions.
      const round: PathRewrite = path => path.length > 0 && Object.hasOwn(input.body.nodes, path[0]) ? undefined : rewrite(path);
      const body = rewriteBlockReferences(input.body, round);
      return input.type === 'loop'
        ? { ...input, loop_variables: input.loop_variables?.map(item => ({ ...item, value: value(item.value, rewrite) })),
          ...(input.break_condition ? { break_condition: predicate(input.break_condition, round) } : {}), body }
        : { ...input, iterator: value(input.iterator, rewrite), output: value(input.output, round), body };
    }
    case 'assigner': return { ...input, items: input.items.map(item => ({ ...item, variable: rewrite(item.variable) ?? item.variable, ...(item.value ? { value: value(item.value, rewrite) } : {}) })) };
    case 'return': return { ...input, outputs: input.outputs.map(item => ({ ...item, value_selector: rewrite(item.value_selector) ?? item.value_selector })) };
  }
}

/** Rewrite bindings in this scope and inherited by its bodies. Each body's
 * own declarations remain bound locally, including its completion expressions. */
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

/** References to `id`'s value: its own output, or a field of it. */
export const renameProducer = (from: string, to: string): PathRewrite =>
  path => path[0] === from ? [to, ...path.slice(1)] : undefined;

/** References whose producer is outside this block and its lexical bodies. */
export function blockReferences(block: Block): string[][] {
  const found: string[][] = [];
  rewriteBlockReferences(block, path => { if (!path.length || !Object.hasOwn(block.nodes, path[0])) found.push([...path]); return undefined; });
  return found;
}
