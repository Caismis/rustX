/**
 * View models derived from presentation state.
 *
 * Components read these instead of subscribing to protocol events directly.
 * That keeps the rendering layer replaceable: a future non-Pi client consumes
 * the same selectors, and swapping the terminal library touches nothing
 * below this file.
 *
 * Selectors are pure derivations. They label and group; they never decide a
 * semantic fact. A tool's origin or name may pick a label, a grouping, or a
 * presentation renderer; it may never pick an execution semantic, because
 * those are Rust-owned.
 *
 * Model facts live in three separate domains and the selectors keep them
 * separate:
 *
 * ```text
 * catalog     what a model offers        CatalogModelView
 * configured  what the session asked for SessionModelView.configured
 * effective   what the runtime will use  SessionModelView.effective
 * ```
 *
 * The working indicator and the footer live in `ui/components/status.ts`
 * because they are presentation compositions rather than derivations, and the
 * model selector reads the catalog directly.
 */

import type {
  BackgroundLifecycle,
  CapabilitySourceView,
  InteractionRequest,
  ModelInvocationView,
  SessionModelConfig,
  RuntimeClientBackgroundExecution,
  RuntimeClientOutcome,
  RuntimeClientSkill,
  RuntimeClientTool,
  SessionSummaryView,
  SessionView,
  ToolOrigin,
} from "../protocol/types.ts";
import { BACKGROUND_TERMINAL_STATES } from "../protocol/types.ts";
import type { PresentationState } from "./state.ts";

/**
 * The line that identifies one Session in the `/resume` selector.
 *
 * A Session is unnamed until a user names it, so an unnamed row shows what
 * the Session opened with instead. Both halves are Rust's: the name it
 * published and the first message it derived. This only chooses between
 * them, and says so plainly when a Session has neither.
 */
export function sessionRowLabel(session: SessionSummaryView): string {
  return session.name ?? session.preview ?? "(no messages)";
}

/**
 * The Session label used away from the selector, where no first message has
 * been derived. An unnamed Session is shown by its identity, which is the
 * only other thing that says which Session it is.
 */
export function sessionLabel(session: SessionView): string {
  return session.name ?? session.id;
}

/** A short human label for a tool origin. Cosmetic only. */
export function originLabel(origin: ToolOrigin): string {
  if (origin === "builtin") {
    return "native";
  }
  return `mcp:${origin.mcp.server_id}`;
}

/** A short human label for an attempt settlement. */
export function outcomeLabel(outcome: RuntimeClientOutcome): string {
  switch (outcome.type) {
    case "completed":
      return outcome.finish_reason.type === "other"
        ? `completed (${outcome.finish_reason.reason})`
        : `completed (${outcome.finish_reason.type})`;
    case "cancelled":
      return `cancelled (${outcome.reason})`;
    case "timed_out":
      return "timed out";
    case "limit_exceeded":
      return `limit exceeded (${outcome.limit})`;
    case "failed":
      return outcome.error.type === "model"
        ? `failed (${outcome.error.kind}): ${outcome.error.message}`
        : `failed: ${outcome.error.error.type}`;
    default:
      return "settled";
  }
}

/** Background executions the runtime still considers active. */
export function activeBackground(
  state: PresentationState,
): RuntimeClientBackgroundExecution[] {
  return state.background.filter(
    (execution) => !BACKGROUND_TERMINAL_STATES.has(execution.state),
  );
}

/**
 * The deterministic questionnaire that the app should focus when several
 * runtime interactions are pending.
 *
 * Runtime publication order is not a user-facing focus contract, so the TUI
 * deliberately selects the lexicographically smallest live InteractionId.
 * Approval remains command-driven; questionnaire responses are whole typed
 * submissions from the questionnaire overlay.
 */
export function focusedQuestionnaire(
  state: PresentationState | undefined,
): InteractionRequest | undefined {
  return state?.pendingInteractions
    .filter((interaction) => interaction.kind.type === "questionnaire")
    .slice()
    .sort((left, right) => {
      if (left.id < right.id) return -1;
      if (left.id > right.id) return 1;
      return 0;
    })[0];
}

/** The focused pending interaction for generic status/card rendering. */
export function focusedInteraction(
  state: PresentationState | undefined,
): InteractionRequest | undefined {
  return state?.pendingInteractions
    .slice()
    .sort((left, right) => left.id.localeCompare(right.id))[0];
}

/** Whether a background lifecycle state is terminal, per the runtime. */
export function isBackgroundTerminal(state: BackgroundLifecycle): boolean {
  return BACKGROUND_TERMINAL_STATES.has(state);
}

/** The tool catalog, grouped by origin for display. */
export function toolsByOrigin(
  state: PresentationState,
): Array<{ origin: string; tools: RuntimeClientTool[] }> {
  const groups = new Map<string, RuntimeClientTool[]>();
  for (const tool of state.capabilities.tools ?? []) {
    const label = originLabel(tool.origin);
    const bucket = groups.get(label);
    if (bucket === undefined) {
      groups.set(label, [tool]);
    } else {
      bucket.push(tool);
    }
  }
  return [...groups.entries()]
    .map(([origin, tools]) => ({ origin, tools }))
    .sort((left, right) => left.origin.localeCompare(right.origin));
}

/** Available Tools that are not in the runtime-published active registry. */
export function inactiveToolsByOrigin(
  state: PresentationState,
): Array<{ origin: string; tools: RuntimeClientTool[] }> {
  const activeIds = new Set((state.capabilities.tools ?? []).map((tool) => tool.id));
  const groups = new Map<string, RuntimeClientTool[]>();
  for (const tool of state.capabilities.available_tools ?? []) {
    if (activeIds.has(tool.id)) {
      continue;
    }
    const label = originLabel(tool.origin);
    const bucket = groups.get(label);
    if (bucket === undefined) {
      groups.set(label, [tool]);
    } else {
      bucket.push(tool);
    }
  }
  return [...groups.entries()]
    .map(([origin, tools]) => ({ origin, tools }))
    .sort((left, right) => left.origin.localeCompare(right.origin));
}

/** The Skill catalog as the runtime published it. */
export function skills(state: PresentationState): RuntimeClientSkill[] {
  return state.capabilities.skills ?? [];
}

/** The optional capability sources the runtime reports as unavailable
 * (Issue #81). The typed reason travels with each source; the footer only
 * renders the count. */
export function unavailableCapabilities(
  state: PresentationState,
): CapabilitySourceView[] {
  return (state.capabilities.sources ?? []).filter(
    (source) => source.state.type === "unavailable",
  );
}

/** A compact capability summary line for one resolved invocation. */
export function capabilitySummary(invocation: ModelInvocationView): string {
  const parts = [
    `in: ${invocation.capabilities.inputModalities.join("/") || "none"}`,
    `out: ${invocation.capabilities.outputModalities.join("/") || "none"}`,
    `tools: ${invocation.capabilities.toolCalls ? "yes" : "no"}`,
    `reasoning: ${describeReasoning(invocation)}`,
  ];
  return parts.join("  ");
}

/**
 * How reasoning stands for one invocation.
 *
 * Reports exactly what rustX published. A reasoning-capable model with no
 * selectable profile is *not* rendered as a profile list: there is no
 * universal off/low/medium/high, and inventing one would make this client a
 * second model-configuration authority.
 */
export function describeReasoning(invocation: ModelInvocationView): string {
  if (!invocation.capabilities.reasoning) {
    return "unsupported";
  }
  if (invocation.reasoningProfile !== undefined) {
    return `${invocation.reasoningEnabled ? "on" : "off"} (profile ${invocation.reasoningProfile})`;
  }
  return invocation.reasoningEnabled
    ? "on (runtime default, no selectable profile)"
    : "off (runtime default, no selectable profile)";
}

/**
 * The reasoning profile the session *asked for*, as configured.
 *
 * Deliberately separate from {@link describeReasoning}, which reports what is
 * effective, and from a catalog entry's `defaultReasoningProfile`, which is
 * what the catalog would fall back to. Those are three different facts and
 * the UI must never present one as another: a catalog default is not evidence
 * that the session configured anything.
 */
export function describeConfiguredReasoning(
  configured: SessionModelConfig,
): string {
  return configured.reasoningProfile === undefined
    ? "not configured (the runtime decides)"
    : `profile ${configured.reasoningProfile}`;
}

/**
 * Modalities the catalog claims but the runtime cannot deliver today.
 *
 * Displaying only effective capability is the rule; this exists so a user can
 * be told *why* something is absent without the client ever advertising an
 * unusable capability as available.
 */
export function unavailableInputModalities(
  invocation: ModelInvocationView,
): string[] {
  const effective = new Set(invocation.capabilities.inputModalities);
  return invocation.declaredCapabilities.inputModalities.filter(
    (modality) => !effective.has(modality),
  );
}
