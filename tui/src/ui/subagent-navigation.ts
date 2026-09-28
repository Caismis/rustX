/**
 * Presentation-only selection helpers for the subagent inspection rows.
 *
 * The Runtime Client owns the rows and their `child_conversation_id` values.
 * This module owns only which row the user has highlighted; it never stores a
 * message, event, lifecycle fact, or transcript for a child.
 */

import type { RuntimeClientAgent } from "../protocol/app-server.ts";

/**
 * Selects the next known child in display order, wrapping at either end.
 *
 * A missing current selection starts at the first row for Down and at the
 * last row for Up. The returned AgentId survives every activation, so a resume
 * cannot create a second row or move selection to another child.
 */
export function cycleSubagentSelection(
  agents: readonly RuntimeClientAgent[],
  current: string | undefined,
  direction: -1 | 1,
): string | undefined {
  if (agents.length === 0) {
    return undefined;
  }
  const currentIndex = current === undefined
    ? -1
    : agents.findIndex((subagent) => subagent.agent_id === current);
  if (currentIndex < 0) {
    return direction > 0
      ? agents[0]!.agent_id
      : agents[agents.length - 1]!.agent_id;
  }
  const nextIndex = (currentIndex + direction + agents.length) % agents.length;
  return agents[nextIndex]!.agent_id;
}

/** Returns whether a selected id still names a row in authoritative state. */
export function hasSubagentSelection(
  agents: readonly RuntimeClientAgent[],
  selected: string | undefined,
): boolean {
  return selected !== undefined && agents.some(
    (subagent) => subagent.agent_id === selected,
  );
}
