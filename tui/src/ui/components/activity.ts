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
 * Background cards are keyed by `ToolExecutionId` and foreground cards by
 * `ToolCallId`. Those are two runtime identity domains, so their expansion
 * preferences are two sets — never one string set both would index.
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
  DEFAULT_PREVIEW_CHARS,
  DEFAULT_PREVIEW_LINES,
  HEADER_BUDGET,
  isBackgroundExecutionExpanded,
  isToolCallExpanded,
} from "../preferences.ts";
import { role, style } from "../theme.ts";
import { renderToolCard, describeProgress, statusLabel } from "./tool-card.ts";
import {
  type ToolRenderContext,
  bounded,
  clipText,
  formatJson,
  preview,
  toLines,
} from "./tool-renderers.ts";

/**
 * The disclosure context of a card with no expansion of its own.
 *
 * Interaction cards are live runtime prompts, not entities with a collapse
 * preference, so their bands are always drawn collapsed. They are still
 * externally-derived text, so they are still bounded — the same invariant the
 * tool card enforces, applied at the only other place arbitrary runtime and
 * provider text reaches the screen.
 */
const COLLAPSED: ToolRenderContext = {
  expanded: false,
  budget: { maxLines: DEFAULT_PREVIEW_LINES, maxChars: DEFAULT_PREVIEW_CHARS },
};

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
    lines.push(`  ${statusLabel(result)}`);
    // The status header names the settlement; the runtime's prose explaining
    // it goes in its own bounded band, exactly as on a foreground card.
    const reason =
      result.status.type === "failed"
        ? toLines(result.status.error).map((line) => role.error(line))
        : result.status.type === "denied"
          ? toLines(result.status.reason).map((line) => role.warning(line))
          : [];
    for (const line of bounded(reason, COLLAPSED.budget, COLLAPSED, "reason line")) {
      lines.push(`  ${line}`);
    }
    const body: string[] = [];
    for (const content of execution.result.content ?? []) {
      if (content.type === "text") {
        body.push(...content.text.split("\n"));
      }
      if (content.type === "json") {
        body.push(...formatJson(content.value));
      }
    }
    // Keyed by `ToolExecutionId`, in its own preference domain: a foreground
    // `ToolCallId` that happens to serialize to the same string is a different
    // identity and never expands this card.
    for (const line of preview(body, {
      expanded: isBackgroundExecutionExpanded(preferences, execution.execution_id),
      budget: preferences.previewBudget,
    })) {
      lines.push(`  ${line}`);
    }
  }
  return lines.join("\n");
}

/** The live runtime-owned approval cards, rendered without local outcome state. */
export function renderInteractionSection(state: PresentationState): string {
  if (state.pendingInteractions.length === 0) {
    return "";
  }
  return [
    role.pending(
      `Approval required · ${state.pendingInteractions.length} pending`,
    ),
    ...state.pendingInteractions.map(renderInteraction),
    role.meta("/approve <interaction-id> <allow|deny> [reason]"),
  ].join("\n");
}

function renderInteraction(interaction: InteractionRequest): string {
  if (interaction.kind.type !== "approval") {
    return `${role.pending("?")} interaction ${interaction.id}`;
  }
  const kind = interaction.kind;
  return [
    `${role.pending("?")} ${role.toolTitle(style.bold(clipText(kind.tool_name, HEADER_BUDGET.maxChars)))} ${role.meta(interaction.id)}`,
    `  ${role.meta(`${kind.mode} · ${originLabel(kind.origin)} · call ${kind.call_id}`)}`,
    // The approval reason is runtime prose and the arguments are model
    // output: both are externally derived, so both are bounded here the same
    // way the tool card bounds them. An approval prompt that scrolls its own
    // question off the screen is an approval prompt nobody can answer.
    ...bounded(toLines(kind.reason), COLLAPSED.budget, COLLAPSED, "reason line").map(
      (line) => `  ${line}`,
    ),
    // The arguments are shown exactly as the runtime validated them. The
    // client never edits them and never sends back a modified call.
    ...bounded(formatJson(kind.arguments), COLLAPSED.budget, COLLAPSED, "argument line").map(
      (line) => `  ${role.meta(line)}`,
    ),
  ].join("\n");
}
