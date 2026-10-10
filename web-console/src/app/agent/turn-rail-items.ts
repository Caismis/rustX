import type { ConversationTurn, ConversationTurnId, ConversationTurnPage, RuntimeClientSnapshot } from '../../../../protocol/app-server/v43';
import { turnKey } from '../../client/transcript';

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

/** Retains only the native page and at most one live identity. Ordinal indexes
 * are arithmetic; unloaded marks are created only for the visible range. */
export function turnRail(page?: ConversationTurnPage, current?: ConversationTurnId, location?: string) {
  const total = page?.total ?? 0;
  const newest = !page || page.offset + page.turns.length >= total;
  const pinned = current && newest && !page?.turns.some(turn => turnKey(turn.id) === turnKey(current))
    ? { id: current, ordinal: 0, cursor: location && page && BigInt(location) <= BigInt(page.cut.transcript) ? location : null, prompt: '', response: '' } : undefined;
  const count = total + (pinned ? 1 : 0);
  return {
    count,
    item(index: number): TurnRailItem | undefined {
      if (!Number.isInteger(index) || index < 0 || index >= count) return;
      if (index === total && pinned) return { key: 'current', id: turnKey(pinned.id), ordinal: 0, turn: pinned };
      const ordinal = index + 1, turn = page?.turns[index - page.offset];
      return turn?.ordinal === ordinal ? { key: `ordinal:${ordinal}`, id: turnKey(turn.id), ordinal, turn } : { key: `ordinal:${ordinal}`, ordinal };
    },
    indexOfKey(key: string | null): number | undefined {
      if (key === 'current') return pinned ? total : undefined;
      if (!key?.startsWith('ordinal:')) return;
      const ordinal = Number(key.slice(8));
      return Number.isSafeInteger(ordinal) && ordinal > 0 && ordinal <= total ? ordinal - 1 : undefined;
    },
    indexOfTurn(id: string | undefined): number | undefined {
      if (!id) return;
      const turn = page?.turns.find(turn => turnKey(turn.id) === id);
      return turn ? turn.ordinal - 1 : pinned && turnKey(pinned.id) === id ? total : undefined;
    },
  };
}
export const TURN_SPACING_PX = 10;
export const RAIL_INSET_PX = 6;
/** Fixed-pitch virtualization needs no O(total) measurements cache. */
export function turnRailRange(count: number, top: number, height: number, focus?: number): number[] {
  if (height <= 0) return [];
  const start = Math.max(0, Math.floor((top - RAIL_INSET_PX) / TURN_SPACING_PX) - 3);
  const end = Math.min(count - 1, Math.ceil((top + height - RAIL_INSET_PX) / TURN_SPACING_PX) + 3);
  const range: number[] = [];
  for (let index = start; index <= end; index++) range.push(index);
  if (focus !== undefined) for (let index = Math.max(0, focus - 1); index <= Math.min(count - 1, focus + 1); index++) if (!range.includes(index)) range.push(index);
  return range.sort((a, b) => a - b);
}
