/**
 * The presentation-only focus model over the pending interaction queue.
 *
 * The runtime owns every pending interaction; this module owns only *which one
 * the human-input surface is showing*. Focus is derived deterministically from
 * two inputs — the authoritative sorted pending list and the previously
 * focused routed identity — so it can be recomputed after any event,
 * replacement, or reconnect without consulting component instances:
 *
 * ```text
 * focus undefined            -> the smallest routed identity
 * focus still pending        -> unchanged (new arrivals never steal focus)
 * focus settled/removed      -> its successor in presentation order,
 *                               else the new last item
 * authoritative resync       -> the app resets focus to undefined first,
 *                               which lands on the smallest identity again
 * ```
 *
 * Presentation order is the lexicographic routed identity pair. It orders the
 * display and nothing else: it is not ownership, not settlement order, and not
 * execution order, and it is reconstructable from any authoritative snapshot,
 * so no durable sequence exists to stabilize it.
 *
 * Nothing in this module emits a response. Navigation returns the identity the
 * surface should show next; settling is a separate, typed act that always
 * names the exact `InteractionRef`.
 */

import type { InteractionRef, RoutedInteraction } from "../protocol/types.ts";

/** Total order over routed identities: conversation first, then interaction. */
export function compareInteractionRefs(
  left: InteractionRef,
  right: InteractionRef,
): number {
  return (
    left.conversation_id.localeCompare(right.conversation_id) ||
    left.interaction_id.localeCompare(right.interaction_id)
  );
}

/** The display spelling of one routed identity. */
export function interactionRefLabel(interaction: InteractionRef): string {
  return `${interaction.conversation_id}::${interaction.interaction_id}`;
}

/** Whether two routed identities name the same runtime interaction. */
export function sameInteractionRef(
  left: InteractionRef,
  right: InteractionRef,
): boolean {
  return compareInteractionRefs(left, right) === 0;
}

/** Finds one pending interaction by its exact routed identity. */
export function findPendingInteraction(
  pending: readonly RoutedInteraction[],
  interaction: InteractionRef,
): RoutedInteraction | undefined {
  return pending.find((entry) =>
    sameInteractionRef(entry.interaction, interaction),
  );
}

/**
 * The pending list is kept sorted by the projection; this module never relies
 * on that. Every derivation re-sorts a copy, so focus stays deterministic
 * even over an unordered input list.
 */
function sortedPending(
  pending: readonly RoutedInteraction[],
): RoutedInteraction[] {
  return pending
    .slice()
    .sort((left, right) =>
      compareInteractionRefs(left.interaction, right.interaction),
    );
}

/**
 * Reconciles the focused identity with the authoritative pending list.
 *
 * Pure and total: given the pending projection and the previously
 * focused identity, returns the identity the surface must show now. This is
 * the only place focus changes in response to runtime facts, so settling or
 * removing the focused interaction advances focus deterministically — to the
 * interaction that follows it in presentation order, or to the new last item
 * when the removed one was last — and removing an unfocused interaction never
 * disturbs the focus.
 */
export function reconcileInteractionFocus(
  pending: readonly RoutedInteraction[],
  current: InteractionRef | undefined,
): InteractionRef | undefined {
  const sorted = sortedPending(pending);
  if (sorted.length === 0) {
    return undefined;
  }
  if (current !== undefined) {
    if (findPendingInteraction(sorted, current) !== undefined) {
      return current;
    }
    // The focused interaction left the projection. Advance to its successor
    // in presentation order — the first identity greater than the removed
    // one — or fall back to the last remaining item.
    const successor = sorted.find(
      (entry) => compareInteractionRefs(entry.interaction, current) > 0,
    );
    return (successor ?? sorted[sorted.length - 1]!).interaction;
  }
  return sorted[0]!.interaction;
}

/**
 * Moves the focus through the pending queue, wrapping at both ends.
 *
 * Navigation is presentation-only: it answers "which interaction is shown",
 * never settles anything, and never emits a response. An unknown current
 * identity reconciles first, so a stale focus cannot navigate the surface
 * onto a nonexistent entry.
 */
export function moveInteractionFocus(
  pending: readonly RoutedInteraction[],
  current: InteractionRef | undefined,
  delta: -1 | 1,
): InteractionRef | undefined {
  const sorted = sortedPending(pending);
  if (sorted.length === 0) {
    return undefined;
  }
  const reconciled = reconcileInteractionFocus(sorted, current);
  if (reconciled === undefined) {
    return undefined;
  }
  const index = sorted.findIndex((entry) =>
    sameInteractionRef(entry.interaction, reconciled),
  );
  const next = (index + delta + sorted.length) % sorted.length;
  return sorted[next]!.interaction;
}
