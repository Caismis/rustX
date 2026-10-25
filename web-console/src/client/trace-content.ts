import type { CheckpointPage, CheckpointPageItem, CheckpointReadCut, CollectionFrontier, CanonicalContentLocator, ContentBinding, TraceContentBlock, TraceFieldSource, TraceInspectionMessage, TraceInspectionMessages, TraceToolSource, RuntimeClientToolOccurrence } from '../../../protocol/app-server/v57';
import { checkpointReadCutKey } from '../../../protocol/app-server/content-identity';
import { ContentCache } from '../../../protocol/app-server/content-cache';
import { readTraceFieldWindow, readTraceSelectedWindow, type TraceValueField, type TraceValueSelector } from '../../../protocol/app-server/trace-field';
import { validateCanonicalInspectionPage, traceInspectionMessage } from '../../../protocol/app-server/trace-inspection';
import { parseJson } from '../presentation/primitives/json';
import { readContentText } from './content-text';
import type { TraceFieldReader } from './trace';

export async function readTraceValue(reader: TraceFieldReader, cut: CheckpointReadCut, field: TraceValueField | TraceValueSelector, current: () => boolean) {
  const decoder = new TextDecoder('utf-8', { fatal: true, ignoreBOM: true }), parts: string[] = [];
  let offset: number | undefined = 0, end: number | undefined, representation: string | undefined;
  do {
    const read = (at: number) => reader.value(cut, field, at, current);
    const chunk: Awaited<ReturnType<typeof readTraceSelectedWindow>> = 'encoded_bytes' in field ? await readTraceFieldWindow(field, cut, offset, read, current)
      : await readTraceSelectedWindow(field, cut, offset, read, current, end);
    if (representation !== undefined && representation !== chunk.representation) throw Error('Trace field representation changed.');
    representation = chunk.representation; end = chunk.readable_end;
    parts.push(decoder.decode(chunk.bytes, { stream: chunk.next !== undefined })); offset = chunk.next;
  } while (offset !== undefined && current());
  if (!current()) throw Error('Trace content view was replaced.');
  return { text: parts.join(''), representation };
}
export async function readBinding(reader: Pick<TraceFieldReader, 'content'>, cut: CheckpointReadCut, binding: ContentBinding, current: () => boolean): Promise<unknown> {
  const cache = new ContentCache((ref, offset, _max, owned) => reader.content(cut, ref, offset, owned));
  try {
    const text = await readContentText(cache, binding.content_ref, current);
    const value = binding.content_ref.representation === 'utf8_text' ? text : parseJson(text);
    let selected = value;
    for (const key of binding.content_ref.representation === 'raw_json_utf8' ? [] : binding.source_path ?? []) {
      if (selected == null || typeof selected !== 'object' || !(key in selected)) throw Error('Original Trace content path is unavailable.');
      selected = (selected as Record<string, unknown>)[key];
    }
    return selected;
  } finally { cache.clear(); }
}
export async function readTraceSource(reader: TraceFieldReader, cut: CheckpointReadCut, field: TraceFieldSource, current: () => boolean): Promise<unknown> {
  if (field.type === 'content') return readBinding(reader, cut, field.binding, current);
  const value = await readTraceValue(reader, cut, field, current);
  return value.representation === 'utf8_text' ? value.text : parseJson(value.text);
}
export type TraceDisplayBlock = TraceContentBlock & { nativeOccurrence?: RuntimeClientToolOccurrence };
const object = (value: unknown): Record<string, unknown> => value != null && typeof value === 'object' && !Array.isArray(value) ? value as Record<string, unknown> : {};
const text = (value: string) => ({ text: value, truncated: false });
function checkPage(page: CheckpointPage, cut: CheckpointReadCut, domain: string) {
  if (page.domain !== domain || page.cursor !== cut.revision || checkpointReadCutKey(page.read_cut) !== checkpointReadCutKey(cut)
    || !Array.isArray(page.items) || page.items.length > 32 || new TextEncoder().encode(JSON.stringify(page)).length > 786432) throw Error('Original Trace content page changed.');
}
export async function readTraceCollection(reader: Pick<TraceFieldReader, 'page'>, cut: CheckpointReadCut, frontier: Pick<CollectionFrontier, 'domain' | 'total'>, current: () => boolean, first?: CheckpointPage, start = 0) {
  const items: CheckpointPageItem[] = []; let offset = start;
  do {
    if (!current()) throw Error('Trace content view was replaced.');
    const page = offset === start && first ? first : await reader.page(cut, frontier.domain, offset, current);
    if (!current()) throw Error('Trace content view was replaced.');
    checkPage(page, cut, frontier.domain); validateCanonicalInspectionPage(page, offset, frontier.total, offset !== 0 && BigInt(offset) < BigInt(frontier.total));
    items.push(...page.items);
    if (page.next_offset == null) return items;
    offset = Number(page.next_offset);
  } while (current());
  throw Error('Trace content view was replaced.');
}
/** Read the original canonical blocks once; a tools frontier is an index of the same calls. */
export async function readTraceBlocks(reader: TraceFieldReader, cut: CheckpointReadCut, collections: readonly CollectionFrontier[], current: () => boolean, first?: CheckpointPage, owner?: CanonicalContentLocator): Promise<TraceDisplayBlock[] | undefined> {
  const frontier = collections.find(collection => /^messages\.ledger:[1-9][0-9]*\.blocks$/.test(collection.domain));
  if (!frontier) return;
  if (owner?.type === 'by_ledger_position' && frontier.domain !== `messages.ledger:${owner.position}.blocks`) throw Error('Original message blocks owner changed.');
  const rows = await readTraceCollection(reader, cut, frontier, current, first), blocks: TraceDisplayBlock[] = [];
  for (const row of rows) {
    let metadata = object(row.value);
    const metadataBinding = row.content.find(binding => binding.content_ref.source.component === 'block_metadata');
    if (row.value == null && metadataBinding) metadata = object(await readBinding(reader, cut, metadataBinding, current));
    const payload = row.content.find(binding => binding.content_ref.source.component === 'payload' && binding.content_ref.representation !== 'binary_bytes');
    const value = payload ? await readBinding(reader, cut, payload, current) : undefined;
    const kind = row.identity;
    if (kind === 'text' || kind === 'reasoning' || kind === 'refusal') {
      const body = typeof value === 'string' ? value : metadata.text;
      if (typeof body === 'string') blocks.push({ type: kind, text: text(body) });
    } else if (kind === 'tool_call') {
      const callId = metadata.id ?? metadata.call_id;
      if (typeof callId !== 'string' || typeof metadata.tool_id !== 'string' || typeof metadata.name !== 'string') throw Error('Original Tool proposal metadata is unavailable.');
      const source = payload?.content_ref.source;
      blocks.push({ type: 'tool_call', call_id: callId, tool_id: metadata.tool_id, name: metadata.name, arguments: { value: value ?? metadata.arguments, truncated: false }, ...(source?.type === 'canonical_message_block' ? { nativeOccurrence: { locator: owner ?? source.locator, block_index: source.block_index } } : {}) });
    } else if (kind === 'json') {
      blocks.push({ type: 'json', value: { value: value ?? metadata.value, truncated: false } });
    } else if (kind === 'image' || kind === 'file') {
      const file = { ...metadata, ...object(value) };
      if (typeof file.artifact_id === 'string') blocks.push(kind === 'image' ? { type: 'image', artifact: { artifact_id: file.artifact_id, image: true }, alt: typeof file.alt === 'string' ? file.alt : undefined }
        : { type: 'file', artifact: { artifact_id: file.artifact_id, image: false, name: typeof file.name === 'string' ? file.name : undefined, mime_type: typeof file.mime_type === 'string' ? file.mime_type : undefined } });
    } else if (kind === 'uploaded_file' && typeof metadata.name === 'string') blocks.push({ type: 'upload', name: metadata.name });
  }
  return blocks;
}
export async function readTraceMessages(reader: TraceFieldReader, cut: CheckpointReadCut, head: TraceInspectionMessages, current: () => boolean) {
  const messages: TraceInspectionMessage[] = [...head.items]; let next = head.next_offset;
  while (next != null) {
    const page = await reader.page(cut, head.domain, next, current);
    if (!current()) throw Error('Trace content view was replaced.');
    checkPage(page, cut, head.domain); validateCanonicalInspectionPage(page, next, head.total, true);
    messages.push(...page.items.map(item => {
      if (!item.deferred || item.body_complete !== false) throw Error('Original Trace message descriptor changed.');
      return traceInspectionMessage(item.value);
    })); next = page.next_offset == null ? undefined : Number(page.next_offset);
  }
  const blocks: TraceDisplayBlock[] = [];
  for (const message of messages) {
    const body = await readTraceBlocks(reader, cut, message.collections, current, undefined, message.locator);
    blocks.push(...body ?? (message.preview ? [{ type: 'text' as const, text: message.preview }] : []));
  }
  return { blocks, source: messages[0]?.source, attachments: messages.flatMap(message => message.attachments) };
}
/** Request fields retain the native child ordinals and original value windows. */
export async function readRequestField(reader: TraceFieldReader, cut: CheckpointReadCut, domain: string, current: () => boolean): Promise<unknown> {
  let offset = 0, total: string | undefined; const rows: CheckpointPageItem[] = [];
  do {
    const page = await reader.page(cut, domain, offset, current);
    if (!current()) throw Error('Trace content view was replaced.');
    checkPage(page, cut, domain); total ??= page.total; validateCanonicalInspectionPage(page, offset, total, offset !== 0);
    if (page.total === '0') {
      const value = await readTraceValue(reader, cut, { domain, item_offset: 0, path: [] }, current);
      return value.representation === 'utf8_text' ? value.text : parseJson(value.text);
    }
    rows.push(...page.items);
    if (page.next_offset == null) break;
    offset = Number(page.next_offset);
  } while (current());
  const field = domain.split('.detail.request.')[1];
  if (!field) throw Error('Original request field is unavailable.');
  const array = ['options', 'tools', 'contributions', 'messages'].includes(field);
  const entries: [string, unknown][] = [];
  for (const row of rows) {
    const value = await readTraceValue(reader, cut, { domain, item_offset: Number(row.offset), path: [] }, current);
    const key = row.key?.preview ?? (row.identity.startsWith(`${field}.`) ? row.identity.slice(field.length + 1) : undefined);
    if (key === undefined) throw Error('Original request field identity changed.');
    entries.push([key, value.representation === 'utf8_text' ? value.text : parseJson(value.text)]);
  }
  return array ? entries.map(([, value]) => value) : Object.fromEntries(entries);
}
/** These native contracts match runtime_client/trace/tool.rs, never a name or file extension. */
export function traceProgram(toolId: string | null | undefined, args: unknown): TraceToolSource | undefined {
  const field = toolId === 'tool-bash' ? 'command' : toolId === 'tool-write' ? 'content' : undefined;
  if (!field) return;
  const body = object(args)[field];
  return typeof body === 'string' ? { field, text: text(body), language: toolId === 'tool-bash' ? 'shell' : undefined } : undefined;
}
