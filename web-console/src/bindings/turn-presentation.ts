import { attemptOwnerKey } from '../../../protocol/app-server/attempt-identity';
import type { TurnProcessView, CompletedResponseView, RuntimeClientTranscriptEntry, RuntimeClientTranscriptPage } from '../../../protocol/app-server/v57';

export type TurnNode =
  | { kind: 'fork-point' }
  | { kind: 'process'; key: string; process: TurnProcessView }
  | { kind: 'entry'; entry: RuntimeClientTranscriptEntry }
  | { kind: 'tail'; key: string; response: CompletedResponseView; text: string };

/** Completion evidence, not Assistant placement, owns the tail. The native
 * closing cursor is its insertion boundary; origin identity survives paging
 * and lineage. No completion or usage is inferred from visible message rows. */
export function turnPresentation(entries: readonly RuntimeClientTranscriptEntry[], forkPage?: RuntimeClientTranscriptPage, conversationId?: string): TurnNode[] {
  const nodes: TurnNode[] = [];
  const emitted = new Set<string>();
  for (const entry of entries) {
    nodes.push({ kind: 'entry', entry });
    const response = entry.completed_response;
    if (!response) continue;
    const key = attemptOwnerKey(response.origin);
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
    if (process && process.outcome !== 'completed') controls.set(attemptOwnerKey(process), process);
  }
  for (const [key, process] of [...controls].sort((a, b) => BigInt(a[1].control_cursor) < BigInt(b[1].control_cursor) ? -1 : 1)) {
    // A frozen publication is not a canonical Assistant message. Its explicit
    // native Attempt identity still seats the process control above that output.
    // Imported owners cannot borrow an audit from the current conversation.
    const index = nodes.findIndex(node => node.kind === 'entry' && (BigInt(node.entry.cursor) >= BigInt(process.control_cursor)
      || process.conversation_id === conversationId && process.attempt_id != null && node.entry.item.type === 'publication_audit'
        && node.entry.item.audit.attempt_id === process.attempt_id));
    const control: TurnNode = { kind: 'process', key, process };
    nodes.splice(index < 0 ? nodes.length : index, 0, control);
  }
  const through = forkPage?.inherited_through;
  if (forkPage && through != null) {
    const boundary = BigInt(through), first = entries[0], last = entries.at(-1);
    // Show only when the finite window reaches both sides of the cut, or
    // contains its inherited endpoint. An empty fork still has a boundary.
    if ((!first && forkPage.next_cursor == null && boundary === 0n) || first && last && BigInt(last.cursor) >= boundary && (BigInt(first.cursor) <= boundary + 1n || forkPage.next_cursor == null)) {
      const index = nodes.findIndex(node => node.kind === 'entry' && BigInt(node.entry.cursor) > boundary || node.kind === 'process' && BigInt(node.process.control_cursor) > boundary);
      nodes.splice(index < 0 ? nodes.length : index, 0, { kind: 'fork-point' });
    }
  }
  return nodes;
}
