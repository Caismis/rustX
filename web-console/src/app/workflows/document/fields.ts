import { CommandRejected, replaceNode } from './commands';
import { blockAt, nodeAt, sameBlockPath, updateBlock, type BlockPath, type WorkflowDocument } from './model';
import { nodePresentation, putNote, setNodePresentation } from './geometry';
import type { WorkflowNodePresentation, WorkflowNote } from '../../../../../protocol/app-server/v62';

/** One component of a field path inside a node or block. A string is a
 * structural property of the native authoring type, `{ key }` an authored
 * object key (a value field or an Agent input) and a
 * number an array index. Authored keys may contain any text, so a path is
 * kept as components and never joined into one string. */
export type FieldStep = string | number | { readonly key: string };

/** What a field's text edits at its location: the JSON `value` there, the
 * authored `key` the path ends in (the node id itself when the path is
 * empty), one component to `append` to the collection there, or an explicitly
 * `optional` field that may be absent. */
export type FieldRole = 'value' | 'key' | 'append' | 'optional';

/** The editor-internal identity of one editable field: its lexical block,
 * its owning node (none for a block's own contract) and its path. It owns
 * a field's unapplied text, its error, its commit registration and its
 * coalesced edit session. It is not a native diagnostic path. */
export interface FieldAddress {
  readonly block: BlockPath;
  readonly node?: string;
  readonly path: readonly FieldStep[];
  readonly role: FieldRole;
  readonly domain?: { readonly kind: 'node_presentation' } | { readonly kind: 'note'; readonly id: string };
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
  const parts: unknown[] = [address.block.map(step => [step.node]), address.node ?? null, address.path, address.role];
  if (address.domain) parts.push(address.domain);
  return JSON.stringify(parts);
}

/** The identity of one of the Workflow's own fields (description,
 * timeout, display), which belong to no block. A display field is named by
 * its native diagnostic path. */
export function workflowField(name: 'description' | 'timeout_ms' | 'editor.display' | 'editor.display.description'): string {
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
  if (address.domain?.kind === 'note') { const id = address.domain.id; return document.editor?.notes?.find(note => note.id === id && sameBlockPath(note.block, address.block)) ?? MISSING; }
  if (address.domain?.kind === 'node_presentation') {
    if (!address.node || !nodeAt(block, address.node)) return MISSING;
    return { ...nodePresentation(document, address.block, address.node), title: nodePresentation(document, address.block, address.node)?.title ?? address.node, description: nodePresentation(document, address.block, address.node)?.description ?? '' };
  }
  if (address.node === undefined) return block;
  return nodeAt(block, address.node) ?? MISSING;
}

/** The array lengths along an address, so an item that moved into an index
 * is never taken for the item that held it; `undefined` when missing. */
function shape(document: WorkflowDocument, address: FieldAddress): number[] | undefined {
  let at = root(document, address);
  const lengths: number[] = [];
  for (const [index, component] of address.path.entries()) {
    if (at === MISSING) return undefined;
    if (typeof component === 'number' && Array.isArray(at)) lengths.push(at.length);
    const next = step(at, component);
    if (next === MISSING && index === address.path.length - 1 && address.role === 'optional') return lengths;
    at = next;
  }
  return at === MISSING ? undefined : lengths;
}

/** The value at an address; `undefined` when the field no longer exists. */
export function fieldValue(document: WorkflowDocument, address: FieldAddress): { value: unknown } | undefined {
  let at = root(document, address);
  for (const [index, component] of address.path.entries()) {
    if (at === MISSING) return undefined;
    at = step(at, component);
    if (at === MISSING && index === address.path.length - 1 && address.role === 'optional') return { value: undefined };
  }
  return at === MISSING ? undefined : { value: at };
}

/** The text a field shows when it holds no unapplied text; `undefined`
 * when the addressed field no longer exists. */
export function fieldText(document: WorkflowDocument, address: FieldAddress): string | undefined {
  const found = fieldValue(document, address);
  if (!found) return undefined;
  switch (address.role) {
    case 'optional': return found.value === undefined ? '' : JSON.stringify(found.value, null, 2) ?? 'null';
    case 'value': return JSON.stringify(found.value, null, 2) ?? 'null';
    case 'key': {
      const last = address.path.at(-1);
      if (last === undefined) return address.node;
      return typeof last === 'object' ? last.key : undefined;
    }
    case 'append': return found.value !== null && typeof found.value === 'object' ? '' : undefined;
  }
}

/** Whether `after` still holds the same field as `before`: it exists and
 * no array along its path changed length (a shifted index is another item). */
export function sameField(before: WorkflowDocument, after: WorkflowDocument, address: FieldAddress): boolean {
  const old = shape(before, address), current = shape(after, address);
  return !!old && !!current && old.length === current.length && old.every((length, index) => length === current[index]);
}

function setIn(at: unknown, path: readonly FieldStep[], value: unknown, optional = false): unknown {
  if (!path.length) return value;
  const [head, ...rest] = path;
  const next = step(at, head);
  if (next === MISSING && !(optional && !rest.length && typeof head !== 'number' && at !== null && typeof at === 'object')) throw new CommandRejected('missing_field');
  if (typeof head === 'number') return (at as unknown[]).map((item, index) => index === head ? setIn(item, rest, value, optional) : item);
  const name = typeof head === 'string' ? head : head.key;
  return { ...(at as Record<string, unknown>), [name]: setIn(next, rest, value, optional) };
}

/** Write `value` at a `value` address of the current document, through
 * the owning node's or block's own replacement. */
export function setField(document: WorkflowDocument, address: FieldAddress, value: unknown): WorkflowDocument {
  const block = blockAt(document, address.block);
  if (!block) throw new CommandRejected('missing_node');
  if (address.domain?.kind === 'node_presentation' && address.node) return setNodePresentation(document, address.block, address.node, setIn(root(document, address), address.path, value) as WorkflowNodePresentation);
  if (address.domain?.kind === 'note') return putNote(document, setIn(root(document, address), address.path, value) as WorkflowNote);
  if (address.node === undefined) return updateBlock(document, address.block, current => setIn(current, address.path, value, address.role === 'optional') as typeof current);
  const node = nodeAt(block, address.node);
  if (!node) throw new CommandRejected('missing_node');
  return replaceNode(document, address.block, address.node, setIn(node, address.path, value, address.role === 'optional') as typeof node);
}

/** Replace the value at an address from its current value. */
export function updateField<T>(document: WorkflowDocument, address: FieldAddress, update: (current: T) => T): WorkflowDocument {
  const found = fieldValue(document, address);
  if (!found) throw new CommandRejected('missing_field');
  return setField(document, address, update(found.value as T));
}

/** `address` after the node `renamed` (a `key` address)
 * was renamed to `to`: every field of that node or branch, and of the
 * blocks it owns, follows it. Other addresses are unchanged. */
export function renameAddress(address: FieldAddress, renamed: FieldAddress, to: string): FieldAddress {
  const depth = renamed.block.length;
  const inside = address.block.length >= depth && sameBlockPath(address.block.slice(0, depth), renamed.block);
  if (!inside) return address;
  if (renamed.path.length === 0) {
    if (renamed.node === undefined) return address;
    if (address.block.length === depth) return address.node === renamed.node ? { ...address, node: to } : address;
    const owner = address.block[depth];
    if (owner.node !== renamed.node) return address;
    return { ...address, block: [...renamed.block, { ...owner, node: to }, ...address.block.slice(depth + 1)] };
  }
  // Schema-property drafts follow their contract's explicit key rename.
  const property = renamed.path.at(-1);
  if (renamed.path.at(-2) === 'properties' && typeof property === 'object'
    && address.block.length === depth && address.node === renamed.node
    && address.path.length >= renamed.path.length && renamed.path.every((part, index) => sameStep(part, address.path[index]))) {
    return { ...address, path: [...renamed.path.slice(0, -1), { key: to }, ...address.path.slice(renamed.path.length)] };
  }
  return address;
}

/** A readable location of a field for the author, e.g.
 * `reviews / quality › done · output.fields["a.fields.b"].value`. */
export function describeField(address: FieldAddress): string {
  const place = [...address.block.map(step => `${step.node} / body`), ...(address.domain?.kind === 'note' ? [address.domain.id] : address.node === undefined ? [] : [address.node])].join(' › ');
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
