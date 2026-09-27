import type { TurnProcessView, CompletedResponseView, RuntimeClientTranscriptEntry } from '../../../protocol/app-server/v25';

export type TurnNode =
  | { kind: 'process'; key: string; process: TurnProcessView }
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
  // Native cursors seat non-successful controls even when the anchor member is
  // outside this page. Membership is never inferred from neighboring entries.
  const controls = new Map<string, TurnProcessView>();
  for (const entry of entries) {
    const process = entry.turn_process ?? (entry.item.type === 'attempt_terminal' ? entry.item.turn : undefined);
    if (process && process.outcome !== 'completed') controls.set(JSON.stringify([process.conversation_id, process.attempt_id]), process);
  }
  for (const [key, process] of [...controls].sort((a, b) => BigInt(a[1].control_cursor) < BigInt(b[1].control_cursor) ? -1 : 1)) {
    const index = nodes.findIndex(node => node.kind === 'entry' && BigInt(node.entry.cursor) >= BigInt(process.control_cursor));
    const control: TurnNode = { kind: 'process', key, process };
    nodes.splice(index < 0 ? nodes.length : index, 0, control);
  }
  return nodes;
}
