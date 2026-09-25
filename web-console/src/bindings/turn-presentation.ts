import type { CompletedResponseView, RuntimeClientTranscriptEntry } from '../../../protocol/app-server/v22';

export type TurnNode =
  | { kind: 'entry'; entry: RuntimeClientTranscriptEntry }
  | { kind: 'tail'; key: string; response: CompletedResponseView; text: string };

/** Completion evidence, not Assistant placement, owns the tail. The native
 * closing cursor is its insertion boundary; origin identity survives paging
 * and lineage. No completion or usage is inferred from visible message rows. */
export function turnPresentation(entries: readonly RuntimeClientTranscriptEntry[]): TurnNode[] {
  const nodes: TurnNode[] = [];
  const emitted = new Set<string>();
  for (const entry of entries) {
    nodes.push({ kind: 'entry', entry });
    const response = entry.completed_response;
    if (!response) continue;
    const key = JSON.stringify([response.origin.conversation_id, response.origin.attempt_id]);
    if (emitted.has(key)) continue;
    emitted.add(key);
    const closing = entries.find(candidate => candidate.item.type === 'message' && candidate.item.message.id === response.closing_message_id);
    const text = closing?.item.type === 'message' && closing.item.message.role === 'assistant' ? closing.item.message.content.flatMap(block => block.type === 'text' || block.type === 'refusal' ? [block.text] : []).join('') : '';
    nodes.push({ kind: 'tail', key, response, text });
  }
  return nodes;
}
