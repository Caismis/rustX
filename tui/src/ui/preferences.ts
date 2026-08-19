/**
 * Client presentation preferences.
 *
 * These are the *only* pieces of visible state rustX does not own, and they
 * are deliberately kept out of `PresentationState`: nothing here describes a
 * runtime fact, so nothing here may be written into the projection or
 * reconstructed from a snapshot. A fresh snapshot rebuilds every semantic
 * fact; these preferences survive because the client kept them, not because
 * the runtime republished them, and losing them changes nothing but the view.
 *
 * The distinction that matters most:
 *
 * ```text
 * reasoningVisible     a client presentation setting  (do I want to read it?)
 * reasoningProfile     a model request configuration  (what did we ask for?)
 * reasoningEnabled     a model request configuration  (was it requested?)
 * ```
 *
 * Hiding reasoning here never changes what rustX asked the provider for, and
 * turning reasoning off in the model configuration is not something this file
 * can express.
 *
 * Likewise, expanding a tool card is a *visual* act: it re-renders facts the
 * client already holds. It never re-executes a tool, re-reads a file, or
 * fetches anything, and it is unrelated to the runtime's own result
 * truncation, which is reported separately and always.
 */

import type { ToolCallId } from "../protocol/types.ts";

/** How many output lines a collapsed tool card shows. */
export const DEFAULT_PREVIEW_LINES = 8;

export interface PresentationPreferences {
  /** Whether model reasoning is rendered, or replaced by a compact marker. */
  reasoningVisible: boolean;
  /**
   * Cards the user expanded, by the stable runtime identity they render.
   *
   * A `ToolCallId` for a tool card, a `ToolExecutionId` for a background one.
   * Both are runtime-allocated and unique, which is the only property this
   * set needs.
   */
  expandedCalls: ReadonlySet<ToolCallId>;
  /** How many output lines a collapsed card shows. */
  previewLines: number;
}

export function defaultPreferences(): PresentationPreferences {
  return {
    reasoningVisible: true,
    expandedCalls: new Set(),
    previewLines: DEFAULT_PREVIEW_LINES,
  };
}

export function withReasoningVisible(
  preferences: PresentationPreferences,
  visible: boolean,
): PresentationPreferences {
  return { ...preferences, reasoningVisible: visible };
}

/** Toggles one card's expansion. Purely visual, never a runtime request. */
export function withToggledCall(
  preferences: PresentationPreferences,
  callId: ToolCallId,
): PresentationPreferences {
  const expanded = new Set(preferences.expandedCalls);
  if (!expanded.delete(callId)) {
    expanded.add(callId);
  }
  return { ...preferences, expandedCalls: expanded };
}

export function withAllCollapsed(
  preferences: PresentationPreferences,
): PresentationPreferences {
  return { ...preferences, expandedCalls: new Set() };
}

export function withExpandedCalls(
  preferences: PresentationPreferences,
  callIds: Iterable<ToolCallId>,
): PresentationPreferences {
  return { ...preferences, expandedCalls: new Set(callIds) };
}

export function isExpanded(
  preferences: PresentationPreferences,
  callId: ToolCallId,
): boolean {
  return preferences.expandedCalls.has(callId);
}
