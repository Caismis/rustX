import type { DisplayInteraction } from './interaction-display.ts';
/** Focus follows the server's publication order; UUID identity never orders work. */
import type { InteractionRef } from "../protocol/app-server.ts";
export function interactionRefLabel(interaction: InteractionRef): string {
  return `${interaction.conversation_id}::${interaction.interaction_id}`;
}
export function sameInteractionRef(left: InteractionRef, right: InteractionRef): boolean {
  return left.conversation_id === right.conversation_id && left.interaction_id === right.interaction_id;
}
export function findPendingInteraction(pending: readonly DisplayInteraction[], interaction: InteractionRef): DisplayInteraction | undefined {
  return pending.find(entry => sameInteractionRef(entry.interaction, interaction));
}
/** Preserve an existing focus; otherwise choose the first published pending item. */
export function reconcileInteractionFocus(pending: readonly DisplayInteraction[], current: InteractionRef | undefined): InteractionRef | undefined {
  return current !== undefined && findPendingInteraction(pending, current) !== undefined ? current : pending[0]?.interaction;
}
/** Pure navigation through native publication order, wrapping at both ends. */
export function moveInteractionFocus(pending: readonly DisplayInteraction[], current: InteractionRef | undefined, delta: -1 | 1): InteractionRef | undefined {
  const focused = reconcileInteractionFocus(pending, current);
  if (focused === undefined) return undefined;
  const index = pending.findIndex(entry => sameInteractionRef(entry.interaction, focused));
  return pending[(index + delta + pending.length) % pending.length]!.interaction;
}
