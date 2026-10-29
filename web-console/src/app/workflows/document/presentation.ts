import type { WorkflowDisplay, WorkflowEditorMetadata, WorkflowFieldOrder, WorkflowNodeAddress, WorkflowOrderTarget, WorkflowPosition } from '../../../../../protocol/app-server/v62';
import { sameBlockPath, sameDocument, type BlockPath, type WorkflowDocument } from './model';
import { CommandRejected } from './commands';

export const emptyPresentation = (): WorkflowEditorMetadata => ({ nodes: [], notes: [], orders: [] });

/** Set how people see the Workflow. A display with no name, description or
 * icon is removed; one with only a missing name is kept for native
 * validation to name. */
export function setDisplay(document: WorkflowDocument, display: WorkflowDisplay): WorkflowDocument {
  const editor = document.editor ?? emptyPresentation();
  const empty = !display.title.trim() && !display.description && !display.icon;
  const { display: _previous, ...rest } = editor;
  return { ...document, editor: empty ? rest : { ...rest, display } };
}

/** Persist only authored positions. Renderer dimensions and viewport never enter
 * this transaction. Existing titles, notes, geometry and orders are retained. */
export function setPositions(document: WorkflowDocument, block: BlockPath, positions: Readonly<Record<string, WorkflowPosition>>): WorkflowDocument {
  const editor = document.editor ?? emptyPresentation();
  const nodes = [...editor.nodes ?? []];
  for (const [node, position] of Object.entries(positions)) {
    if (!Number.isFinite(position.x) || !Number.isFinite(position.y) || Math.abs(position.x) > 1_000_000 || Math.abs(position.y) > 1_000_000) continue;
    const index = nodes.findIndex(entry => sameBlockPath(entry.address.block, block) && entry.address.node === node);
    const previous = index < 0 ? { address: { block: [...block], node } } : nodes[index];
    const next = { ...previous, position };
    if (index < 0) nodes.push(next); else nodes[index] = next;
  }
  return { ...document, editor: { ...editor, nodes } };
}

export function positionsIn(document: WorkflowDocument, block: BlockPath): Record<string, WorkflowPosition> {
  return Object.fromEntries((document.editor?.nodes ?? []).filter(node => sameBlockPath(node.address.block, block) && node.position).map(node => [node.address.node, node.position!]));
}

/** Authored display order has no execution meaning. Newly declared fields use
 * deterministic key order after explicitly ordered fields. */
export function orderedKeys(document: WorkflowDocument, target: WorkflowOrderTarget, keys: readonly string[]): string[] {
  const order = document.editor?.orders?.find(order => sameDocument(order.target, target))?.fields ?? [];
  const present = new Set(keys), chosen = order.filter(key => present.has(key));
  return [...chosen, ...keys.filter(key => !chosen.includes(key)).sort()];
}

type RewriteBlock = (block: BlockPath) => BlockPath | undefined;
type RewriteNode = (node: WorkflowNodeAddress) => WorkflowNodeAddress | undefined;

/** Metadata and executable structural edits are one document transaction.
 * A note is removed only when its owning Block is removed. */
function rewrite(editor: WorkflowEditorMetadata | null | undefined, block: RewriteBlock, node: RewriteNode,
  fields: (target: WorkflowOrderTarget, fields: string[]) => string[] = (_, fields) => fields,
  orderTarget: (target: WorkflowOrderTarget) => WorkflowOrderTarget | undefined = target => target): WorkflowEditorMetadata | null | undefined {
  if (!editor) return editor;
  const nodes = (editor.nodes ?? []).flatMap(entry => { const address = node(entry.address); return address ? [{ ...entry, address }] : []; });
  const notes = (editor.notes ?? []).flatMap(entry => { const path = block(entry.block); return path ? [{ ...entry, block: [...path] }] : []; });
  const orders = (editor.orders ?? []).flatMap<WorkflowFieldOrder>(entry => {
    const target = orderTarget(entry.target);
    if (!target) return [];
    if ('address' in target) { const address = node(target.address); return address ? [{ ...entry, fields: fields(entry.target, entry.fields), target: { ...target, address } }] : []; }
    const path = block(target.block); return path ? [{ ...entry, target: { ...target, block: [...path] } }] : [];
  });
  // A stale destination entry is authored content too. Refuse a collision,
  // rather than erase it or let a rename silently replace it.
  for (const identities of [nodes.map(n => n.address), orders.map(o => o.target)]) {
    const keys = identities.map(identity => JSON.stringify(identity));
    if (new Set(keys).size !== keys.length) throw new CommandRejected('duplicate_identifier');
  }
  if (orders.some(order => new Set(order.fields).size !== order.fields.length)) throw new CommandRejected('duplicate_identifier');
  return { ...editor, nodes, notes, orders };
}

function within(block: BlockPath, parent: BlockPath): boolean { return block.length >= parent.length && sameBlockPath(block.slice(0, parent.length), parent); }

/** Test the original destination, including metadata with no source counterpart. */
function refuseOccupiedPresentation(document: WorkflowDocument, block: (path: BlockPath) => boolean, node: (address: WorkflowNodeAddress) => boolean,
  order: (order: WorkflowFieldOrder) => boolean = () => false): void {
  const editor = document.editor;
  if (editor && ((editor.nodes ?? []).some(entry => node(entry.address))
    || (editor.notes ?? []).some(entry => block(entry.block))
    || (editor.orders ?? []).some(entry => order(entry) || ('address' in entry.target ? node(entry.target.address) : block(entry.target.block))))) {
    throw new CommandRejected('duplicate_identifier');
  }
}

export function requireVacantNodePresentation(document: WorkflowDocument, parent: BlockPath, to: string): void {
  const block = (path: BlockPath) => within(path, parent) && path.length > parent.length && path[parent.length].node === to;
  refuseOccupiedPresentation(document, block, address => block(address.block) || sameBlockPath(address.block, parent) && address.node === to);
}

export function renameNodePresentation(document: WorkflowDocument, parent: BlockPath, from: string, to: string): WorkflowDocument {
  if (from === to) return document;
  requireVacantNodePresentation(document, parent, to);
  const block: RewriteBlock = path => within(path, parent) && path.length > parent.length && path[parent.length].node === from
    ? [...parent, { ...path[parent.length], node: to }, ...path.slice(parent.length + 1)] : path;
  const node: RewriteNode = address => ({ block: [...block(address.block)!], node: sameBlockPath(address.block, parent) && address.node === from ? to : address.node });
  return { ...document, editor: rewrite(document.editor, block, node) };
}

export function deleteNodePresentation(document: WorkflowDocument, parent: BlockPath, ids: ReadonlySet<string>): WorkflowDocument {
  const block: RewriteBlock = path => within(path, parent) && path.length > parent.length && ids.has(path[parent.length].node) ? undefined : path;
  const node: RewriteNode = address => {
    const path = block(address.block);
    return !path || (sameBlockPath(address.block, parent) && ids.has(address.node)) ? undefined : { ...address, block: [...path] };
  };
  return { ...document, editor: rewrite(document.editor, block, node) };
}
