import type { ConversationTurn, ConversationTurnId, ConversationTurnPage, RuntimeClientSnapshot } from '../../../../protocol/app-server/v36';
import { turnKey } from '../../client/transcript';

/** Native outline pages hold at most this many turns. */
export const OUTLINE_PAGE = 64;

/** One rail mark. A loaded mark carries its native turn; an unloaded one is
 * known only by ordinal and pages its outline in before navigating. */
export interface TurnRailItem {
  /** Stable across paging: ordinals never move within append-only cuts. */
  readonly key: string;
  /** Native identity, known only once the mark's page is loaded. */
  readonly id?: string;
  /** One-based native ordinal; zero is the pinned live identity. */
  readonly ordinal: number;
  readonly turn?: ConversationTurn;
}

/** Native identity/location only. A live identity is independent of a frozen outline. */
export function currentTurnLocation(snapshot?: RuntimeClientSnapshot): string | undefined {
  const attempt=snapshot?.attempt;
  if (!snapshot || !attempt || attempt.phase.type==='settled') return;
  const id={conversation_id:snapshot.conversation_id,attempt_id:attempt.attempt_id};
  return snapshot.transcript.entries?.find(entry=>entry.turn_process && turnKey(entry.turn_process)===turnKey(id))?.turn_process?.control_cursor;
}

/** Every turn the native outline counts, loaded from its one bounded page,
 * plus the current identity, never inferred from content. */
export function turnRailItems(page?: ConversationTurnPage, current?: ConversationTurnId, location?: string): TurnRailItem[] {
  const items: TurnRailItem[] = [];
  for (let ordinal = 1; ordinal <= (page?.total ?? 0); ordinal++) {
    const turn = page!.turns[ordinal - page!.offset - 1];
    items.push(turn?.ordinal === ordinal ? { key: `ordinal:${ordinal}`, id: turnKey(turn.id), ordinal, turn } : { key: `ordinal:${ordinal}`, ordinal });
  }
  const newest = !page || page.offset + page.turns.length >= page.total;
  if (!current || !newest || page?.turns.some(turn=>turnKey(turn.id)===turnKey(current))) return items;
  // Ordinal zero pins the live identity until the native outline supplies its order and location.
  const cursor = location && page && BigInt(location)<=BigInt(page.cut.transcript) ? location : null;
  return [...items, { key: 'current', id: turnKey(current), ordinal: 0, turn: { id: current, ordinal: 0, cursor, prompt: '', response: '' } }];
}
