/**
 * The one visual entity of one logical tool call.
 *
 * ```text
 * assembled            ◇ Bash
 *                        $ cargo test --all
 *
 * running (+progress)  ◐ Bash · running · 842/900
 *                        $ cargo test --all
 *
 * settled              ✓ Bash · 2.8s · exit 0
 *                        $ cargo test --all
 *                        test result: ok. 842 passed
 *                        … 17 more lines · ctrl+o to expand
 * ```
 *
 * The same `ToolCallId` produces the same card through all three states; the
 * card is not three records that happen to be adjacent.
 *
 * Every status word, glyph, duration, and exit code below comes from a
 * runtime-published field. Nothing is read out of the output text: an
 * interrupted call whose stdout says `ok` is still reported as interrupted,
 * and a call with no output is never reported as cancelled.
 *
 * The specialized renderer chosen by tool identity formats the *arguments*
 * and the *result body* and nothing else, so it cannot reach the status line.
 */

import type { ToolExecutionResult } from "../../protocol/types.ts";
import type { CorrelatedTool, ToolLifecycle } from "../../presentation/tools.ts";
import { role, style } from "../theme.ts";
import {
  type ToolCallPresentation,
  type ToolPresentationRenderer,
  type ToolRenderContext,
  genericRenderer,
  genericResultLines,
  parseArguments,
  rendererFor,
} from "./tool-renderers.ts";

/** Renders one correlated tool call as one card. */
export function renderToolCard(
  tool: CorrelatedTool,
  context: ToolRenderContext,
): string {
  const args = parseArguments(tool.argumentsText);
  const renderer = rendererFor(tool.toolId);

  const call = presentCall(renderer.renderCall(args), tool);
  const lines: string[] = [];

  lines.push(
    `${statusGlyph(tool.lifecycle)} ${role.toolTitle(style.bold(call.title))}${statusSuffix(tool.lifecycle)}`,
  );
  if (call.subject !== undefined && call.subject.length > 0) {
    lines.push(`  ${call.subject}`);
  }
  for (const line of call.lines ?? []) {
    lines.push(`  ${line}`);
  }

  if (tool.lifecycle.type === "settled") {
    for (const line of resultBody(renderer, tool.lifecycle.result, args, context)) {
      lines.push(`  ${line}`);
    }
    for (const line of terminalDetail(tool.lifecycle.result)) {
      lines.push(`  ${line}`);
    }
  }

  return lines.join("\n");
}

/**
 * The call header, with the generic renderer as the guaranteed fallback.
 *
 * A specialized renderer that does not recognise its arguments — a malformed
 * shape, a partially streamed fragment, a schema that moved — returns
 * `undefined` and loses nothing but its own formatting.
 */
function presentCall(
  presentation: ToolCallPresentation | undefined,
  tool: CorrelatedTool,
): ToolCallPresentation {
  const fallbackTitle = tool.name || tool.toolId;
  if (presentation === undefined) {
    const generic = genericRenderer.renderCall(
      parseArguments(tool.argumentsText),
    );
    return {
      title: fallbackTitle,
      subject: generic?.subject,
      lines: generic?.lines ?? rawArgumentLines(tool),
    };
  }
  return {
    ...presentation,
    title: presentation.title.length > 0 ? presentation.title : fallbackTitle,
  };
}

/** Arguments that are not JSON yet are still shown, verbatim and bounded. */
function rawArgumentLines(tool: CorrelatedTool): string[] {
  const text = tool.argumentsText.trim();
  return text.length === 0 ? [] : [role.meta(text)];
}

function resultBody(
  renderer: ToolPresentationRenderer,
  result: ToolExecutionResult,
  args: unknown,
  context: ToolRenderContext,
): string[] {
  const specialized = renderer.renderResult?.(result, args, context);
  const body = specialized ?? genericResultLines(result, context);

  // A failure or denial reason is runtime-published prose and is always
  // shown, whatever the renderer decided about the body.
  const reason: string[] = [];
  if (result.status.type === "failed") {
    reason.push(role.error(result.status.error));
  }
  if (result.status.type === "denied") {
    reason.push(role.warning(result.status.reason));
  }
  return [...reason, ...body];
}

/**
 * Terminal metadata the runtime owns.
 *
 * `TruncationState` is the *runtime's* truncation of the result it committed.
 * It is reported unconditionally and is never confused with the client's own
 * collapse, which is undone by expanding the card while this is not.
 */
function terminalDetail(result: ToolExecutionResult): string[] {
  if (result.truncation?.truncated !== true) {
    return [];
  }
  const original = result.truncation.original_bytes;
  return [
    role.meta(
      `⚠ runtime-truncated result${original === undefined ? "" : ` (from ${original} bytes)`}`,
    ),
  ];
}

// ---------------------------------------------------------------------------
// Runtime-owned status presentation
// ---------------------------------------------------------------------------

function statusGlyph(lifecycle: ToolLifecycle): string {
  switch (lifecycle.type) {
    case "assembled":
      return role.meta("◇");
    case "running":
      return role.pending("◐");
    default:
      return settledGlyph(lifecycle.result);
  }
}

function settledGlyph(result: ToolExecutionResult): string {
  switch (result.status.type) {
    case "success":
      return role.success("✓");
    case "failed":
      return role.error("✗");
    case "denied":
      return role.warning("⊘");
    case "cancelled":
      return role.warning("⊗");
    case "timed_out":
      return role.warning("⧖");
    case "interrupted":
      return role.warning("!");
    default:
      return role.meta("·");
  }
}

/** The ` · a · b · c` tail of the header line. */
function statusSuffix(lifecycle: ToolLifecycle): string {
  const parts = statusParts(lifecycle);
  return parts.length === 0 ? "" : ` ${role.chrome("·")} ${parts.join(role.chrome(" · "))}`;
}

function statusParts(lifecycle: ToolLifecycle): string[] {
  switch (lifecycle.type) {
    case "assembled":
      return [role.meta("preparing")];
    case "running": {
      const parts = [role.pending("running")];
      const progress = describeProgress(lifecycle.progress);
      if (progress !== undefined) {
        parts.push(role.meta(progress));
      }
      return parts;
    }
    default:
      return settledParts(lifecycle.result);
  }
}

function settledParts(result: ToolExecutionResult): string[] {
  const parts = [statusLabel(result)];
  parts.push(role.meta(formatDuration(result.duration_ms)));
  if (result.exit_code !== undefined) {
    parts.push(role.meta(`exit ${result.exit_code}`));
  }
  return parts;
}

/** The runtime's own settlement, spelled out. Never softened. */
export function statusLabel(result: ToolExecutionResult): string {
  switch (result.status.type) {
    case "success":
      return role.success("ok");
    case "failed":
      return role.error("failed");
    case "denied":
      return role.warning(`denied (${result.status.reason})`);
    case "cancelled":
      return role.warning(`cancelled (${result.status.reason})`);
    case "timed_out":
      return role.warning("timed out");
    case "interrupted":
      // Interrupted means the outcome is genuinely unknown. It is not a quiet
      // success and it is not a failure.
      return role.warning("interrupted (outcome unknown)");
    default:
      return role.meta("settled");
  }
}

export function describeProgress(
  progress: { message?: string; completed?: number; total?: number } | undefined,
): string | undefined {
  if (progress === undefined) {
    return undefined;
  }
  const pieces: string[] = [];
  if (progress.message !== undefined) {
    pieces.push(progress.message);
  }
  if (progress.completed !== undefined) {
    pieces.push(
      progress.total === undefined
        ? String(progress.completed)
        : `${progress.completed}/${progress.total}`,
    );
  }
  return pieces.length === 0 ? undefined : pieces.join(" · ");
}

/** A compact duration from the runtime's own millisecond measurement. */
export function formatDuration(durationMs: number): string {
  if (durationMs < 1_000) {
    return `${durationMs}ms`;
  }
  if (durationMs < 60_000) {
    return `${(durationMs / 1_000).toFixed(1)}s`;
  }
  const minutes = Math.floor(durationMs / 60_000);
  const seconds = Math.round((durationMs % 60_000) / 1_000);
  return `${minutes}m${String(seconds).padStart(2, "0")}s`;
}
