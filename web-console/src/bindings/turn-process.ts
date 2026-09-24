import type { RuntimeClientTranscriptEntry } from '../../../protocol/app-server/v21';
/** Only exact native identity is a grouping key. Canonical order is untouched;
 * pagination can add members without changing the disclosure's identity. */
export function turnProcesses(entries: readonly RuntimeClientTranscriptEntry[]) {
  const groups = new Map<string, { first: string; tools: number; messages: number }>();
  const membership = new Map<string, string>();
  const attempts = new Map<string, string>();
  for (const entry of entries) {
    const process = entry.completed_process;
    if (!process || entry.response_pending) continue;
    const key = JSON.stringify([process.origin.conversation_id, process.origin.attempt_id, process.final_message_id]);
    const final = entry.item.type === 'message' && entry.item.message.id === process.final_message_id;
    const foldable = !final || entry.item.type === 'message' && entry.item.message.role === 'assistant' && entry.item.message.content.some(b => b.type === 'reasoning');
    if (!foldable) continue;
    membership.set(entry.cursor, key);
    const group = groups.get(key) ?? { first: entry.cursor, tools: 0, messages: 0 };
    group.tools += entry.tool_calls?.length ?? 0;
    if (!final && entry.item.type === 'message' && entry.item.message.role === 'assistant') group.messages++;
    groups.set(key, group);
    attempts.set(JSON.stringify([process.origin.conversation_id, process.origin.attempt_id]), key);
  }
  return { groups, membership, attempts };
}
