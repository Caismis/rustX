import type { WorkflowEditorMetadata } from '../../../../../protocol/app-server/v62';
import { readJson } from '../../../../shared/json-numbers';
import { boundedDocument, CommandRejected, freshIdentifier, validLocalKey, validNodeId } from './commands';
import { blockAt, deepNodeCount, edgesOf, sameBlockPath, sameDocument, updateBlock, type Block, type BlockPath, type NodeDefinition, type WorkflowDocument } from './model';
import type { FieldAddress } from './fields';
import { MAX_COORDINATE, dimensions, freshNote, position, presentationEdit, utf8Length } from './geometry';
import { blockReferences, rewriteBlockReferences } from './references';
import { schemaObject } from './schema';
import { requireVacantNodePresentation } from './presentation';
import { RESERVED_PORTS, TIMEOUT_PORT } from './ports';

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
  if (ids.some(id => block.nodes[id].type === 'start')) throw new CommandRejected('invalid_port');
  const selected = new Set(ids), ownsBlock = (at: BlockPath) => within(at, path) && at.length > path.length && selected.has(at[path.length].node);
  const ownsNode = (address: { block: BlockPath; node: string }) => ownsBlock(address.block) || sameBlockPath(address.block, path) && selected.has(address.node);
  const editor = document.editor;
  const fragment: Fragment = { format: 'rustx-workflow-fragment', source, block: path,
    nodes: Object.fromEntries(Object.entries(block.nodes).filter(([id]) => selected.has(id))),
    edges: edgesOf(block).filter(edge => selected.has(edge.from) && selected.has(edge.to)),
    editor: { nodes: editor?.nodes?.filter(entry => ownsNode(entry.address)), notes: editor?.notes?.filter(note => ownsBlock(note.block) || sameBlockPath(note.block, path) && noteIds.includes(note.id)),
      orders: editor?.orders?.filter(order => 'address' in order.target ? ownsNode(order.target.address) : ownsBlock(order.target.block)) },
  };
  const text = JSON.stringify(fragment);
  if (utf8Length(text) > MAX_BYTES) throw new CommandRejected('limit_exceeded');
  return text;
}

const fail = (): never => { throw new CommandRejected('invalid_fragment'); };
const VAR_TYPES: readonly string[] = ['string', 'number', 'boolean', 'object', 'array[string]', 'array[number]', 'array[boolean]', 'array[object]'];
const WRITE_MODES: readonly string[] = ['overwrite', 'clear', 'set', 'append', 'extend', 'remove_first', 'remove_last', 'increment', 'decrement', 'multiply', 'divide'];
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
function path(value: unknown): void { const steps = array(value); if (steps.length > 8) fail(); for (const step of steps) { const part = keys(step, ['node']); text(part.node); if (validLocalKey(part.node)) fail(); } }
function address(value: unknown): void { const at = keys(value, ['block', 'node']); path(at.block); text(at.node); if (validLocalKey(at.node)) fail(); }
function unsigned(value: unknown): void { if (typeof value !== 'number' || !Number.isSafeInteger(value) || value < 0 || value > 4294967295) fail(); }
/** A Model or Agent output: text, or one declared object schema. */
function output(value: unknown): void {
  const record = object(value);
  if (record.type === 'text') keys(record, ['type']); else if (record.type === 'json_schema') { keys(record, ['type', 'schema']); object(record.schema); } else fail();
}
function retry(input: unknown): void { if (input === undefined) return; const at = keys(input, ['max_retries', 'interval_ms']); unsigned(at.max_retries); unsigned(at.interval_ms); }
/** A Tool's or Agent's failure handling: stop, a whole default value, or the fail branch. */
function valueStrategy(input: unknown): void {
  if (input === undefined) return;
  const strategy = object(input);
  if (strategy.type === 'stop' || strategy.type === 'fail_branch') keys(strategy, ['type']);
  else if (strategy.type === 'default_value') { keys(strategy, ['type', 'value']); if (!Object.hasOwn(strategy, 'value')) fail(); }
  else fail();
}
function value(input: unknown): void {
  const record = object(input);
  switch (record.type) {
    case 'literal': keys(record, ['type', 'value']); if (!Object.hasOwn(record, 'value')) fail(); return;
    case 'reference': keys(record, ['type', 'path']); for (const part of array(record.path)) text(part); return;
    case 'object': keys(record, ['type', 'fields']); Object.values(object(record.fields)).forEach(value); return;
    case 'array': keys(record, ['type', 'items']); array(record.items).forEach(value); return;
    case 'template': keys(record, ['type', 'parts']); for (const part of array(record.parts)) { const at = object(part); if (at.type === 'literal' ? typeof at.value !== 'string' : at.type !== 'reference') fail(); value(part); } return;
    default: fail();
  }
}
function predicate(input: unknown): void {
  const record = object(input);
  switch (record.type) {
    case 'boolean': keys(record, ['type', 'value']); value(record.value); return;
    case 'equal': case 'not_equal': case 'contains': case 'not_contains': case 'starts_with': case 'ends_with': keys(record, ['type', 'left', 'right']); value(record.left); value(record.right); return;
    case 'empty': case 'not_empty': keys(record, ['type', 'value']); value(record.value); return;
    case 'not': keys(record, ['type', 'predicate']); predicate(record.predicate); return;
    case 'and': case 'or': keys(record, ['type', 'predicates']); array(record.predicates).forEach(predicate); return;
    default: fail();
  }
}
function node(input: unknown): void {
  const record = object(input);
  switch (record.type) {
    case 'start': keys(record, ['type']); return;
    case 'agent':
      keys(record, ['type', 'profile', 'task', 'output', 'routes', 'on_error']); text(record.profile); array(record.task).forEach(value); output(record.output); valueStrategy(record.on_error);
      if (record.routes !== undefined) {
        const ids = new Set<string>();
        for (const item of array(record.routes)) {
          const route = keys(item, ['id', 'description']); text(route.id); text(route.description);
          if (validLocalKey(route.id as string) || RESERVED_PORTS.includes(route.id as string) || ids.has(route.id as string)) fail();
          ids.add(route.id as string);
        }
      }
      return;
    case 'model': {
      keys(record, ['type', 'model', 'messages', 'output', 'retry', 'on_error']); retry(record.retry);
      if (record.on_error !== undefined) {
        const strategy = object(record.on_error);
        if (strategy.type === 'stop' || strategy.type === 'fail_branch') keys(strategy, ['type']);
        else if (strategy.type === 'default_value') { keys(strategy, ['type', 'text', 'structured_output']); text(strategy.text); }
        else fail();
      }
      for (const message of array(record.messages)) {
        const entry = keys(message, ['role', 'content']);
        if (entry.role !== 'system' && entry.role !== 'user' && entry.role !== 'assistant') fail();
        if (entry.content !== undefined) array(entry.content).forEach(value);
      }
      if (record.model !== undefined) {
        const policy = object(record.model);
        if (policy.mode === 'session') keys(policy, ['mode']);
        else if (policy.mode === 'explicit') { keys(policy, ['mode', 'model', 'profile', 'request_params', 'max_output_tokens']); text(policy.model); if (policy.profile != null) text(policy.profile); if (policy.request_params !== undefined) object(policy.request_params); if (policy.max_output_tokens != null) unsigned(policy.max_output_tokens); }
        else fail();
      }
      output(record.output); return;
    }
    case 'tool': {
      keys(record, ['type', 'selector', 'arguments', 'result', 'retry', 'on_error']); value(record.arguments); retry(record.retry); valueStrategy(record.on_error);
      const selector = keys(record.selector, ['origin', 'name', 'source_id']); text(selector.name);
      if (selector.origin === 'source') text(selector.source_id); else if (selector.origin !== 'builtin' || selector.source_id !== undefined) fail();
      const result = keys(record.result, ['type', 'schema']);
      if (result.type === 'json') object(result.schema); else if (result.type !== 'text' || result.schema !== undefined) fail(); return;
    }
    case 'branch': {
      keys(record, ['type', 'cases']); const cases = array(record.cases);
      if (!cases.length || cases.length > 32) fail();
      const ids = new Set<string>();
      for (const item of cases) {
        const entry = keys(item, ['id', 'condition']); text(entry.id);
        if (validLocalKey(entry.id as string) || RESERVED_PORTS.includes(entry.id as string) || ids.has(entry.id as string)) fail();
        ids.add(entry.id as string); predicate(entry.condition);
      }
      return;
    }
    case 'return':
      keys(record, ['type', 'outputs']);
      for (const item of array(record.outputs)) { const output = keys(item, ['variable', 'value_selector']); text(output.variable); array(output.value_selector).forEach(text); }
      return;
    case 'review': {
      keys(record, ['type', 'content', 'actions', 'timeout_ms']); array(record.content).forEach(value);
      if (typeof record.timeout_ms !== 'string' || !/^[1-9]\d*$/.test(record.timeout_ms)) fail();
      const ids = new Set<string>();
      for (const item of array(record.actions)) {
        const action = keys(item, ['id', 'title']); text(action.id); text(action.title);
        if (validLocalKey(action.id as string) || RESERVED_PORTS.includes(action.id as string) || action.id === TIMEOUT_PORT || ids.has(action.id as string)) fail();
        ids.add(action.id as string);
      }
      return;
    }
    case 'loop': {
      keys(record, ['type', 'loop_variables', 'break_condition', 'loop_count', 'body']); unsigned(record.loop_count); block(record.body);
      if (record.break_condition != null) predicate(record.break_condition);
      for (const item of array(record.loop_variables ?? [])) { const variable = keys(item, ['label', 'var_type', 'value']); text(variable.label); if (!VAR_TYPES.includes(variable.var_type as string)) fail(); value(variable.value); }
      return;
    }
    case 'iteration': {
      keys(record, ['type', 'iterator', 'output', 'is_parallel', 'parallel_nums', 'error_handle_mode', 'flatten_output', 'body']); value(record.iterator); value(record.output); block(record.body);
      if (record.is_parallel !== undefined && typeof record.is_parallel !== 'boolean') fail();
      if (record.parallel_nums !== undefined) unsigned(record.parallel_nums);
      if (record.error_handle_mode !== undefined && !['terminated', 'continue_on_error', 'remove_abnormal_output'].includes(record.error_handle_mode as string)) fail();
      if (record.flatten_output !== undefined && typeof record.flatten_output !== 'boolean') fail();
      return;
    }
    case 'loop_end': keys(record, ['type']); return;
    case 'assigner':
      keys(record, ['type', 'items']);
      for (const item of array(record.items)) {
        const write = keys(item, ['variable', 'operation', 'value']); array(write.variable).forEach(text);
        if (!WRITE_MODES.includes(write.operation as string)) fail();
        if (write.value !== undefined) value(write.value);
      }
      return;
    default: fail();
  }
}
function edges(input: unknown): void { for (const item of array(input ?? [])) { const edge = keys(item, ['from', 'to', 'port']); text(edge.from); text(edge.to); if (edge.port != null && typeof edge.port !== 'boolean' && (typeof edge.port !== 'string' || validLocalKey(edge.port))) fail(); } }
function nodes(input: unknown): void { for (const [id, item] of Object.entries(object(input))) { if (validNodeId(id)) fail(); node(item); } }
function block(input: unknown): void { const record = keys(input, ['input', 'nodes', 'edges']); if (record.input !== undefined) object(record.input); nodes(record.nodes); edges(record.edges); }

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
  const metadata = keys(record.editor, ['nodes', 'notes', 'orders']);
  const lists = Object.values(metadata).map(array);
  if (lists.reduce((count, list) => count + list.length, 0) > 1024 || array(metadata.notes ?? []).length > 128 || utf8Length(JSON.stringify(metadata)) > 256 * 1024) throw new CommandRejected('limit_exceeded');
  for (const item of array(metadata.nodes ?? [])) {
    const entry = keys(item, ['address', 'title', 'description', 'position', 'dimensions']); address(entry.address); if (entry.title != null) text(entry.title); if (entry.description != null) text(entry.description);
    if (typeof entry.title === 'string' && utf8Length(entry.title) > 256 || typeof entry.description === 'string' && utf8Length(entry.description) > 4096) throw new CommandRejected('limit_exceeded');
    if (entry.position != null) point(entry.position); if (entry.dimensions != null) size(entry.dimensions);
  }
  for (const item of array(metadata.notes ?? [])) {
    const note = keys(item, ['id', 'block', 'text', 'theme', 'position', 'dimensions']); text(note.id); text(note.text); if (validLocalKey(note.id) || utf8Length(note.text) > 16384 || !['neutral', 'blue', 'green', 'amber', 'rose'].includes(note.theme as string)) fail();
    path(note.block); point(note.position); size(note.dimensions);
  }
  for (const item of array(metadata.orders ?? [])) {
    const order = keys(item, ['target', 'fields']), target = object(order.target), fields = array(order.fields); fields.forEach(text); unique(fields, value => value);
    if (fields.length > 256 || fields.some(field => !field || utf8Length(field as string) > 64)) fail();
    if (target.kind === 'node_output') { keys(target, ['kind', 'address', 'path']); address(target.address); }
    else if (target.kind === 'block_input') { keys(target, ['kind', 'block', 'path']); path(target.block); } else fail();
    const steps = array(target.path); if (steps.length > 32) fail();
    for (const step of steps) { const part = object(step); if (part.kind === 'property') { keys(part, ['kind', 'name']); text(part.name); if (!part.name || utf8Length(part.name) > 64) fail(); } else if (part.kind === 'items') keys(part, ['kind']); else fail(); }
  }
  unique(array(metadata.nodes ?? []), item => object(item).address);
  unique(array(metadata.notes ?? []), item => object(item).id);
  unique(array(metadata.orders ?? []), item => object(item).target);
  return raw as Fragment;
}

export interface PastedFragment { document: WorkflowDocument; nodes: string[]; notes: string[] }

export function pasteFragment(document: WorkflowDocument, source: string, path: BlockPath, fragment: Fragment): PastedFragment {
  if (Object.values(fragment.nodes).some(node => node.type === 'start')) throw new CommandRejected('invalid_fragment');
  const destination = blockAt(document, path); if (!destination) throw new CommandRejected('missing_node');
  // A selection owns private Block presentation, never the source Block's
  // entry or contract order. Do not let forged fragments adopt the destination.
  for (const at of [...fragment.editor.orders?.flatMap(order => 'block' in order.target ? [order.target.block] : []) ?? []]) {
    if (!within(at, fragment.block) || at.length <= fragment.block.length) fail();
  }
  const ids = Object.keys(fragment.nodes).sort(), original = new Set(ids);
  const block: Block = { input: {}, nodes: fragment.nodes, edges: fragment.edges };
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
