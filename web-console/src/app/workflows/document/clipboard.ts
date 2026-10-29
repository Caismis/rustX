import type { WorkflowEditorMetadata } from '../../../../../protocol/app-server/v62';
import { readJson } from '../../../../shared/json-numbers';
import { boundedDocument, CommandRejected, freshIdentifier, validIdentifier } from './commands';
import { blockAt, deepNodeCount, edgesOf, sameBlockPath, sameDocument, updateBlock, type Block, type BlockPath, type NodeDefinition, type WorkflowDocument } from './model';
import type { FieldAddress } from './fields';
import { MAX_COORDINATE, dimensions, freshNote, position, presentationEdit, utf8Length } from './geometry';
import { blockReferences, rewriteBlockReferences } from './references';
import { schemaObject } from './schema';
import { requireVacantNodePresentation } from './presentation';

const MAX_BYTES = 768 * 1024;
export interface Fragment {
  format: 'rustx-workflow-fragment';
  source: string;
  block: BlockPath;
  nodes: Record<string, NodeDefinition>;
  edges: Block['edges'];
  editor: WorkflowEditorMetadata;
}
const within = (path: BlockPath, parent: BlockPath) => path.length >= parent.length && sameBlockPath(path.slice(0, parent.length), parent);

/** Draft ownership includes every private Block of a selected container. */
export function fragmentOwnsField(path: BlockPath, ids: readonly string[], notes: readonly string[], address: FieldAddress): boolean {
  return within(address.block, path) && (address.block.length > path.length ? ids.includes(address.block[path.length].node)
    : address.domain?.kind === 'note' ? notes.includes(address.domain.id) : address.node !== undefined && ids.includes(address.node));
}

/** Export only authored definition and owned presentation, never cached
 * catalogs, field drafts, provider credentials or execution state. */
export function copyFragment(document: WorkflowDocument, source: string, path: BlockPath, ids: readonly string[], noteIds: readonly string[] = []): string {
  const block = blockAt(document, path);
  if (!block) throw new CommandRejected('missing_node');
  if (ids.some(id => !Object.hasOwn(block.nodes, id)) || noteIds.some(id => !document.editor?.notes?.some(note => note.id === id && sameBlockPath(note.block, path)))) throw new CommandRejected('missing_node');
  const selected = new Set(ids), ownsBlock = (at: BlockPath) => within(at, path) && at.length > path.length && selected.has(at[path.length].node);
  const ownsNode = (address: { block: BlockPath; node: string }) => ownsBlock(address.block) || sameBlockPath(address.block, path) && selected.has(address.node);
  const editor = document.editor;
  const fragment: Fragment = { format: 'rustx-workflow-fragment', source, block: path,
    nodes: Object.fromEntries(Object.entries(block.nodes).filter(([id]) => selected.has(id))),
    edges: edgesOf(block).filter(edge => selected.has(edge.from) && selected.has(edge.to)),
    editor: { nodes: editor?.nodes?.filter(entry => ownsNode(entry.address)), entries: editor?.entries?.filter(entry => ownsBlock(entry.block)),
      branches: editor?.branches?.filter(entry => ownsNode(entry.address)), notes: editor?.notes?.filter(note => ownsBlock(note.block) || sameBlockPath(note.block, path) && noteIds.includes(note.id)),
      orders: editor?.orders?.filter(order => 'address' in order.target ? ownsNode(order.target.address) : ownsBlock(order.target.block)) },
  };
  const text = JSON.stringify(fragment);
  if (utf8Length(text) > MAX_BYTES) throw new CommandRejected('limit_exceeded');
  return text;
}

const fail = (): never => { throw new CommandRejected('invalid_fragment'); };
function object(value: unknown): Record<string, unknown> { return schemaObject(value) ?? fail(); }
function keys(value: unknown, allowed: string[]): Record<string, unknown> { const record = object(value); if (Object.keys(record).some(key => !allowed.includes(key))) fail(); return record; }
function text(value: unknown): asserts value is string { if (typeof value !== 'string') fail(); }
function array(value: unknown): unknown[] { return Array.isArray(value) ? value : fail(); }
function point(value: unknown): void { position(keys(value, ['x', 'y']) as unknown as { x: number; y: number }); }
function size(value: unknown): void { dimensions(keys(value, ['width', 'height']) as unknown as { width: number; height: number }); }
function unique(values: unknown[], identity: (value: unknown) => unknown): void {
  const identities = values.map(identity);
  if (identities.some((value, index) => identities.slice(0, index).some(other => sameDocument(value, other)))) fail();
}
function path(value: unknown): void { const steps = array(value); if (steps.length > 8) fail(); for (const step of steps) { const part = keys(step, ['node', 'branch']); text(part.node); if (validIdentifier(part.node)) fail(); if (part.branch !== undefined) { text(part.branch); if (validIdentifier(part.branch)) fail(); } } }
function address(value: unknown): void { const at = keys(value, ['block', 'node']); path(at.block); text(at.node); if (validIdentifier(at.node)) fail(); }
function unsigned(value: unknown): void { if (typeof value !== 'number' || !Number.isSafeInteger(value) || value < 0 || value > 4294967295) fail(); }
function flag(value: unknown): void { if (value !== undefined && typeof value !== 'boolean') fail(); }
function override(value: unknown): void {
  const record = keys(value, ['tools', 'skills', 'plugins']);
  if (record.skills !== undefined) array(record.skills).forEach(text);
  if (record.tools !== undefined) {
    const tools = keys(record.tools, ['builtin', 'sources']);
    if (tools.builtin !== undefined) array(tools.builtin).forEach(text);
    if (tools.sources !== undefined) for (const selected of Object.values(object(tools.sources))) if (selected !== 'all') array(selected).forEach(text);
  }
  if (record.plugins !== undefined) {
    const plugins = keys(record.plugins, ['agentStatus', 'todo', 'goal']);
    for (const name of ['todo', 'goal']) if (plugins[name] !== undefined) flag(keys(plugins[name], ['enabled']).enabled);
    if (plugins.agentStatus !== undefined) {
      const status = keys(plugins.agentStatus, ['enabled', 'time', 'background']); flag(status.enabled);
      if (status.background !== undefined) flag(keys(status.background, ['enabled']).enabled);
      if (status.time !== undefined) { const time = keys(status.time, ['enabled', 'timezone']); flag(time.enabled); if (time.timezone != null) text(time.timezone); }
    }
  }
}
function value(input: unknown): void {
  const record = object(input);
  switch (record.type) {
    case 'literal': keys(record, ['type', 'value']); if (!Object.hasOwn(record, 'value')) fail(); return;
    case 'reference': keys(record, ['type', 'path']); for (const part of array(record.path)) text(part); return;
    case 'object': keys(record, ['type', 'fields']); Object.values(object(record.fields)).forEach(value); return;
    case 'array': keys(record, ['type', 'items']); array(record.items).forEach(value); return;
    default: fail();
  }
}
function predicate(input: unknown): void {
  const record = object(input);
  switch (record.type) {
    case 'boolean': keys(record, ['type', 'value']); value(record.value); return;
    case 'equal': case 'not_equal': keys(record, ['type', 'left', 'right']); value(record.left); value(record.right); return;
    case 'not': keys(record, ['type', 'predicate']); predicate(record.predicate); return;
    case 'and': case 'or': keys(record, ['type', 'predicates']); array(record.predicates).forEach(predicate); return;
    default: fail();
  }
}
function node(input: unknown): void {
  const record = object(input);
  switch (record.type) {
    case 'agent':
      keys(record, ['type', 'profile', 'task', 'input', 'output', 'override']); text(record.profile); text(record.task); object(record.output);
      if (record.override !== undefined) override(record.override);
      if (record.input !== undefined) Object.values(object(record.input)).forEach(value); return;
    case 'model': {
      keys(record, ['type', 'model', 'instruction', 'input', 'output']); text(record.instruction);
      if (record.model !== undefined) {
        const policy = object(record.model);
        if (policy.mode === 'session') keys(policy, ['mode']);
        else if (policy.mode === 'explicit') { keys(policy, ['mode', 'model', 'profile', 'request_params', 'max_output_tokens']); text(policy.model); if (policy.profile != null) text(policy.profile); if (policy.request_params !== undefined) object(policy.request_params); if (policy.max_output_tokens != null) unsigned(policy.max_output_tokens); }
        else fail();
      }
      if (record.input !== undefined) Object.values(object(record.input)).forEach(value);
      const output = object(record.output);
      if (output.type === 'text') keys(output, ['type']); else if (output.type === 'json_schema') { keys(output, ['type', 'schema']); object(output.schema); } else fail(); return;
    }
    case 'tool': {
      keys(record, ['type', 'selector', 'arguments', 'result']); value(record.arguments);
      const selector = keys(record.selector, ['origin', 'name', 'source_id']); text(selector.name);
      if (selector.origin === 'source') text(selector.source_id); else if (selector.origin !== 'builtin' || selector.source_id !== undefined) fail();
      const result = keys(record.result, ['type', 'part', 'schema']);
      unsigned(result.part);
      if (result.type === 'json') object(result.schema); else if (result.type !== 'text' || result.schema !== undefined) fail(); return;
    }
    case 'branch': keys(record, ['type', 'condition']); predicate(record.condition); return;
    case 'return': keys(record, ['type', 'output']); value(record.output); return;
    case 'review': {
      keys(record, ['type', 'subject', 'context']); const subject = keys(record.subject, ['type', 'value']); if (subject.type !== 'plan') fail(); value(subject.value); array(record.context).forEach(value); return;
    }
    case 'loop': keys(record, ['type', 'input', 'body', 'until', 'carry', 'max_iterations']); value(record.input); block(record.body); predicate(record.until); value(record.carry); unsigned(record.max_iterations); return;
    case 'parallel': keys(record, ['type', 'branches']); for (const [id, entry] of Object.entries(object(record.branches))) { if (validIdentifier(id)) fail(); const branch = keys(entry, ['input', 'block']); value(branch.input); block(branch.block); } return;
    default: fail();
  }
}
function edges(input: unknown): void { for (const item of array(input ?? [])) { const edge = keys(item, ['from', 'to', 'port']); text(edge.from); text(edge.to); if (edge.port != null && !['next', 'true', 'false', 'satisfied', 'exhausted', true, false].includes(edge.port as never)) fail(); } }
function nodes(input: unknown): void { for (const [id, item] of Object.entries(object(input))) { if (validIdentifier(id)) fail(); node(item); } }
function block(input: unknown): void { const record = keys(input, ['input', 'output', 'entry', 'nodes', 'edges']); object(record.input); object(record.output); text(record.entry); nodes(record.nodes); edges(record.edges); }

/** This is a bounded transport-shape decoder, not a reachability/type proof.
 * Native authoring remains the only authority for program validity. */
export function parseFragment(source: string): Fragment {
  if (utf8Length(source) > MAX_BYTES) throw new CommandRejected('limit_exceeded');
  let raw: unknown;
  try { raw = JSON.parse(source); } catch { return fail(); }
  const queue = [{ item: raw, depth: 0 }]; let count = 0;
  while (queue.length) {
    const { item, depth } = queue.pop()!;
    if (++count > 65536 || depth > 128) throw new CommandRejected('limit_exceeded');
    if (item !== null && typeof item === 'object') for (const child of Object.values(item)) queue.push({ item: child, depth: depth + 1 });
  }
  const numeric = readJson(source); if (!numeric.ok) fail();
  const record = keys(raw, ['format', 'source', 'block', 'nodes', 'edges', 'editor']);
  if (record.format !== 'rustx-workflow-fragment') fail(); text(record.source); path(record.block); nodes(record.nodes); edges(record.edges);
  const metadata = keys(record.editor, ['nodes', 'entries', 'branches', 'notes', 'orders']);
  const lists = Object.values(metadata).map(array);
  if (lists.reduce((count, list) => count + list.length, 0) > 1024 || array(metadata.notes ?? []).length > 128 || utf8Length(JSON.stringify(metadata)) > 256 * 1024) throw new CommandRejected('limit_exceeded');
  for (const item of array(metadata.nodes ?? [])) {
    const entry = keys(item, ['address', 'title', 'description', 'position', 'dimensions']); address(entry.address); if (entry.title != null) text(entry.title); if (entry.description != null) text(entry.description);
    if (typeof entry.title === 'string' && utf8Length(entry.title) > 256 || typeof entry.description === 'string' && utf8Length(entry.description) > 4096) throw new CommandRejected('limit_exceeded');
    if (entry.position != null) point(entry.position); if (entry.dimensions != null) size(entry.dimensions);
  }
  for (const item of array(metadata.entries ?? [])) { const entry = keys(item, ['block', 'position']); path(entry.block); point(entry.position); }
  for (const item of array(metadata.branches ?? [])) { const entry = keys(item, ['address', 'branch', 'position', 'dimensions']); address(entry.address); text(entry.branch); if (validIdentifier(entry.branch)) fail(); point(entry.position); size(entry.dimensions); }
  for (const item of array(metadata.notes ?? [])) {
    const note = keys(item, ['id', 'block', 'text', 'theme', 'position', 'dimensions']); text(note.id); text(note.text); if (!/^[A-Za-z0-9_-]{1,64}$/.test(note.id) || utf8Length(note.text) > 16384 || !['neutral', 'blue', 'green', 'amber', 'rose'].includes(note.theme as string)) fail();
    path(note.block); point(note.position); size(note.dimensions);
  }
  for (const item of array(metadata.orders ?? [])) {
    const order = keys(item, ['target', 'fields']), target = object(order.target), fields = array(order.fields); fields.forEach(text); unique(fields, value => value);
    if (fields.length > 256 || fields.some(field => !field || utf8Length(field as string) > 64)) fail();
    if (['node_input', 'branches'].includes(target.kind as string)) { keys(target, ['kind', 'address']); address(target.address); }
    else {
      if (target.kind === 'node_output') { keys(target, ['kind', 'address', 'path']); address(target.address); }
      else if (['block_input', 'block_output'].includes(target.kind as string)) { keys(target, ['kind', 'block', 'path']); path(target.block); } else fail();
      const steps = array(target.path); if (steps.length > 32) fail();
      for (const step of steps) { const part = object(step); if (part.kind === 'property') { keys(part, ['kind', 'name']); text(part.name); if (!part.name || utf8Length(part.name) > 64) fail(); } else if (part.kind === 'items') keys(part, ['kind']); else fail(); }
    }
  }
  unique(array(metadata.nodes ?? []), item => object(item).address);
  unique(array(metadata.entries ?? []), item => object(item).block);
  unique(array(metadata.branches ?? []), item => [object(item).address, object(item).branch]);
  unique(array(metadata.notes ?? []), item => object(item).id);
  unique(array(metadata.orders ?? []), item => object(item).target);
  return raw as Fragment;
}

export interface PastedFragment { document: WorkflowDocument; nodes: string[]; notes: string[] }

export function pasteFragment(document: WorkflowDocument, source: string, path: BlockPath, fragment: Fragment): PastedFragment {
  const destination = blockAt(document, path); if (!destination) throw new CommandRejected('missing_node');
  // A selection owns private Block presentation, never the source Block's
  // entry or contract order. Do not let forged fragments adopt the destination.
  for (const at of [...fragment.editor.entries?.map(entry => entry.block) ?? [], ...fragment.editor.orders?.flatMap(order => 'block' in order.target ? [order.target.block] : []) ?? []]) {
    if (!within(at, fragment.block) || at.length <= fragment.block.length) fail();
  }
  const ids = Object.keys(fragment.nodes).sort(), original = new Set(ids);
  const block: Block = { input: {}, output: {}, entry: '', nodes: fragment.nodes, edges: fragment.edges };
  if ((source !== fragment.source || !sameBlockPath(path, fragment.block)) && blockReferences(block).some(reference => !original.has(reference[0]))) throw new CommandRejected('cross_scope');
  const names = new Map<string, string>(); let reserved = destination;
  for (const id of ids) {
    let name = freshIdentifier(reserved, `${id.slice(0, 48)}_copy`);
    for (let suffix = 2; ; suffix++) {
      try { requireVacantNodePresentation(document, path, name); break; }
      catch (error) { if (!(error instanceof CommandRejected) || error.code !== 'duplicate_identifier') throw error; name = freshIdentifier(reserved, `${id.slice(0, 48)}_copy_${suffix}`); }
    }
    names.set(id, name); reserved = { ...reserved, nodes: { ...reserved.nodes, [name]: fragment.nodes[id] } };
  }
  const rewritten = rewriteBlockReferences(block, reference => names.has(reference[0]) ? [names.get(reference[0])!, ...reference.slice(1)] : undefined);
  let next = updateBlock(document, path, current => ({ ...current, nodes: { ...current.nodes, ...Object.fromEntries(ids.map(id => [names.get(id)!, structuredClone(rewritten.nodes[id])])) },
    edges: [...edgesOf(current), ...edgesOf(block).map(edge => ({ ...edge, from: names.get(edge.from) ?? fail(), to: names.get(edge.to) ?? fail() }))] }));
  if (deepNodeCount(next.block) > 256) throw new CommandRejected('limit_exceeded');
  const remap = (at: BlockPath): BlockPath => {
    if (!within(at, fragment.block)) return fail();
    const rest = at.slice(fragment.block.length);
    if (!rest.length) return path;
    if (!names.has(rest[0].node)) return fail();
    return [...path, { ...rest[0], node: names.get(rest[0].node)! }, ...rest.slice(1)];
  };
  const addressAt = (at: { block: BlockPath; node: string }) => {
    const owner = remap(at.block), id = sameBlockPath(at.block, fragment.block) ? names.get(at.node) ?? fail() : at.node;
    return { block: [...owner], node: id };
  };
  const copied = fragment.editor;
  const topPositions = [...copied.nodes?.filter(entry => sameBlockPath(entry.address.block, fragment.block)).flatMap(entry => entry.position ? [entry.position] : []) ?? [],
    ...copied.notes?.filter(note => sameBlockPath(note.block, fragment.block)).map(note => note.position) ?? []];
  const occupied = [...document.editor?.nodes?.filter(entry => sameBlockPath(entry.address.block, path)).flatMap(entry => entry.position ? [entry.position] : []) ?? [],
    ...document.editor?.notes?.filter(note => sameBlockPath(note.block, path)).map(note => note.position) ?? []];
  // Each pair forbids at most one diagonal offset. Enumerate once, rather
  // than rescanning every pair for every candidate in an untrusted fragment.
  const forbidden = new Set<number>();
  let maximumStep = Infinity;
  for (const point of topPositions) {
    maximumStep = Math.min(maximumStep, Math.floor((MAX_COORDINATE - Math.max(point.x, point.y)) / 32));
    for (const other of occupied) {
      const candidate = Math.round((other.x - point.x) / 32);
      // Match the actual placement addition: subtraction/modulo loses exact
      // multiples at ordinary fractional drag/zoom coordinates.
      if (candidate > 0 && point.x + candidate * 32 === other.x && point.y + candidate * 32 === other.y) forbidden.add(candidate);
    }
  }
  let step = 1;
  while (step <= maximumStep && forbidden.has(step)) step++;
  if (step > maximumStep) throw new CommandRejected('invalid_geometry');
  const shift = step * 32;
  const offset = (point: { x: number; y: number }) => { const next = { x: point.x + shift, y: point.y + shift }; position(next); return next; };
  const notes: string[] = [];
  for (const note of copied.notes ?? []) {
    const id = freshNote(next, note.id); if (sameBlockPath(note.block, fragment.block)) notes.push(id);
    next = presentationEdit(next, editor => ({ ...editor, notes: [...editor.notes ?? [], { ...note, id, block: [...remap(note.block)], position: sameBlockPath(note.block, fragment.block) ? offset(note.position) : note.position }] }));
  }
  next = presentationEdit(next, editor => ({ ...editor,
    nodes: [...editor.nodes ?? [], ...(copied.nodes ?? []).map(entry => ({ ...entry, address: addressAt(entry.address), ...(entry.position && sameBlockPath(entry.address.block, fragment.block) ? { position: offset(entry.position) } : {}) }))],
    entries: [...editor.entries ?? [], ...(copied.entries ?? []).map(entry => ({ ...entry, block: [...remap(entry.block)] }))],
    branches: [...editor.branches ?? [], ...(copied.branches ?? []).map(entry => ({ ...entry, address: addressAt(entry.address) }))],
    orders: [...editor.orders ?? [], ...(copied.orders ?? []).map(order => ({ ...order, target: 'address' in order.target ? { ...order.target, address: addressAt(order.target.address) } : { ...order.target, block: [...remap(order.target.block)] } }))],
  }));
  return { document: boundedDocument(next), nodes: [...names.values()], notes };
}

/** Inspector, canvas and keyboard share one duplicate transaction. The scene
 * supplies current top-level positions for nodes without authored geometry;
 * private coordinates remain relative to their newly owned containers. */
export function duplicateFragment(document: WorkflowDocument, path: BlockPath, ids: readonly string[], notes: readonly string[], positions: ReadonlyMap<string, { x: number; y: number }>): PastedFragment {
  const source = 'duplicate';
  const fragment = parseFragment(copyFragment(document, source, path, ids, notes));
  for (const id of ids) {
    const point = positions.get(id);
    if (!point) throw new CommandRejected('missing_node');
    const entry = fragment.editor.nodes?.find(entry => sameBlockPath(entry.address.block, path) && entry.address.node === id);
    if (entry) entry.position = point;
    else (fragment.editor.nodes ??= []).push({ address: { block: [...path], node: id }, position: point });
  }
  return pasteFragment(document, source, path, fragment);
}
