/**
 * The activity area: work that is not conversation content.
 *
 * ```text
 * transcript   what was said, and every tool call that was said
 * activity     background executions, subagent live activity, pending HITL
 *              interactions, orphaned executions
 * ```
 *
 * A foreground tool call is deliberately *not* here. It belongs to the
 * assistant message that requested it and renders there, as one card, which
 * is what stops the same call from appearing as transcript JSON, a running
 * card, and a result block at the same time.
 *
 * Background executions and interactions stay runtime-owned. Hiding a card
 * cancels nothing, an empty section is not evidence of settlement, and this
 * client never simulates either lifecycle locally.
 *
 * Foreground cards are keyed by `ToolCallId`, background cards by
 * `ToolExecutionId`, and interaction cards by `InteractionRef`. Those are three
 * runtime identity domains, so their expansion preferences are three sets —
 * never one string set all three would index.
 *
 * ## Every collapsed band here is reversible
 *
 * A background settlement and a pending approval both carry runtime-published
 * prose that can be arbitrarily long, so both are bounded by default. Both are
 * also *decision-relevant*: a reader deciding whether to allow a call, or
 * reading why one failed, must be able to see the whole published fact. So
 * every band this file collapses, it can restore — from `PresentationState`
 * alone, with no runtime request, no re-execution, and no read.
 *
 * One expansion state per entity covers every expandable band of that entity.
 * A background card whose body expanded but whose failure reason stayed
 * clipped would be a collapse the reader cannot undo, which is the one thing
 * client-side collapse may never be.
 */

import type {
  InteractionRef,
  RoutedInteraction,
  RuntimeClientBackgroundExecution,
  RuntimeClientSubagent,
  RuntimeClientSubagentActivity,
} from "../../protocol/types.ts";
import { SUBAGENT_TERMINAL_STATES } from "../../protocol/types.ts";
import type { PresentationState } from "../../presentation/state.ts";
import { interactionRefLabel } from "../../presentation/interaction-focus.ts";
import type { ToolCorrelation } from "../../presentation/tools.ts";
import {
  activeBackground,
  activeSubagents,
  isBackgroundTerminal,
  originLabel,
} from "../../presentation/selectors.ts";
import {
  type PresentationPreferences,
  HEADER_BUDGET,
  isBackgroundExecutionExpanded,
  isInteractionExpanded,
  interactionKey,
  isToolCallExpanded,
} from "../preferences.ts";
import { role, style } from "../theme.ts";
import { renderToolCard, describeProgress, statusLabel } from "./tool-card.ts";
import {
  type ToolRenderContext,
  clipText,
  formatJson,
  preview,
  toLines,
} from "./tool-renderers.ts";

/**
 * Executions the runtime still tracks whose transcript anchor is gone.
 *
 * This happens when an attempt settles without committing its assistant
 * message: the stream is dropped, but the executions it started are real and
 * must not vanish from the screen.
 */
export function renderOrphanExecutions(
  correlation: ToolCorrelation,
  preferences: PresentationPreferences,
): string {
  if (correlation.orphans.length === 0) {
    return "";
  }
  return [
    role.meta("Executions from an uncommitted turn"),
    ...correlation.orphans.map((tool) =>
      renderToolCard(tool, {
        expanded: isToolCallExpanded(preferences, tool.callId),
        budget: preferences.previewBudget,
      }),
    ),
  ].join("\n");
}

/** The background section, or an empty string when nothing is known. */
export function renderBackgroundSection(
  state: PresentationState,
  preferences: PresentationPreferences,
): string {
  if (state.background.length === 0) {
    return "";
  }
  const active = activeBackground(state).length;
  return [
    role.strong(
      `Background · ${active} active of ${state.background.length} known`,
    ),
    ...state.background.map((execution) =>
      renderBackground(execution, preferences),
    ),
  ].join("\n");
}

/** One background execution card, driven entirely by the runtime lifecycle. */
export function renderBackground(
  execution: RuntimeClientBackgroundExecution,
  preferences: PresentationPreferences,
): string {
  const terminal = isBackgroundTerminal(execution.state);
  const glyph = terminal ? role.meta("●") : role.pending("◐");
  const lines = [
    `${glyph} ${role.toolTitle(style.bold(clipText(execution.tool_name, HEADER_BUDGET.maxChars)))} ${role.chrome("·")} ${
      terminal ? role.meta(execution.state) : role.pending(execution.state)
    } ${role.meta(execution.execution_id)}`,
  ];
  const progress = describeProgress(execution.progress);
  if (progress !== undefined) {
    lines.push(`  ${role.meta(progress)}`);
  }
  if (execution.result !== undefined) {
    const result = execution.result;
    // One disclosure context for the whole execution. Keyed by
    // `ToolExecutionId`, in its own preference domain: a foreground
    // `ToolCallId` or an `InteractionId` that happens to serialize to the same
    // string is a different identity and never expands this card.
    const context: ToolRenderContext = {
      expanded: isBackgroundExecutionExpanded(
        preferences,
        execution.execution_id,
      ),
      budget: preferences.previewBudget,
    };
    lines.push(`  ${statusLabel(result)}`);
    // The status header names the settlement; the runtime's prose explaining
    // it goes in its own bounded band, exactly as on a foreground card — and
    // under the *same* expansion state as the body, so expanding this card
    // reveals every fact it holds rather than only half of them.
    const reason =
      result.status.type === "failed"
        ? toLines(result.status.error).map((line) => role.error(line))
        : result.status.type === "denied"
          ? toLines(result.status.reason).map((line) => role.warning(line))
          : result.status.type === "outcome_unknown"
            ? toLines(result.status.detail).map((line) => role.warning(line))
            : [];
    for (const line of preview(reason, context, "reason line")) {
      lines.push(`  ${line}`);
    }
    const body: string[] = [];
    for (const content of result.content ?? []) {
      if (content.type === "text") {
        body.push(...content.text.split("\n"));
      }
      if (content.type === "json") {
        body.push(...formatJson(content.value));
      }
    }
    for (const line of preview(body, context)) {
      lines.push(`  ${line}`);
    }
    // The runtime's own truncation is a different fact: expanding restores
    // what this client hid, never what the runtime never sent.
    if (result.truncation?.truncated === true) {
      const original = result.truncation.original_bytes;
      lines.push(
        `  ${role.meta(`⚠ runtime-truncated result${original === undefined ? "" : ` (from ${original} bytes)`}`)}`,
      );
    }
  }
  return lines.join("\n");
}

/**
 * The subagent section: one compact row per child known to the parent runtime.
 * Active rows show the disposable observation plane; terminal rows remain
 * visible as navigation targets for their durable child conversation (Issue
 * #179).
 *
 * This is the observation plane, not a second authority: the lifecycle
 * label and the activity line are both runtime-published facts. Terminal rows
 * render lifecycle and identity only: their settlement is conversation
 * content, delivered by the durable terminal inbound publication, and
 * `detail` — diagnostics only since Issue #178 — is never rendered as a
 * payload.
 *
 * `now` is injectable so the elapsed and last-activity labels are provable
 * without a wall clock.
 */
export function renderSubagentSection(
  state: PresentationState,
  preferences: PresentationPreferences,
  now: Date = new Date(),
  selectedSubagentId?: string,
): string {
  if (state.subagents.length === 0) {
    return "";
  }
  const active = activeSubagents(state);
  const retained = state.subagents.filter(
    (subagent) =>
      subagent.workspace.resource_state !== "none" &&
      subagent.workspace.resource_state !== "disposed",
  ).length;
  const actions = ["Ctrl+↑↓ select", "Enter inspect"];
  if (retained > 0) actions.push("D dispose retained");
  return [
    role.strong(
      `Subagents · ${active.length} active of ${state.subagents.length} known · ${actions.join(" · ")}`,
    ),
    ...state.subagents.map((subagent) => renderSubagent(
      subagent,
      preferences,
      now,
      subagent.subagent_id === selectedSubagentId,
    )),
  ].join("\n");
}

/** One compact row for a subagent identity and its optional live observation. */
function renderSubagent(
  subagent: RuntimeClientSubagent,
  _preferences: PresentationPreferences,
  now: Date,
  selected: boolean,
): string {
  const terminal = SUBAGENT_TERMINAL_STATES.has(subagent.state);
  const glyph = terminal ? role.meta("●") : role.pending("◐");
  const timing = terminal ? "" : ` ${role.chrome("·")} ${role.meta(formatElapsed(now, subagent.started_at))}`;
  const lines = [
    `${selected ? role.accent("▸") : " "} ${glyph} ${role.toolTitle(style.bold(bounded(subagent.agent)))} ${role.chrome("·")} ${terminal ? role.meta(subagent.state) : role.pending(subagent.state)}${timing} ${role.chrome("·")} ${role.meta(bounded(subagent.child_conversation_id))}`,
  ];
  if (terminal) {
    return lines.join("\n");
  }
  lines.push(`  ${role.meta(activityLine(subagent.observation.activity))}`);
  const lastActivityAt = subagent.observation.last_activity_at;
  if (lastActivityAt !== undefined) {
    lines.push(
      `  ${role.meta(`last activity ${formatElapsed(now, lastActivityAt)} ago`)}`,
    );
  }
  const profile = subagent.execution_profile;
  if (profile !== undefined) {
    const reasoning = profile.reasoning_profile;
    lines.push(
      `  ${role.meta(bounded(reasoning === undefined ? profile.model : `${profile.model} · ${reasoning}`))}`,
    );
  }
  return lines.join("\n");
}

/** The one activity line of a subagent card, from the latest projection. */
function activityLine(activity: RuntimeClientSubagentActivity): string {
  switch (activity.type) {
    case "awaiting_activity":
      return "awaiting activity";
    case "model":
      return activity.retry > 0
        ? `model request · retry ${activity.retry}`
        : "model request";
    case "retrying_model":
      return `retrying · attempt ${activity.retry}`;
    case "tool": {
      const progress = describeProgress(activity.progress);
      return progress === undefined
        ? bounded(activity.tool_id)
        : `${bounded(activity.tool_id)} · ${progress}`;
    }
    case "compacting":
      return "compacting context";
    case "waiting":
      return activity.on.type === "approval"
        ? `waiting · approval (${bounded(activity.on.tool_id)})`
        : "waiting · questionnaire";
  }
}

/** Bounds one externally derived value to a single finite line. */
function bounded(value: string): string {
  return clipText(value.replace(/\r?\n/g, " "), HEADER_BUDGET.maxChars);
}

/** A compact runtime-relative duration (`5s`, `3m`, `2h`, `1d`). */
function formatElapsed(now: Date, since: string): string {
  const ms = now.getTime() - Date.parse(since);
  if (!Number.isFinite(ms) || ms < 0) {
    return "0s";
  }
  const seconds = Math.floor(ms / 1_000);
  if (seconds < 60) {
    return `${seconds}s`;
  }
  const minutes = Math.floor(seconds / 60);
  if (minutes < 60) {
    return `${minutes}m`;
  }
  const hours = Math.floor(minutes / 60);
  if (hours < 24) {
    return `${hours}h`;
  }
  return `${Math.floor(hours / 24)}d`;
}

/**
 * The live runtime-owned pending interactions, rendered without local outcome
 * state.
 *
 * Every pending routed interaction — approvals and questionnaires, from the
 * primary conversation and from supervised subagents — is independently
 * represented here for as long as the runtime keeps it pending. The section
 * answers nothing: the unified human-input surface (opened automatically, or
 * with Ctrl+G after a dismissal) collects the typed responses, and this list
 * is the always-visible record that work is waiting.
 *
 * `focused` is the app's presentation-only focus, passed in so the marker
 * here always names the same interaction the surface is showing.
 */
export function renderInteractionSection(
  state: PresentationState,
  preferences: PresentationPreferences,
  focused?: InteractionRef,
): string {
  if (state.pendingInteractions.length === 0) {
    return "";
  }
  const focusedKey =
    focused === undefined ? undefined : interactionKey(focused);
  return [
    role.pending(`Human input required · ${state.pendingInteractions.length} pending`),
    ...(focused === undefined
      ? []
      : [role.accent(`Focused interaction: ${interactionRefLabel(focused)}`)]),
    ...state.pendingInteractions.map((interaction) =>
      renderInteraction(
        interaction,
        preferences,
        focusedKey !== undefined &&
          interactionKey(interaction.interaction) === focusedKey,
      ),
    ),
    role.meta(
      "Ctrl+G opens the human-input surface · /expand interaction <conversation-id>::<interaction-id> reveals full detail",
    ),
  ].join("\n");
}

/**
 * One pending approval, with its decision facts finite but never lost.
 *
 * The reason is runtime prose and the arguments are model output, so both are
 * externally derived and both are bounded by default — an approval prompt
 * that scrolls its own question off the screen is one nobody can answer. But
 * they are also the facts the decision is *made from*, so the bound must be
 * one the reader can lift: `/expand interaction <conversation-id>::<interaction-id>` reveals the complete
 * reason and the complete validated arguments, rendered from the interaction
 * the client already holds.
 *
 * Expanding is not a second approval gate. Nothing here requires the reader to
 * open the card before answering, and nothing here can edit what they answer
 * about: the arguments are drawn exactly as the runtime validated them, and
 * this client never sends back a modified call.
 */
function renderInteraction(
  routed: RoutedInteraction,
  preferences: PresentationPreferences,
  focused: boolean,
): string {
  const interaction = routed.request;
  const identity = formatInteraction(routed);
  const source = sourceLabel(routed);
  if (interaction.kind.type === "questionnaire") {
    const questions = interaction.kind.questionnaire.questions.flatMap((question, index) => [
      `${index + 1}. ${question.header}: ${clipText(question.question, HEADER_BUDGET.maxChars)}`,
      ...question.options.map((option) => `   • ${option.label}`),
    ]);
    return [
      `${focused ? role.accent("→") : role.pending("?")} ${role.toolTitle(style.bold("Questionnaire"))} ${role.meta(identity)}`,
      `  ${role.meta(source)}`,
      ...questions.map((question) => `  ${role.meta(question)}`),
      `  ${role.meta("custom answer is always available in the questionnaire")}`,
    ].join("\n");
  }
  const kind = interaction.kind;
  // Keyed by `InteractionRef`, its own preference domain. A `ToolCallId` or a
  // `ToolExecutionId` that serializes to the same string is a different
  // identity and never expands this card — note that `kind.call_id` is a
  // genuinely different identity of the same request, and is displayed, not
  // used as the expansion key.
  const context: ToolRenderContext = {
    expanded: isInteractionExpanded(preferences, routed.interaction),
    budget: preferences.previewBudget,
  };
  return [
    `${focused ? role.accent("→") : role.pending("?")} ${role.toolTitle(style.bold(clipText(kind.tool_name, HEADER_BUDGET.maxChars)))} ${role.meta(identity)}`,
    `  ${role.meta(source)}`,
    `  ${role.meta(`${kind.mode} · ${originLabel(kind.origin)} · call ${kind.call_id}`)}`,
    ...preview(toLines(kind.reason), context, "reason line").map(
      (line) => `  ${line}`,
    ),
    ...preview(formatJson(kind.arguments), context, "argument line").map(
      (line) => `  ${role.meta(line)}`,
    ),
  ].join("\n");
}

function formatInteraction(interaction: RoutedInteraction): string {
  return `${interaction.interaction.conversation_id}::${interaction.interaction.interaction_id}`;
}

function sourceLabel(interaction: RoutedInteraction): string {
  if (interaction.source.type === "primary") {
    return "source · primary conversation";
  }
  return `source · subagent ${bounded(interaction.source.agent_name)} · ${bounded(interaction.source.child_conversation_id)}`;
}
