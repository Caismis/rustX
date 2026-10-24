import { attemptOwnerKey } from '../../../protocol/app-server/attempt-identity';
import { sameCanonicalLocator, statusIdentity } from '../../../protocol/app-server/projection';
import { statusesAt, type AgentStatusPlacement } from './agent-status';
import type { TurnProcessView, RuntimeClientTranscriptEntry, RuntimeClientMessageDescriptor } from '../../../protocol/app-server/v57';
/** Native completion owns finality; a canonical locator only identifies its member. */
export function isFinalTurnMessage(entry: RuntimeClientTranscriptEntry): boolean {
  const process=entry.turn_process;
  if(process?.outcome!=='completed' || entry.item.type!=='message' || entry.item.message.role!=='assistant')return false;
  if(process.final_message_id!=null)return entry.item.message.id===process.final_message_id;
  const locator=process.final_message_locator;
  if(!locator)return false;
  return locator.type==='by_message_id'&&locator.message_id===entry.item.message.id;
}
/** A typed deferred descriptor supplies the exact canonical locator that a loaded row omits. */
export function isFinalTurnDescriptor(descriptor: RuntimeClientMessageDescriptor): boolean {
  const process=descriptor.turn_process;
  return descriptor.role==='assistant' && process?.outcome==='completed'
    && process.final_message_locator!=null && sameCanonicalLocator(descriptor.locator,process.final_message_locator);
}
/** Only exact native identity is a grouping key. Canonical order is untouched;
 * pagination can add members without changing the disclosure's identity. */
export type ProcessSeat = { cursor: string; kind: 'entry' } | { cursor: string; kind: 'status'; statusId: string };
export function turnProcesses(entries: readonly RuntimeClientTranscriptEntry[], placement?: AgentStatusPlacement, conversationId?: string) {
  const groups = new Map<string, { seat?: ProcessSeat; tools: number; messages: number; owner: TurnProcessView }>();
  const membership = new Map<string, string>();
  const attempts = new Map<string, string>();
  const finalMetadataSeats = new Map<string,string>();
  for (const entry of entries) {
    const process = entry.turn_process;
    if (!process) continue;
    const key = attemptOwnerKey(process);
    if(process.outcome==='completed'&&process.final_message_id_deferred&&process.final_message_id_source&&!finalMetadataSeats.has(key))finalMetadataSeats.set(key,entry.cursor);
    const final = isFinalTurnMessage(entry);
    const foldable = process.outcome !== 'completed' || !final || entry.item.type === 'message' && entry.item.message.role === 'assistant' && entry.item.message.content.some(b => b.type === 'reasoning');
    attempts.set(attemptOwnerKey(process), key);
    if (!foldable) continue;
    membership.set(entry.cursor, key);
    const group = groups.get(key) ?? { tools: process.tool_call_count, messages: process.message_count, owner: process };
    groups.set(key, group);
  }
  // Native membership and Status anchors do not move. Only the disclosure
  // seat follows presentation order (entry body, then its anchored statuses).
  // Index completed owners even when their only member is final text: an exact
  // owned Status can be the entire foldable presentation of that process.
  for (const entry of entries) {
    const key = membership.get(entry.cursor);
    if (key && groups.get(key)!.owner.outcome === 'completed') {
      const group = groups.get(key)!;
      group.seat ??= { kind: 'entry', cursor: entry.cursor };
    }
    if (!placement) continue;
    for (const status of statusesAt(placement, { cursor: entry.cursor, messageId: entry.item.type === 'message' ? entry.item.message.id : undefined })) {
      const owner = attempts.get(JSON.stringify([conversationId, status.attempt_id]));
      if (!owner) continue;
      const spec = entries.find(entry => entry.turn_process && attemptOwnerKey(entry.turn_process) === owner)!.turn_process!;
      if (spec.outcome !== 'completed') continue;
      const group: { seat?: ProcessSeat; tools: number; messages: number; owner: TurnProcessView } = groups.get(owner) ?? { tools: spec.tool_call_count, messages: spec.message_count, owner: spec };
      group.seat ??= { kind: 'status', cursor: entry.cursor, statusId: statusIdentity(status) };
      groups.set(owner, group);
    }
  }
  return { groups, membership, attempts, finalMetadataSeats };
}
