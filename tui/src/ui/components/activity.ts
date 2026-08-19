/**
 * The activity area: work that is not conversation content.
 *
 * ```text
 * transcript   what was said, and every tool call that was said
 * activity     background executions, pending approvals, orphaned executions
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
 * `ToolExecutionId`, and interaction cards by `InteractionId`. Those are three
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
  InteractionRequest,
  RuntimeClientBackgroundExecution,
} from "../../protocol/types.ts";
import type { PresentationState } from "../../presentation/state.ts";
import type { ToolCorrelation } from "../../presentation/tools.ts";
import {
  activeBackground,
  isBackgroundTerminal,
  originLabel,
} from "../../presentation/selectors.ts";
import {
  type PresentationPreferences,
  HEADER_BUDGET,
  isBackgroundExecutionExpanded,
  isInteractionExpanded,
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

/** The live runtime-owned approval cards, rendered without local outcome state. */
export function renderInteractionSection(
  state: PresentationState,
  preferences: PresentationPreferences,
): string {
  if (state.pendingInteractions.length === 0) {
    return "";
  }
  return [
    role.pending(
      `Approval required · ${state.pendingInteractions.length} pending`,
    ),
    ...state.pendingInteractions.map((interaction) =>
      renderInteraction(interaction, preferences),
    ),
    role.meta("/approve <interaction-id> <allow|deny> [reason]"),
    role.meta("/expand interaction <interaction-id> to see the full request"),
  ].join("\n");
}

/**
 * One pending approval, with its decision facts finite but never lost.
 *
 * The reason is runtime prose and the arguments are model output, so both are
 * externally derived and both are bounded by default — an approval prompt
 * that scrolls its own question off the screen is one nobody can answer. But
 * they are also the facts the decision is *made from*, so the bound must be
 * one the reader can lift: `/expand interaction <id>` reveals the complete
 * reason and the complete validated arguments, rendered from the interaction
 * the client already holds.
 *
 * Expanding is not a second approval gate. Nothing here requires the reader to
 * open the card before answering, and nothing here can edit what they answer
 * about: the arguments are drawn exactly as the runtime validated them, and
 * this client never sends back a modified call.
 */
function renderInteraction(
  interaction: InteractionRequest,
  preferences: PresentationPreferences,
): string {
  if (interaction.kind.type !== "approval") {
    return `${role.pending("?")} interaction ${interaction.id}`;
  }
  const kind = interaction.kind;
  // Keyed by `InteractionId`, its own preference domain. A `ToolCallId` or a
  // `ToolExecutionId` that serializes to the same string is a different
  // identity and never expands this card — note that `kind.call_id` is a
  // genuinely different identity of the same request, and is displayed, not
  // used as the expansion key.
  const context: ToolRenderContext = {
    expanded: isInteractionExpanded(preferences, interaction.id),
    budget: preferences.previewBudget,
  };
  return [
    `${role.pending("?")} ${role.toolTitle(style.bold(clipText(kind.tool_name, HEADER_BUDGET.maxChars)))} ${role.meta(interaction.id)}`,
    `  ${role.meta(`${kind.mode} · ${originLabel(kind.origin)} · call ${kind.call_id}`)}`,
    ...preview(toLines(kind.reason), context, "reason line").map(
      (line) => `  ${line}`,
    ),
    ...preview(formatJson(kind.arguments), context, "argument line").map(
      (line) => `  ${role.meta(line)}`,
    ),
  ].join("\n");
}
