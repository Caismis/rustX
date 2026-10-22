import { CommandRejected, replaceNode } from './commands';
import { blockAt, nodeAt, sameBlockPath, updateBlock, type BlockPath, type WorkflowDocument } from './model';

/** One component of a field path inside a node or block. A string is a
 * structural property of the native authoring type, `{ key }` an authored
 * object key (a value field, an Agent input, a Parallel branch) and a
 * number an array index. Authored keys may contain any text, so a path is
 * kept as components and never joined into one string. */
export type FieldStep = string | number | { readonly key: string };

/** What a field's text edits at its location: the JSON `value` there, the
 * authored `key` the path ends in (the node id itself when the path is
 * empty), or one component to `append` to the path array there. */
export type FieldRole = 'value' | 'key' | 'append';

/** The editor-internal identity of one editable field: its lexical block,
 * its owning node (none for a block's own contract) and its path. It owns
 * a field's unapplied text, its error, its commit registration and its
 * coalesced edit session. It is not a native diagnostic path. */
export interface FieldAddress {
  readonly block: BlockPath;
  readonly node?: string;
  readonly path: readonly FieldStep[];
  readonly role: FieldRole;
}

export function nodeField(block: BlockPath, node: string, path: readonly FieldStep[] = [], role: FieldRole = 'value'): FieldAddress {
  return { block, node, path, role };
}

export function blockField(block: BlockPath, path: readonly FieldStep[]): FieldAddress {
  return { block, path, role: 'value' };
}

/** The address of a location inside `address`'s value. */
export function within(address: FieldAddress, steps: readonly FieldStep[], role: FieldRole = 'value'): FieldAddress {
  return { ...address, path: [...address.path, ...steps], role };
}

/** The canonical key of an address: a JSON tuple of its components, so
 * `fields → "a.fields.b"` and `fields → a → fields → b` never collide. */
export function encodeField(address: FieldAddress): string {
  return JSON.stringify([address.block.map(step => step.branch === undefined ? [step.node] : [step.node, step.branch]), address.node ?? null, address.path, address.role]);
}

/** The identity of one of the Workflow's own fields (description,
 * timeout), which belong to no block. */
export function workflowField(name: 'description' | 'timeout_ms'): string {
  return JSON.stringify(['workflow', name]);
}

const MISSING = Symbol('missing');

function step(at: unknown, component: FieldStep): unknown {
  if (typeof component === 'number') return Array.isArray(at) && Number.isInteger(component) && component >= 0 && component < at.length ? at[component] : MISSING;
  const name = typeof component === 'string' ? component : component.key;
  return at !== null && typeof at === 'object' && !Array.isArray(at) && Object.hasOwn(at, name) ? (at as Record<string, unknown>)[name] : MISSING;
}

/** The node or block an address is rooted in, or `MISSING`. */
function root(document: WorkflowDocument, address: FieldAddress): unknown {
  const block = blockAt(document, address.block);
  if (!block) return MISSING;
  if (address.node === undefined) return block;
  return nodeAt(block, address.node) ?? MISSING;
}

/** The array lengths along an address, so an item that moved into an index
 * is never taken for the item that held it; `undefined` when missing. */
function shape(document: WorkflowDocument, address: FieldAddress): number[] | undefined {
  let at = root(document, address);
  const lengths: number[] = [];
  for (const component of address.path) {
    if (at === MISSING) return undefined;
    if (typeof component === 'number' && Array.isArray(at)) lengths.push(at.length);
    at = step(at, component);
  }
  return at === MISSING ? undefined : lengths;
}

/** The value at an address; `undefined` when the field no longer exists. */
export function fieldValue(document: WorkflowDocument, address: FieldAddress): { value: unknown } | undefined {
  let at = root(document, address);
  for (const component of address.path) at = at === MISSING ? MISSING : step(at, component);
  return at === MISSING ? undefined : { value: at };
}

/** The text a field shows when it holds no unapplied text; `undefined`
 * when the addressed field no longer exists. */
export function fieldText(document: WorkflowDocument, address: FieldAddress): string | undefined {
  const found = fieldValue(document, address);
  if (!found) return undefined;
  switch (address.role) {
    case 'value': return JSON.stringify(found.value, null, 2) ?? 'null';
    case 'key': {
      const last = address.path.at(-1);
      if (last === undefined) return address.node;
      return typeof last === 'object' ? last.key : undefined;
    }
    case 'append': return Array.isArray(found.value) ? '' : undefined;
  }
}

/** Whether `after` still holds the same field as `before`: it exists and
 * no array along its path changed length (a shifted index is another item). */
export function sameField(before: WorkflowDocument, after: WorkflowDocument, address: FieldAddress): boolean {
  const old = shape(before, address), current = shape(after, address);
  return !!old && !!current && old.length === current.length && old.every((length, index) => length === current[index]);
}

function setIn(at: unknown, path: readonly FieldStep[], value: unknown): unknown {
  if (!path.length) return value;
  const [head, ...rest] = path;
  const next = step(at, head);
  if (next === MISSING) throw new CommandRejected('missing_field');
  if (typeof head === 'number') return (at as unknown[]).map((item, index) => index === head ? setIn(item, rest, value) : item);
  const name = typeof head === 'string' ? head : head.key;
  return { ...(at as Record<string, unknown>), [name]: setIn(next, rest, value) };
}

/** Write `value` at a `value` address of the current document, through
 * the owning node's or block's own replacement. */
export function setField(document: WorkflowDocument, address: FieldAddress, value: unknown): WorkflowDocument {
  const block = blockAt(document, address.block);
  if (!block) throw new CommandRejected('missing_node');
  if (address.node === undefined) return updateBlock(document, address.block, current => setIn(current, address.path, value) as typeof current);
  const node = nodeAt(block, address.node);
  if (!node) throw new CommandRejected('missing_node');
  return replaceNode(document, address.block, address.node, setIn(node, address.path, value) as typeof node);
}

/** Replace the value at an address from its current value. */
export function updateField<T>(document: WorkflowDocument, address: FieldAddress, update: (current: T) => T): WorkflowDocument {
  const found = fieldValue(document, address);
  if (!found) throw new CommandRejected('missing_field');
  return setField(document, address, update(found.value as T));
}

/** `address` after the node or Parallel branch `renamed` (a `key` address)
 * was renamed to `to`: every field of that node or branch, and of the
 * blocks it owns, follows it. Other addresses are unchanged. */
export function renameAddress(address: FieldAddress, renamed: FieldAddress, to: string): FieldAddress {
  const depth = renamed.block.length;
  const inside = address.block.length >= depth && sameBlockPath(address.block.slice(0, depth), renamed.block);
  if (!inside || renamed.node === undefined) return address;
  if (renamed.path.length === 0) {
    if (address.block.length === depth) return address.node === renamed.node ? { ...address, node: to } : address;
    const owner = address.block[depth];
    if (owner.node !== renamed.node) return address;
    return { ...address, block: [...renamed.block, { ...owner, node: to }, ...address.block.slice(depth + 1)] };
  }
  const branch = renamed.path[1];
  if (renamed.path.length !== 2 || renamed.path[0] !== 'branches' || typeof branch !== 'object') return address;
  if (address.block.length === depth) {
    const [first, second, ...rest] = address.path;
    return address.node === renamed.node && first === 'branches' && typeof second === 'object' && second.key === branch.key
      ? { ...address, path: [first, { key: to }, ...rest] } : address;
  }
  const owner = address.block[depth];
  if (owner.node !== renamed.node || owner.branch !== branch.key) return address;
  return { ...address, block: [...renamed.block, { node: owner.node, branch: to }, ...address.block.slice(depth + 1)] };
}

/** A readable location of a field for the author, e.g.
 * `reviews / quality › done · output.fields["a.fields.b"].value`. */
export function describeField(address: FieldAddress): string {
  const place = [...address.block.map(step => `${step.node} / ${step.branch ?? 'body'}`), ...(address.node === undefined ? [] : [address.node])].join(' › ');
  const path = address.path.map((component, index) => typeof component === 'number' ? `[${component}]`
    : typeof component === 'object' ? `[${JSON.stringify(component.key)}]` : `${index ? '.' : ''}${component}`).join('');
  return [place, path].filter(Boolean).join(' · ');
}

/** One array edit of the working document: an item removed at `removed`,
 * or inserted at `inserted` (an append inserts at the old length). It is
 * editor-local metadata that lets the drafts of other items keep their
 * identity; nothing of it reaches the native file. */
export interface ArrayEdit {
  readonly array: FieldAddress;
  readonly removed?: number;
  readonly inserted?: number;
}

/** The edit that undoes `edit`. */
export function invertArrayEdit(edit: ArrayEdit): ArrayEdit {
  return { array: edit.array, ...(edit.removed !== undefined ? { inserted: edit.removed } : {}), ...(edit.inserted !== undefined ? { removed: edit.inserted } : {}) };
}

function sameStep(left: FieldStep, right: FieldStep): boolean {
  return typeof left === 'object' && typeof right === 'object' ? left.key === right.key : left === right;
}

/** Where a field inside the edited array is after `edit`: the same item at
 * its new index, `undefined` when its own item was removed, or `address`
 * itself for a field outside that array. `covered` says the edit accounts
 * for the array's length change on this field's path. */
export function remapAddress(address: FieldAddress, edit: ArrayEdit): { address: FieldAddress; covered: boolean } | undefined {
  const { array } = edit, depth = array.path.length;
  const inside = address.node === array.node && sameBlockPath(address.block, array.block)
    && address.path.length > depth && array.path.every((component, index) => sameStep(component, address.path[index]));
  const index = inside ? address.path[depth] : undefined;
  if (typeof index !== 'number') return { address, covered: false };
  let moved = index;
  if (edit.removed !== undefined) {
    if (index === edit.removed) return undefined;
    if (index > edit.removed) moved--;
  }
  if (edit.inserted !== undefined && moved >= edit.inserted) moved++;
  if (moved === index) return { address, covered: true };
  return { address: { ...address, path: [...address.path.slice(0, depth), moved, ...address.path.slice(depth + 1)] }, covered: true };
}
