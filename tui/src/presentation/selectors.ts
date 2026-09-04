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
  AgentStatusView,
  BackgroundLifecycle,
  CapabilitySourceView,
  RoutedInteraction,
  ModelInvocationView,
  SessionModelConfig,
  RuntimeClientBackgroundExecution,
  RuntimeClientOutcome,
  RuntimeClientSkill,
  RuntimeClientSubagent,
  RuntimeClientTool,
  MessageId,
  RuntimeClientTranscriptCursor,
  SessionSummaryView,
  SessionView,
  ToolOrigin,
} from "../protocol/types.ts";
import {
  BACKGROUND_TERMINAL_STATES,
  SUBAGENT_TERMINAL_STATES,
} from "../protocol/types.ts";
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

/** Subagent children the runtime still considers active. */
export function activeSubagents(
  state: PresentationState,
): RuntimeClientSubagent[] {
  return state.subagents.filter(
    (subagent) => !SUBAGENT_TERMINAL_STATES.has(subagent.state),
  );
}

/**
 * The deterministic questionnaire that the app should focus when several
 * runtime interactions are pending.
 *
 * Runtime publication order is not a user-facing focus contract, so the TUI
 * deliberately selects the lexicographically smallest routed identity pair.
 * Approval remains command-driven; questionnaire responses are whole typed
 * submissions from the questionnaire overlay.
 */
export function focusedQuestionnaire(
  state: PresentationState | undefined,
): RoutedInteraction | undefined {
  return state?.pendingInteractions
    .filter((interaction) => interaction.request.kind.type === "questionnaire")
    .slice()
    .sort(compareRoutedInteractions)[0];
}

/** The focused pending interaction for generic status/card rendering. */
export function focusedInteraction(
  state: PresentationState | undefined,
): RoutedInteraction | undefined {
  return state?.pendingInteractions
    .slice()
    .sort(compareRoutedInteractions)[0];
}

function compareRoutedInteractions(
  left: RoutedInteraction,
  right: RoutedInteraction,
): number {
  return left.interaction.conversation_id.localeCompare(
    right.interaction.conversation_id,
  ) || left.interaction.interaction_id.localeCompare(
    right.interaction.interaction_id,
  );
}

// ---------------------------------------------------------------------------
// Agent Status placement
//
// Agent Status is runtime-owned model-visible context. Its canonical Context
// message stays out of the ordinary transcript, and the annotation the
// transcript draws instead is placed from runtime facts alone:
//
// ```text
// FreshInbound present   the referenced inbound message
//                          opportunities.fresh_inbound.target_message_id
// otherwise              the settled tool batch it followed
//                          opportunities.post_tool_batch.transcript_anchor
// ```
//
// Both identities are frozen by the runtime at the point the opportunity was
// established, so neither can be moved afterwards by unrelated durable
// activity. Nothing here reconstructs a position.
//
// Exactly one of those applies to any one composition, which is what makes
// "every composed Agent Status has exactly one presentation anchor" a
// property of the data rather than a rule a renderer has to remember. Nothing
// below reads a timestamp, an array position, or a neighbouring row.
// ---------------------------------------------------------------------------

/** Where one composed Agent Status belongs, per the runtime. */
export type AgentStatusAnchor =
  | {
      /** Subordinate metadata of the inbound turn that made it eligible. */
      kind: "inbound_message";
      messageId: MessageId;
    }
  | {
      /** A standalone update after the settled tool batch it followed. */
      kind: "transcript_position";
      cursor: RuntimeClientTranscriptCursor;
    }
  | {
      /** The runtime published no placement fact; nothing is drawn. */
      kind: "unplaced";
    };

/**
 * The one anchor of one composed status.
 *
 * `FreshInbound` wins when both opportunities are present: it names an exact
 * message identity, which is a stronger fact than a position, and choosing it
 * unconditionally is what keeps a doubly-eligible composition from being
 * drawn twice.
 */
export function agentStatusAnchor(status: AgentStatusView): AgentStatusAnchor {
  const fresh = status.opportunities.fresh_inbound;
  if (fresh !== undefined) {
    return { kind: "inbound_message", messageId: fresh.target_message_id };
  }
  const batch = status.opportunities.post_tool_batch;
  if (batch?.transcript_anchor !== undefined) {
    return { kind: "transcript_position", cursor: batch.transcript_anchor };
  }
  return { kind: "unplaced" };
}

/** Composed statuses grouped by the transcript entry they are drawn after. */
export interface AgentStatusPlacement {
  /** Keyed by the referenced inbound message identity. */
  byMessageId: Map<MessageId, AgentStatusView[]>;
  /** Keyed by the durable transcript position the composition followed. */
  byCursor: Map<RuntimeClientTranscriptCursor, AgentStatusView[]>;
}

/**
 * Groups every composed status by its single anchor, in composition order.
 *
 * A status appears in exactly one bucket, so a renderer that visits each
 * bucket once draws each composition once. A status whose anchor names a
 * transcript entry that is not loaded is simply not drawn; paging that entry
 * in later places it, because placement is a property of the fact and not of
 * what happened to be on screen.
 */
export function agentStatusPlacement(
  state: PresentationState,
): AgentStatusPlacement {
  const byMessageId = new Map<MessageId, AgentStatusView[]>();
  const byCursor = new Map<RuntimeClientTranscriptCursor, AgentStatusView[]>();
  for (const status of state.statuses) {
    const anchor = agentStatusAnchor(status);
    if (anchor.kind === "inbound_message") {
      append(byMessageId, anchor.messageId, status);
    } else if (anchor.kind === "transcript_position") {
      append(byCursor, anchor.cursor, status);
    }
  }
  return { byMessageId, byCursor };
}

function append<K>(
  index: Map<K, AgentStatusView[]>,
  key: K,
  status: AgentStatusView,
): void {
  const existing = index.get(key);
  if (existing === undefined) {
    index.set(key, [status]);
    return;
  }
  existing.push(status);
}

/**
 * The newest composed Agent Status, or `undefined` before the first one.
 *
 * Derived from the runtime's own composition order rather than kept as a
 * second field, so it can never disagree with the annotations the transcript
 * draws.
 */
export function latestAgentStatus(
  state: PresentationState,
): AgentStatusView | undefined {
  return state.statuses[state.statuses.length - 1];
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
