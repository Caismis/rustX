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
 * Likewise, expanding a card is a *visual* act: it re-renders facts the client
 * already holds. It never re-executes a tool, re-reads a file, or fetches
 * anything, and it is unrelated to the runtime's own result truncation, which
 * is reported separately and always.
 *
 * ## Two identity domains, never one
 *
 * ```text
 * ToolCallId       a logical model-issued tool call
 * ToolExecutionId  a detached background execution instance
 * ```
 *
 * rustX models those as separate identity domains and this file preserves the
 * separation. They both happen to serialize as strings, and nothing forbids
 * the same string appearing in both, so a single set keyed by a naked string
 * would let one card's expansion silently toggle an unrelated one. Two sets
 * make that unrepresentable — no naming convention (`call_*`, `exec_*`) is
 * relied on anywhere, because a wire spelling is not a type.
 */

import type { ToolCallId, ToolExecutionId } from "../protocol/types.ts";

/** How many detail lines a collapsed card shows, per detail section. */
export const DEFAULT_PREVIEW_LINES = 8;

/**
 * How many characters of an unparsed argument fragment a collapsed card shows.
 *
 * A partially streamed argument fragment is frequently one unbroken line, so a
 * line budget alone does not bound it. This is the character budget that does.
 */
export const RAW_FRAGMENT_PREVIEW_CHARS = 400;

export interface PresentationPreferences {
  /** Whether model reasoning is rendered, or replaced by a compact marker. */
  reasoningVisible: boolean;
  /** Expanded tool cards, keyed by the runtime's `ToolCallId`. */
  expandedToolCalls: ReadonlySet<ToolCallId>;
  /** Expanded background cards, keyed by the runtime's `ToolExecutionId`. */
  expandedBackgroundExecutions: ReadonlySet<ToolExecutionId>;
  /** How many detail lines a collapsed card shows, per detail section. */
  previewLines: number;
}

export function defaultPreferences(): PresentationPreferences {
  return {
    reasoningVisible: true,
    expandedToolCalls: new Set(),
    expandedBackgroundExecutions: new Set(),
    previewLines: DEFAULT_PREVIEW_LINES,
  };
}

export function withReasoningVisible(
  preferences: PresentationPreferences,
  visible: boolean,
): PresentationPreferences {
  return { ...preferences, reasoningVisible: visible };
}

// ---------------------------------------------------------------------------
// Tool-call domain
// ---------------------------------------------------------------------------

/** Toggles one tool card's expansion. Purely visual, never a runtime request. */
export function withToggledToolCall(
  preferences: PresentationPreferences,
  callId: ToolCallId,
): PresentationPreferences {
  return {
    ...preferences,
    expandedToolCalls: toggled(preferences.expandedToolCalls, callId),
  };
}

export function withExpandedToolCalls(
  preferences: PresentationPreferences,
  callIds: Iterable<ToolCallId>,
): PresentationPreferences {
  return { ...preferences, expandedToolCalls: new Set(callIds) };
}

export function isToolCallExpanded(
  preferences: PresentationPreferences,
  callId: ToolCallId,
): boolean {
  return preferences.expandedToolCalls.has(callId);
}

// ---------------------------------------------------------------------------
// Background-execution domain
// ---------------------------------------------------------------------------

/** Toggles one background card's expansion. */
export function withToggledBackgroundExecution(
  preferences: PresentationPreferences,
  executionId: ToolExecutionId,
): PresentationPreferences {
  return {
    ...preferences,
    expandedBackgroundExecutions: toggled(
      preferences.expandedBackgroundExecutions,
      executionId,
    ),
  };
}

export function withExpandedBackgroundExecutions(
  preferences: PresentationPreferences,
  executionIds: Iterable<ToolExecutionId>,
): PresentationPreferences {
  return { ...preferences, expandedBackgroundExecutions: new Set(executionIds) };
}

export function isBackgroundExecutionExpanded(
  preferences: PresentationPreferences,
  executionId: ToolExecutionId,
): boolean {
  return preferences.expandedBackgroundExecutions.has(executionId);
}

// ---------------------------------------------------------------------------
// Both domains
// ---------------------------------------------------------------------------

/** Collapses everything, in both identity domains. */
export function withAllCollapsed(
  preferences: PresentationPreferences,
): PresentationPreferences {
  return {
    ...preferences,
    expandedToolCalls: new Set(),
    expandedBackgroundExecutions: new Set(),
  };
}

function toggled<T>(current: ReadonlySet<T>, value: T): ReadonlySet<T> {
  const next = new Set(current);
  if (!next.delete(value)) {
    next.add(value);
  }
  return next;
}
