/**
 * Presentation-only selection helpers for the subagent inspection rows.
 *
 * The Runtime Client owns the rows and their `child_conversation_id` values.
 * This module owns only which row the user has highlighted; it never stores a
 * message, event, lifecycle fact, or transcript for a child.
 */

import type { RuntimeClientSubagent } from "../protocol/types.ts";

/**
 * Selects the next known child in display order, wrapping at either end.
 *
 * A missing current selection starts at the first row for Down and at the
 * last row for Up. The returned value is the subagent identity solely because
 * that is the stable key already present in the Runtime Client snapshot.
 */
export function cycleSubagentSelection(
  subagents: readonly RuntimeClientSubagent[],
  current: string | undefined,
  direction: -1 | 1,
): string | undefined {
  if (subagents.length === 0) {
    return undefined;
  }
  const currentIndex = current === undefined
    ? -1
    : subagents.findIndex((subagent) => subagent.subagent_id === current);
  if (currentIndex < 0) {
    return direction > 0
      ? subagents[0]!.subagent_id
      : subagents[subagents.length - 1]!.subagent_id;
  }
  const nextIndex = (currentIndex + direction + subagents.length) % subagents.length;
  return subagents[nextIndex]!.subagent_id;
}

/** Returns whether a selected id still names a row in authoritative state. */
export function hasSubagentSelection(
  subagents: readonly RuntimeClientSubagent[],
  selected: string | undefined,
): boolean {
  return selected !== undefined && subagents.some(
    (subagent) => subagent.subagent_id === selected,
  );
}
