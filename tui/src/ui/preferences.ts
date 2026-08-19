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
 * ## Disclosure is bounded in two dimensions
 *
 * A collapsed band is finite in *height* and in *content length*, and the
 * budgets below are the single policy that says so. One dimension is not a
 * bound: three pretty-printed JSON lines can carry 100 kB, and a 50 kB path
 * is one line. {@link PreviewBudget} carries both, the card shell applies it
 * to every externally-derived band, and no renderer is asked to remember.
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

/**
 * The finite budget of one collapsed presentation band.
 *
 * Two dimensions, because a band is unbounded if either one is. A line budget
 * alone leaves `{"payload": "<100 kB on one line>"}` — three pretty-printed
 * lines — free to fill the terminal, and a character budget alone leaves ten
 * thousand short lines free to do the same. Every externally-derived text
 * band in a collapsed card is bounded by both.
 */
export interface PreviewBudget {
  /** How many lines of the band a collapsed card shows. */
  maxLines: number;
  /** How many visible characters of the band a collapsed card shows. */
  maxChars: number;
}

/** How many detail lines a collapsed card shows, per detail section. */
export const DEFAULT_PREVIEW_LINES = 8;

/**
 * How many visible characters a collapsed detail section shows.
 *
 * Sized so ordinary output — the eight lines the height budget already
 * allows, at terminal width — is never clipped, while a single 50 kB line
 * is. The bound is on *content*, so it does not matter whether the bulk of a
 * band arrives as height or as width.
 */
export const DEFAULT_PREVIEW_CHARS = 1_000;

/**
 * The always-visible identity line of a call: a path, a command, a pattern.
 *
 * One line by contract, and now finite by contract too: a published path or
 * pattern is externally derived and may be arbitrarily long.
 */
export const SUBJECT_BUDGET: PreviewBudget = { maxLines: 1, maxChars: 160 };

/** The card title and header chrome. Externally derived, so also finite. */
export const HEADER_BUDGET: PreviewBudget = { maxLines: 1, maxChars: 80 };

/**
 * The always-visible runtime-published summary band.
 *
 * `summary` is a documented *structural* contract — `42 matches`, `wrote 120
 * bytes` — but a contract a renderer could forget is not an invariant. The
 * shell bounds it too, so no present or future renderer can route arbitrary
 * tool prose through `summary` to escape progressive disclosure.
 */
export const SUMMARY_BUDGET: PreviewBudget = { maxLines: 4, maxChars: 240 };

export interface PresentationPreferences {
  /** Whether model reasoning is rendered, or replaced by a compact marker. */
  reasoningVisible: boolean;
  /** Expanded tool cards, keyed by the runtime's `ToolCallId`. */
  expandedToolCalls: ReadonlySet<ToolCallId>;
  /** Expanded background cards, keyed by the runtime's `ToolExecutionId`. */
  expandedBackgroundExecutions: ReadonlySet<ToolExecutionId>;
  /** The collapsed budget of one verbose detail section. */
  previewBudget: PreviewBudget;
}

export function defaultPreferences(): PresentationPreferences {
  return {
    reasoningVisible: true,
    expandedToolCalls: new Set(),
    expandedBackgroundExecutions: new Set(),
    previewBudget: {
      maxLines: DEFAULT_PREVIEW_LINES,
      maxChars: DEFAULT_PREVIEW_CHARS,
    },
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
