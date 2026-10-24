import type { CheckpointPage, CheckpointPageItem, CheckpointReadCut, ContentRef, TraceContentBlock, TraceDetail, TraceInspectionHead, TraceInspectionMessage, TraceRecord } from '../../protocol/app-server/v57';
import type { TraceFieldReader } from '../src/client/trace';
import { checkpoint, snapshot } from './fixture';

/** The finite native protocol shapes used by real App Server detail reads. */
export function nativeTraceFixture(record: TraceRecord, detail: TraceDetail, cut: CheckpointReadCut = checkpoint(snapshot()).read_cut) {
  const pages = new Map<string, CheckpointPageItem[]>(), values = new Map<string, string>(), contents = new Map<string, string>();
  const domain = `trace.records.${record.id.slice(6)}.detail`;
  const addValue = (domain: string, item: number, path: string[], value: unknown) => values.set(JSON.stringify([domain, item, path]), JSON.stringify(value));
  const blocks = (items: TraceContentBlock[], position: number, messageId?: string) => {
    const domain = `messages.ledger:${position}.blocks`;
    pages.set(domain, items.map((block, index) => {
      const reference: ContentRef = { conversation_id: cut.conversation_id, source: { type: 'canonical_message_block', component: 'payload', locator: messageId ? { type: 'by_message_id', message_id: messageId } : { type: 'by_ledger_position', position }, block_index: index }, representation: block.type === 'text' || block.type === 'reasoning' || block.type === 'refusal' ? 'utf8_text' : block.type === 'tool_call' ? 'raw_json_utf8' : 'canonical_json_v1', version: 1, content_revision: 1, readable_end: 0 };
      const value = block.type === 'text' || block.type === 'reasoning' || block.type === 'refusal' ? block.text.text : block.type === 'tool_call' ? JSON.stringify(block.arguments.value) : block.type === 'json' ? JSON.stringify(block.value.value) : JSON.stringify(block.type === 'image' || block.type === 'file' ? block.artifact : block);
      reference.readable_end = new TextEncoder().encode(value).length; contents.set(JSON.stringify(reference), value);
      const metadata = block.type === 'tool_call' ? { id: block.call_id, tool_id: block.tool_id, name: block.name } : block;
      return { offset: String(index), identity: block.type, value: metadata, content: [{ path: ['value'], content_ref: reference, deferred: true }], deferred: true, body_complete: false };
    }));
    return { domain, total: String(items.length), revision: '1', loaded: '0', next_offset: items.length ? '0' : null };
  };
  const messages: TraceInspectionMessage[] = detail.messages.map((message, index) => ({ locator: { type: 'by_ledger_position', position: 100 + index }, identity: message.message_id, source: record.kind === 'user' ? 'human' : null, ordinary_input: record.kind === 'user', context_kind: null, preview: record.preview, attachments: record.attachments, fields: [], collections: [blocks(message.blocks, 100 + index, message.message_id)] }));
  let value: unknown;
  if (detail.request) {
    value = { id: record.id, kind: 'request', request: { model: detail.request.model }, truncated: true };
    for (const [key, value] of Object.entries(detail.request)) {
      const fieldDomain = `${domain}.request.${key}`;
      const rows = Array.isArray(value) ? value.map((_value, index) => ({ offset: String(index), identity: `${key}.${index}`, value: null, content: [], deferred: true }))
        : value != null && typeof value === 'object' ? Object.keys(value).map((name, index) => ({ offset: String(index), identity: `${key}.${name}`, value: null, content: [], deferred: true })) : [];
      pages.set(fieldDomain, rows);
      if (!rows.length) addValue(fieldDomain, 0, [], value);
      else rows.forEach((row, index) => addValue(fieldDomain, index, [], Array.isArray(value) ? value[index] : (value as Record<string, unknown>)[row.identity.slice(key.length + 1)]));
    }
  } else {
    const headRecord = { ...record, fields: [...record.fields], collections: [...record.collections], tool: record.tool ? { ...record.tool, fields: [...record.tool.fields] } : null };
    if (detail.tool) {
      const tool = detail.tool;
      if (tool.arguments) {
        const field = { type: 'value' as const, field: 'arguments' as const, domain: 'native.arguments', item_offset: 0, path: [], encoded_bytes: String(new TextEncoder().encode(JSON.stringify(tool.arguments.value)).length) };
        headRecord.tool!.fields.push(field); addValue(field.domain, 0, [], tool.arguments.value);
      }
      if (tool.result) { headRecord.collections.push(blocks(tool.result.blocks, 200)); headRecord.tool!.outcome = tool.result.outcome; }
      const definition = tool.definition ? { type: 'available' as const, field: { type: 'value' as const, field: 'definition' as const, domain: 'trace.records.2.detail.request.tools', item_offset: 0, path: [], encoded_bytes: String(new TextEncoder().encode(JSON.stringify(tool.definition)).length) } } : { type: 'absent' as const };
      if (tool.definition && definition.type === 'available') addValue(definition.field.domain, 0, [], tool.definition);
      value = { id: record.id, kind: 'tool', record: headRecord, content: { type: 'tool', definition } } satisfies TraceInspectionHead;
    } else value = { id: record.id, kind: record.kind, record: headRecord, content: { type: 'messages', page: { domain: `${domain}.messages`, total: String(messages.length), items: messages, next_offset: null } } } satisfies TraceInspectionHead;
  }
  const page: CheckpointPage = { read_cut: cut, cursor: cut.revision, domain, total: '1', next_offset: null, collections: [], items: [{ offset: '0', identity: record.id, value, deferred: true, body_complete: false, content: [] }] };
  type NativeSource = { [Key in keyof TraceFieldReader]: (...args: Parameters<TraceFieldReader[Key]>) => Awaited<ReturnType<TraceFieldReader[Key]>> };
  const original: NativeSource = {
    page(cut, domain, offset, current) {
      if (!current()) throw Error('Fixture view replaced');
      const items = pages.get(domain); if (!items) throw Error(`Unexpected native page ${domain}`);
      const end = Math.min(offset + 8, items.length);
      return { read_cut: cut, cursor: cut.revision, domain, total: String(items.length), items: items.slice(offset, end), next_offset: end < items.length ? String(end) : null, collections: [] };
    },
    value(cut, field, offset, current) {
      if (!current()) throw Error('Fixture view replaced');
      const value = values.get(JSON.stringify([field.domain, field.item_offset, field.path])); if (value === undefined) throw Error(`Unexpected native value ${JSON.stringify(field)}`);
      const bytes = new TextEncoder().encode(value), end = Math.min(offset + 65536, bytes.length);
      return { cut, ...field, offset, readable_end: bytes.length, next_offset: end < bytes.length ? end : null, representation: 'canonical_json_v1', bytes_base64: btoa(String.fromCharCode(...bytes.slice(offset, end))) };
    },
    content(_cut, reference, offset, current) {
      if (!current()) throw Error('Fixture view replaced');
      const value = contents.get(JSON.stringify(reference)); if (value === undefined) throw Error('Unexpected native content');
      const bytes = new TextEncoder().encode(value), end = Math.min(offset + 65536, bytes.length);
      return { content_ref: reference, offset, data_base64: btoa(String.fromCharCode(...bytes.slice(offset, end))), end_offset: end, readable_end: bytes.length, next_offset: end < bytes.length ? end : null, complete: end === bytes.length };
    },
  };
  const reader: TraceFieldReader = { page: async (...args) => original.page(...args), value: async (...args) => original.value(...args), content: async (...args) => original.content(...args) };
  return { page, reader, pages, values, contents, original };
}
