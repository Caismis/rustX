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
import { type PresentationPreferences, isExpanded } from "../preferences.ts";
import { role, style } from "../theme.ts";
import { renderToolCard, describeProgress, statusLabel } from "./tool-card.ts";
import { formatJson, preview } from "./tool-renderers.ts";

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
        expanded: isExpanded(preferences, tool.callId),
        previewLines: preferences.previewLines,
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
    `${glyph} ${role.toolTitle(style.bold(execution.tool_name))} ${role.chrome("·")} ${
      terminal ? role.meta(execution.state) : role.pending(execution.state)
    } ${role.meta(execution.execution_id)}`,
  ];
  const progress = describeProgress(execution.progress);
  if (progress !== undefined) {
    lines.push(`  ${role.meta(progress)}`);
  }
  if (execution.result !== undefined) {
    lines.push(`  ${statusLabel(execution.result)}`);
    const body: string[] = [];
    for (const content of execution.result.content ?? []) {
      if (content.type === "text") {
        body.push(...content.text.split("\n"));
      }
      if (content.type === "json") {
        body.push(...formatJson(content.value));
      }
    }
    for (const line of preview(body, {
      expanded: isExpanded(preferences, execution.execution_id),
      previewLines: preferences.previewLines,
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
    `${role.pending("?")} ${role.toolTitle(style.bold(kind.tool_name))} ${role.meta(interaction.id)}`,
    `  ${role.meta(`${kind.mode} · ${originLabel(kind.origin)} · call ${kind.call_id}`)}`,
    `  ${kind.reason}`,
    // The arguments are shown exactly as the runtime validated them. The
    // client never edits them and never sends back a modified call.
    ...formatJson(kind.arguments).map((line) => `  ${role.meta(line)}`),
  ].join("\n");
}
