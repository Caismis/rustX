import type { WorkflowOrderTarget, WorkflowSchemaStep } from '../../../../../protocol/app-server/v62';
import { CommandRejected } from './commands';
import { fieldValue, setField, type FieldAddress, type FieldStep } from './fields';
import { blockAt, nodeAt, sameDocument, updateBlock, type BlockPath, type Predicate, type Value, type WorkflowDocument } from './model';
import { setOrder } from './geometry';
import { orderedKeys } from './presentation';
import { rewriteBlockReferences, type PathRewrite } from './references';

export type Schema = Record<string, unknown>;
export const schemaObject = (value: unknown): Schema | undefined => value !== null && typeof value === 'object' && !Array.isArray(value) ? value as Schema : undefined;
export const schemaTypes = ['string', 'integer', 'number', 'boolean', 'object', 'array', 'null'] as const;
export function schemaTemplate(type: string): Schema {
  if (type === 'object') return { type, properties: {}, required: [], additionalProperties: false };
  if (type === 'array') return { type, items: { type: 'string' } };
  return { type };
}

/** The metadata target of exactly this schema, independent of JSON map order. */
export function schemaTarget(address: FieldAddress): Extract<WorkflowOrderTarget, { path: WorkflowSchemaStep[] }> | undefined {
  const parts = [...address.path], first = parts.shift();
  let kind: 'block_input' | 'block_output' | 'node_output';
  if (!address.node && (first === 'input' || first === 'output')) kind = first === 'input' ? 'block_input' : 'block_output';
  else if (address.node && first === 'output') { kind = 'node_output'; if (parts[0] === 'schema') parts.shift(); }
  else if (address.node && first === 'result' && parts.shift() === 'schema') kind = 'node_output';
  else return undefined;
  const path: WorkflowSchemaStep[] = [];
  while (parts.length) {
    const step = parts.shift();
    if (step === 'items') path.push({ kind: 'items' });
    else if (step === 'properties') { const key = parts.shift(); if (typeof key !== 'object') return undefined; path.push({ kind: 'property', name: key.key }); }
    else return undefined;
  }
  return kind === 'node_output' ? { kind, address: { block: [...address.block], node: address.node! }, path } : { kind, block: [...address.block], path };
}

export function patchSchema(document: WorkflowDocument, address: FieldAddress, patch: Schema): WorkflowDocument {
  const value = schemaObject(fieldValue(document, address)?.value);
  if (!value) throw new CommandRejected('missing_field');
  return setField(document, address, { ...value, ...patch });
}

function mapValue(value: Value, rewrite: PathRewrite): Value {
  if (value.type === 'reference') return { ...value, path: rewrite(value.path) ?? value.path };
  if (value.type === 'object') return { ...value, fields: Object.fromEntries(Object.entries(value.fields).map(([key, field]) => [key, mapValue(field, rewrite)])) };
  if (value.type === 'array') return { ...value, items: value.items.map(field => mapValue(field, rewrite)) };
  return value;
}
function mapPredicate(value: Predicate, rewrite: PathRewrite): Predicate {
  if (value.type === 'boolean') return { ...value, value: mapValue(value.value, rewrite) };
  if (value.type === 'equal' || value.type === 'not_equal') return { ...value, left: mapValue(value.left, rewrite), right: mapValue(value.right, rewrite) };
  if (value.type === 'not') return { ...value, predicate: mapPredicate(value.predicate, rewrite) };
  return { ...value, predicates: value.predicates.map(field => mapPredicate(field, rewrite)) };
}
function mappingKey(value: Value, path: readonly string[], from: string, to: string): Value {
  if (value.type !== 'object') return value;
  if (!path.length) return Object.hasOwn(value.fields, from) && !Object.hasOwn(value.fields, to)
    ? { ...value, fields: Object.fromEntries(Object.entries(value.fields).map(([key, item]) => [key === from ? to : key, item])) } : value;
  const [head, ...rest] = path;
  return Object.hasOwn(value.fields, head) ? { ...value, fields: { ...value.fields, [head]: mappingKey(value.fields[head], rest, from, to) } } : value;
}

/** Rename only structured selectors with a proven contract owner. Whole-value
 * references and literal/task strings stay intact; native reports any remaining
 * incompatible construction. Arrays have no element selectors in Workflow. */
export function renameSchemaField(document: WorkflowDocument, address: FieldAddress, from: string, to: string): WorkflowDocument {
  const schema = schemaObject(fieldValue(document, address)?.value), properties = schemaObject(schema?.properties);
  if (!schema || !properties || !Object.hasOwn(properties, from)) throw new CommandRejected('missing_field');
  if (!to || new TextEncoder().encode(to).length > 64) throw new CommandRejected('invalid_identifier');
  if (from === to) return document;
  if (Object.hasOwn(properties, to)) throw new CommandRejected('duplicate_identifier');
  const target = schemaTarget(address);
  if (target && document.editor?.orders?.some(order => {
    const candidate = order.target;
    const sameOwner = candidate.kind === target.kind && ('address' in candidate && 'address' in target ? sameDocument(candidate.address, target.address) : 'block' in candidate && 'block' in target && sameDocument(candidate.block, target.block));
    if (!sameOwner || !('path' in candidate) || !sameDocument(candidate.path.slice(0, target.path.length), target.path)) return false;
    const step = candidate.path[target.path.length];
    return candidate.path.length === target.path.length ? order.fields.includes(to) : step.kind === 'property' && step.name === to;
  })) throw new CommandRejected('duplicate_identifier');
  let next = setField(document, address, { ...schema,
    properties: Object.fromEntries(Object.entries(properties).map(([key, value]) => [key === from ? to : key, value])),
    ...(Array.isArray(schema.required) ? { required: schema.required.map(key => key === from ? to : key) } : {}),
  });
  if (!target) return next;
  const oldOrder = orderedKeys(document, target, Object.keys(properties));
  next = setOrder(next, target, oldOrder.map(key => key === from ? to : key));
  // Orders belonging to a renamed descendant follow the schema property too.
  next = { ...next, editor: { ...next.editor, orders: next.editor?.orders?.map(order => {
    const candidate = order.target;
    const sameOwner = candidate.kind === target.kind && ('address' in candidate && 'address' in target ? sameDocument(candidate.address, target.address) : 'block' in candidate && 'block' in target && sameDocument(candidate.block, target.block));
    if (!sameOwner || !('path' in candidate) || candidate.path.length <= target.path.length || !sameDocument(candidate.path.slice(0, target.path.length), target.path)) return order;
    const step = candidate.path[target.path.length];
    return step.kind === 'property' && step.name === from ? { ...order, target: { ...candidate, path: [...target.path, { kind: 'property', name: to } as const, ...candidate.path.slice(target.path.length + 1)] } } : order;
  }) } };
  if (target.path.some(step => step.kind === 'items')) return next;
  const path = target.path.map(step => (step as { name: string }).name);
  const rewrite = (prefix: string[]): PathRewrite => reference => reference.length > prefix.length && prefix.every((part, index) => reference[index] === part) && reference[prefix.length] === from ? [...prefix, to, ...reference.slice(prefix.length + 1)] : undefined;
  const rewriteAt = (block: BlockPath, prefix: string[]) => { next = updateBlock(next, block, value => rewriteBlockReferences(value, rewrite(prefix))); };
  if (target.kind === 'node_output') rewriteAt(address.block, [address.node!, ...path]);
  if (target.kind === 'block_input') {
    rewriteAt(address.block, ['args', ...path]);
    if (address.block.length) {
      const parent = address.block.slice(0, -1), step = address.block.at(-1)!;
      next = updateBlock(next, parent, block => {
        const owner = nodeAt(block, step.node)!;
        if (owner.type === 'loop' && step.branch === undefined) return { ...block, nodes: { ...block.nodes, [step.node]: { ...owner, input: mappingKey(owner.input, path, from, to), carry: mappingKey(owner.carry, path, from, to) } } };
        if (owner.type === 'parallel' && step.branch !== undefined) return { ...block, nodes: { ...block.nodes, [step.node]: { ...owner, branches: { ...owner.branches, [step.branch]: { ...owner.branches[step.branch], input: mappingKey(owner.branches[step.branch].input, path, from, to) } } } } };
        return block;
      });
    }
  }
  if (target.kind === 'block_output') {
    next = updateBlock(next, address.block, block => ({ ...block, nodes: Object.fromEntries(Object.entries(block.nodes).map(([id, node]) => [id, node.type === 'return' ? { ...node, output: mappingKey(node.output, path, from, to) } : node])) }));
    if (address.block.length) {
      const parent = address.block.slice(0, -1), step = address.block.at(-1)!;
      rewriteAt(parent, [step.node, ...(step.branch === undefined ? ['result'] : [step.branch]), ...path]);
      if (step.branch === undefined) next = updateBlock(next, parent, block => {
        const owner = nodeAt(block, step.node)!;
        return owner.type === 'loop' ? { ...block, nodes: { ...block.nodes, [step.node]: { ...owner, until: mapPredicate(owner.until, rewrite(['result', ...path])), carry: mapValue(owner.carry, rewrite(['result', ...path])) } } } : block;
      });
    }
  }
  return next;
}

/** The key address names a property, not the containing schema object. */
export function schemaPropertyAddress(address: FieldAddress, key: string): FieldAddress {
  return { ...address, path: [...address.path, 'properties', { key }] satisfies FieldStep[], role: 'key' };
}

/** Structural usage only, never a type/availability verdict. Keep affected
 * operations visible after a deletion or type change; native decides validity. */
export function schemaConsumers(document: WorkflowDocument, address: FieldAddress): { block: BlockPath; node: string }[] {
  const target = schemaTarget(address);
  if (!target || target.path.some(step => step.kind === 'items')) return [];
  const property = target.path.map(step => (step as { name: string }).name), found = new Map<string, { block: BlockPath; node: string }>();
  const add = (block: BlockPath, node: string) => found.set(JSON.stringify([block, node]), { block, node });
  const matches = (prefix: string[], path: readonly string[]) => path.slice(0, Math.min(prefix.length, path.length)).every((part, index) => part === prefix[index]);
  const scan = (path: BlockPath, prefix: string[]) => {
    const block = blockAt(document, path); if (!block) return;
    for (const [id, node] of Object.entries(block.nodes)) rewriteBlockReferences({ ...block, nodes: { [id]: node } }, reference => { if (matches(prefix, reference)) add(path, id); return undefined; });
  };
  if (target.kind === 'node_output') scan(address.block, [address.node!, ...property]);
  if (target.kind === 'block_input') scan(address.block, ['args', ...property]);
  if (target.kind === 'block_output') {
    const block = blockAt(document, address.block);
    for (const [id, node] of Object.entries(block?.nodes ?? {})) if (node.type === 'return') add(address.block, id);
    const step = address.block.at(-1);
    if (step) {
      const parent = address.block.slice(0, -1);
      scan(parent, [step.node, ...(step.branch === undefined ? ['result'] : [step.branch]), ...property]);
      const node = blockAt(document, parent)?.nodes[step.node];
      if (step.branch === undefined && node?.type === 'loop') {
        const inspect: PathRewrite = path => { if (matches(['result', ...property], path)) add(parent, step.node); return undefined; };
        mapValue(node.carry, inspect); mapPredicate(node.until, inspect);
      }
    }
  }
  return [...found.values()];
}
