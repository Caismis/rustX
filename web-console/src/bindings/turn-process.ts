import { statusesAt, type AgentStatusPlacement } from './agent-status';
import type { RuntimeClientTranscriptEntry } from '../../../protocol/app-server/v22';
/** Only exact native identity is a grouping key. Canonical order is untouched;
 * pagination can add members without changing the disclosure's identity. */
export type ProcessSeat = { cursor: string; kind: 'entry' } | { cursor: string; kind: 'status'; statusId: string };
export function turnProcesses(entries: readonly RuntimeClientTranscriptEntry[], placement?: AgentStatusPlacement, conversationId?: string) {
  const groups = new Map<string, { seat?: ProcessSeat; tools: number; messages: number }>();
  const membership = new Map<string, string>();
  const attempts = new Map<string, string>();
  for (const entry of entries) {
    const process = entry.completed_process;
    if (!process || entry.response_pending) continue;
    const key = JSON.stringify([process.origin.conversation_id, process.origin.attempt_id, process.final_message_id]);
    const final = entry.item.type === 'message' && entry.item.message.id === process.final_message_id;
    const foldable = !final || entry.item.type === 'message' && entry.item.message.role === 'assistant' && entry.item.message.content.some(b => b.type === 'reasoning');
    attempts.set(JSON.stringify([process.origin.conversation_id, process.origin.attempt_id]), key);
    if (!foldable) continue;
    membership.set(entry.cursor, key);
    const group = groups.get(key) ?? { tools: 0, messages: 0 };
    group.tools += entry.tool_calls?.length ?? 0;
    if (!final && entry.item.type === 'message' && entry.item.message.role === 'assistant') group.messages++;
    groups.set(key, group);
  }
  // Native membership and Status anchors do not move. Only the disclosure
  // seat follows presentation order (entry body, then its anchored statuses).
  // Index completed owners even when their only member is final text: an exact
  // owned Status can be the entire foldable presentation of that process.
  for (const entry of entries) {
    const key = membership.get(entry.cursor);
    if (key) {
      const group = groups.get(key)!;
      group.seat ??= { kind: 'entry', cursor: entry.cursor };
    }
    if (!placement) continue;
    for (const status of statusesAt(placement, { cursor: entry.cursor, messageId: entry.item.type === 'message' ? entry.item.message.id : undefined })) {
      const owner = attempts.get(JSON.stringify([conversationId, status.attempt_id]));
      if (!owner) continue;
      const group = groups.get(owner) ?? { tools: 0, messages: 0 };
      group.seat ??= { kind: 'status', cursor: entry.cursor, statusId: status.status_message_id };
      groups.set(owner, group);
    }
  }
  return { groups, membership, attempts };
}
