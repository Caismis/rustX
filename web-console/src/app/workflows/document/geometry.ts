import type { WorkflowBranchPresentation, WorkflowDimensions, WorkflowEntryPresentation, WorkflowNodePresentation, WorkflowNote, WorkflowOrderTarget, WorkflowPosition } from '../../../../../protocol/app-server/v62';
import { CommandRejected, validLocalKey } from './commands';
import { blockAt, nodeAt, sameBlockPath, sameDocument, type BlockPath, type WorkflowDocument } from './model';
import { emptyPresentation } from './presentation';

export const MAX_COORDINATE = 1_000_000;
export const utf8Length = (text: string) => new TextEncoder().encode(text).length;
export function position(point: WorkflowPosition): void {
  if (![point.x, point.y].every(value => Number.isFinite(value) && Math.abs(value) <= MAX_COORDINATE)) throw new CommandRejected('invalid_geometry');
}
export function dimensions(size: WorkflowDimensions): void {
  if (![size.width, size.height].every(value => Number.isFinite(value) && value >= 16 && value <= 8192)) throw new CommandRejected('invalid_geometry');
}

/** Bound the same metadata domain as native without deciding program validity. */
export function presentationEdit(document: WorkflowDocument, edit: (editor: NonNullable<WorkflowDocument['editor']>) => NonNullable<WorkflowDocument['editor']>): WorkflowDocument {
  const editor = edit(document.editor ?? emptyPresentation());
  if ((editor.nodes?.length ?? 0) + (editor.entries?.length ?? 0) + (editor.branches?.length ?? 0) + (editor.orders?.length ?? 0) + (editor.notes?.length ?? 0) > 1024
    || (editor.notes?.length ?? 0) > 128 || utf8Length(JSON.stringify(editor)) > 256 * 1024) throw new CommandRejected('limit_exceeded');
  return sameDocument(editor, document.editor) ? document : { ...document, editor };
}

export function nodePresentation(document: WorkflowDocument, block: BlockPath, id: string): WorkflowNodePresentation | undefined {
  return document.editor?.nodes?.find(entry => entry.address.node === id && sameBlockPath(entry.address.block, block));
}
export function setNodePresentation(document: WorkflowDocument, block: BlockPath, id: string, changes: Omit<Partial<WorkflowNodePresentation>, 'address'>): WorkflowDocument {
  const owner = blockAt(document, block);
  if (!owner || !nodeAt(owner, id)) throw new CommandRejected('missing_node');
  if (changes.title != null && utf8Length(changes.title) > 256 || changes.description != null && utf8Length(changes.description) > 4096) throw new CommandRejected('limit_exceeded');
  if (changes.position) position(changes.position);
  if (changes.dimensions) dimensions(changes.dimensions);
  return presentationEdit(document, editor => {
    const nodes = [...editor.nodes ?? []], index = nodes.findIndex(entry => entry.address.node === id && sameBlockPath(entry.address.block, block));
    const value = { ...(index < 0 ? { address: { block: [...block], node: id } } : nodes[index]), ...changes };
    if (index < 0) nodes.push(value); else nodes[index] = value;
    return { ...editor, nodes };
  });
}
export function entryPresentation(document: WorkflowDocument, block: BlockPath): WorkflowEntryPresentation | undefined {
  return document.editor?.entries?.find(entry => sameBlockPath(entry.block, block));
}
/** A block's Start node: its position, title and description. Its default
 * position is the scene's until one is authored. */
export function setEntryPresentation(document: WorkflowDocument, block: BlockPath, changes: Partial<Omit<WorkflowEntryPresentation, 'block'>>): WorkflowDocument {
  if (!blockAt(document, block)) throw new CommandRejected('missing_node');
  if (changes.title != null && utf8Length(changes.title) > 256 || changes.description != null && utf8Length(changes.description) > 4096) throw new CommandRejected('limit_exceeded');
  if (changes.position) position(changes.position);
  const current = entryPresentation(document, block);
  const value: WorkflowEntryPresentation = { ...current, ...changes, block: [...block], position: changes.position ?? current?.position ?? START_POSITION };
  // An empty title or description is the default, not authored text.
  if (!value.title) delete value.title;
  if (!value.description) delete value.description;
  return presentationEdit(document, editor => ({ ...editor, entries: [...(editor.entries ?? []).filter(entry => !sameBlockPath(entry.block, block)), value] }));
}
export function setEntryPosition(document: WorkflowDocument, block: BlockPath, point: WorkflowPosition): WorkflowDocument {
  return setEntryPresentation(document, block, { position: point });
}
/** Where the scene draws a Start node without an authored position. */
export const START_POSITION: WorkflowPosition = { x: 32, y: 80 };
export function setLane(document: WorkflowDocument, value: WorkflowBranchPresentation): WorkflowDocument {
  const owner = blockAt(document, value.address.block), node = owner && nodeAt(owner, value.address.node);
  if (node?.type !== 'parallel' || !Object.hasOwn(node.branches, value.branch)) throw new CommandRejected('missing_branch');
  position(value.position); dimensions(value.dimensions);
  return presentationEdit(document, editor => ({ ...editor, branches: [...(editor.branches ?? []).filter(entry => !(sameDocument(entry.address, value.address) && entry.branch === value.branch)), value] }));
}
export function setOrder(document: WorkflowDocument, target: WorkflowOrderTarget, fields: string[]): WorkflowDocument {
  return presentationEdit(document, editor => ({ ...editor, orders: [...(editor.orders ?? []).filter(order => !sameDocument(order.target, target)), { target, fields }] }));
}
export function freshNote(document: WorkflowDocument, base = 'note'): string {
  const ids = new Set(document.editor?.notes?.map(note => note.id));
  for (let index = 1; ; index++) { const id = index === 1 ? base : `${base.slice(0, 54)}_${index}`; if (!ids.has(id)) return id; }
}
export function putNote(document: WorkflowDocument, note: WorkflowNote): WorkflowDocument {
  if (!blockAt(document, note.block)) throw new CommandRejected('missing_node');
  if (validLocalKey(note.id)) throw new CommandRejected('invalid_identifier');
  if (utf8Length(note.text) > 16 * 1024) throw new CommandRejected('limit_exceeded');
  position(note.position); dimensions(note.dimensions);
  return presentationEdit(document, editor => ({ ...editor, notes: [...(editor.notes ?? []).filter(entry => entry.id !== note.id), note] }));
}
export function removeNotes(document: WorkflowDocument, ids: readonly string[]): WorkflowDocument {
  return presentationEdit(document, editor => ({ ...editor, notes: editor.notes?.filter(note => !ids.includes(note.id)) }));
}
