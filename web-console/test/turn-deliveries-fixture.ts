import type { RuntimeClientDeliveryHead, RuntimeClientSnapshot, RuntimeClientTranscriptEntry, SessionFileReference, TurnProcessView } from '../../protocol/app-server/v59';
import { snapshot } from './fixture';

export function deliveryConversation(deferred = false): RuntimeClientSnapshot {
  const state = snapshot(), owner: TurnProcessView = { conversation_id: state.conversation_id, attempt_id: 'delivery-attempt', attempt_id_source: null,
    final_message_id: 'delivery-answer', final_message_locator: { type: 'by_message_id', message_id: 'delivery-answer' }, final_message_id_deferred: false,
    final_message_id_source: null, outcome: 'completed', control_cursor: '2', message_count: 3, tool_call_count: 2 };
  const file = (name: string, description = 'Delivered file'): SessionFileReference => ({ scope: { conversation_id: state.conversation_id, device: '1', inode: '2' },
    path: `out/${name}`, name, mime_type: name.endsWith('.md') ? 'text/markdown' : 'text/plain', description });
  const result = (position: string, files: SessionFileReference[]): RuntimeClientTranscriptEntry => ({ cursor: position, turn_process: owner,
    item: { type: 'message', message: { role: 'tool', id: `declaration-${position}`, occurrence: { assistant_message_id: `call-${position}`, block_index: 0 },
      tool_call_id: `present-${position}`, tool_id: 'tool-present', result: { status: { type: 'success' }, duration_ms: '1', content: [], deliveries: files } } } });
  const declarations = [result('2', ['report.md', 'b.txt', 'c.rs', 'd.png', 'e.bin', 'f.txt'].map(name => file(name))),
    result('3', [file('report.md', 'Latest report'), file('README.md', 'Instructions')])];
  const final: RuntimeClientTranscriptEntry = { cursor: '4', turn_process: owner, item: { type: 'message', message: { role: 'assistant', id: 'delivery-answer',
    content: [{ type: 'text', text: '## Delivery complete\n\nThe report and instructions are ready.\n\nAll checks passed.' }] } },
    completed_response: { origin: { conversation_id: state.conversation_id, attempt_id: owner.attempt_id, attempt_id_source: null, closing_message_id: 'delivery-answer' },
      closing_message_id: 'delivery-answer', closing_locator: { type: 'by_message_id', message_id: 'delivery-answer' }, deferred_fields: [], surface_revision: '1',
      completed_at: '2026-10-21T03:00:00Z', usage: { input_tokens: '100', output_tokens: '37', total_tokens: '137' } } };
  state.transcript.entries = [{ cursor: '1', item: { type: 'message', message: { role: 'user', source: 'human', id: 'delivery-user',
    content: [{ type: 'text', text: 'Deliver the report and instructions.' }] } } }, ...declarations, final];
  if (deferred) {
    state.message_descriptors = declarations.map(entry => {
      if (entry.item.type !== 'message' || entry.item.message.role !== 'tool') throw Error('Expected declaration');
      const locator = { type: 'by_ledger_position' as const, position: Number(entry.cursor) + 100 };
      return { identity: entry.item.message.id, locator, role: 'tool', kind: 'tool_result', block_count: '0', tool_call_count: '0', tool_calls: [],
        turn_process: owner, content: [], collections: [], deliveries: { domain: `messages.ledger:${locator.position}.deliveries`,
          total: String(entry.item.message.result.deliveries!.length), items: entry.item.message.result.deliveries!.map((file, index) => ({ locator, index, file,
            encoded_bytes: String(new TextEncoder().encode(JSON.stringify(file)).length), metadata_deferred: false, metadata_domain: `messages.ledger:${locator.position}.deliveries.${index}.metadata` })) as RuntimeClientDeliveryHead['items'] } };
    });
    state.transcript.deferred_entries = state.message_descriptors.map((value, index) => ({ offset: declarations[index].cursor, identity: value.identity!, value, content: [], deferred: true }));
    state.transcript.entries = state.transcript.entries.filter(entry => !declarations.includes(entry));
  }
  state.messages = state.transcript.entries.flatMap(entry => entry.item.type === 'message' ? [entry.item.message] : []);
  return state;
}
