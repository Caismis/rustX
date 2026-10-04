import type { ConversationTurn, ConversationTurnId, ConversationTurnPage, RuntimeClientSnapshot } from '../../../../protocol/app-server/v34';
import { turnKey } from '../../client/transcript';

/** Native identity/location only. A live identity is independent of a frozen outline. */
export function currentTurnLocation(snapshot?: RuntimeClientSnapshot): string | undefined {
  const attempt=snapshot?.attempt;
  if (!snapshot || !attempt || attempt.phase.type==='settled') return;
  const id={conversation_id:snapshot.conversation_id,attempt_id:attempt.attempt_id};
  return snapshot.transcript.entries?.find(entry=>entry.turn_process && turnKey(entry.turn_process)===turnKey(id))?.turn_process?.control_cursor;
}

/** One bounded native page plus the current identity, never inferred from content. */
export function turnRailItems(page?: ConversationTurnPage, current?: ConversationTurnId, location?: string): ConversationTurn[] {
  const turns=page?.turns ?? [];
  if (!current || turns.some(turn=>turnKey(turn.id)===turnKey(current))) return turns;
  // Ordinal zero denotes the pinned live identity until the native outline supplies order/location.
  return [...turns,{id:current,ordinal:0,cursor:location && page && BigInt(location)<=BigInt(page.cut.transcript) ? location : null,preview:''}];
}
