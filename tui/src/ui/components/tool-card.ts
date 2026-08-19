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
 *
 * ## The shell owns progressive disclosure
 *
 * A card is laid out in bands, and only two of them are ever bounded:
 *
 * ```text
 * header        glyph, title, runtime lifecycle   always visible
 * subject       the one-line identity of the call always visible
 * call detail   argument JSON, a diff, a command  bounded when collapsed
 * reason        failure / denial prose            always visible
 * result summary runtime-published counts         always visible
 * result detail the body                          bounded when collapsed
 * truncation    the runtime's own TruncationState always visible
 * ```
 *
 * Both detail bands get their own budget rather than sharing one, so a call
 * with a huge argument object never squeezes its result off the screen (and
 * the reverse). Renderers never see the collapse context, which is what makes
 * the bound impossible for a present or future renderer to forget.
 *
 * ## Cards split when, and only when, folding would reorder canonical facts
 *
 * A card may be drawn in three parts, chosen by the transcript, never by this
 * module:
 *
 * ```text
 * "full"          call and result in one card at the call's position
 * "call"          the call alone, at the call's position
 * "continuation"  the settled result alone, at the result's position
 * ```
 *
 * The invariant is in {@link ../../presentation/tools.ts}: a committed result
 * folds into its call card only when folding cannot move it across unrelated
 * canonical content. When it cannot fold, the same entity renders as a `call`
 * part at the `tool_call` block and a `continuation` part at the canonical
 * result message — one identity, two fragments, canonical order intact, and
 * never the old duplicated call/result log blocks.
 */

import type { ToolExecutionResult } from "../../protocol/types.ts";
import type { CorrelatedTool, ToolLifecycle } from "../../presentation/tools.ts";
import { RAW_FRAGMENT_PREVIEW_CHARS } from "../preferences.ts";
import { role, style } from "../theme.ts";
import {
  type ToolCallPresentation,
  type ToolPresentationRenderer,
  type ToolRenderContext,
  type ToolResultPresentation,
  genericRenderer,
  genericResultLines,
  parseArguments,
  preview,
  previewText,
  rendererFor,
  toLines,
} from "./tool-renderers.ts";

/**
 * Which part of one entity's card to draw.
 *
 * Chosen by the transcript from the canonical fold invariant, never inferred
 * here: this module renders what it is told to render.
 */
export type ToolCardPart = "full" | "call" | "continuation";

/** Renders one correlated tool call as one card, or as one part of one. */
export function renderToolCard(
  tool: CorrelatedTool,
  context: ToolRenderContext,
  part: ToolCardPart = "full",
): string {
  const args = parseArguments(tool.argumentsText);
  const renderer = rendererFor(tool.toolId);
  const call = presentCall(renderer.renderCall(args), tool, context);
  const lines: string[] = [];

  if (part === "continuation") {
    // The terminal continuation of the same entity, at the canonical position
    // of the committed result. It repeats the call's identity — title and
    // subject — so it reads as that call settling rather than as a second
    // tool record, and it carries the runtime lifecycle.
    lines.push(
      `${role.chrome("↳")} ${statusGlyph(tool.lifecycle)} ${role.toolTitle(style.bold(call.title))}${statusSuffix(tool.lifecycle)}`,
    );
    pushSubject(lines, call);
    pushResult(lines, renderer, tool, args, context);
    return lines.join("\n");
  }

  // The `call` part carries no lifecycle suffix on purpose. Its result is
  // rendered below, at the result's own canonical position, and restating the
  // settlement here would report one runtime fact twice.
  lines.push(
    `${part === "call" ? role.meta("◇") : statusGlyph(tool.lifecycle)} ${role.toolTitle(style.bold(call.title))}${
      part === "call" ? `${role.chrome(" · ")}${role.meta("result below")}` : statusSuffix(tool.lifecycle)
    }`,
  );
  pushSubject(lines, call);
  for (const line of preview(call.detail ?? [], context, "argument line")) {
    lines.push(`  ${line}`);
  }

  if (part === "full") {
    pushResult(lines, renderer, tool, args, context);
  }
  return lines.join("\n");
}

/**
 * The subject band, which is one line by contract and one line in fact.
 *
 * A renderer builds a subject from published arguments — a path, a command's
 * first line, a pattern — and a published string may contain a newline. The
 * shell enforces the band rather than trusting every renderer to, so an
 * argument with embedded newlines cannot turn the always-visible band into an
 * unbounded one.
 */
function pushSubject(lines: string[], call: ToolCallPresentation): void {
  if (call.subject === undefined || call.subject.length === 0) {
    return;
  }
  const [first, ...rest] = call.subject.split("\n");
  lines.push(`  ${first ?? ""}${rest.length === 0 ? "" : role.meta(" …")}`);
}

/** The settled bands: reason, summary, bounded body, runtime truncation. */
function pushResult(
  lines: string[],
  renderer: ToolPresentationRenderer,
  tool: CorrelatedTool,
  args: unknown,
  context: ToolRenderContext,
): void {
  if (tool.lifecycle.type !== "settled") {
    return;
  }
  const result = tool.lifecycle.result;
  for (const line of resultBody(renderer, result, args, context)) {
    lines.push(`  ${line}`);
  }
  for (const line of terminalDetail(result)) {
    lines.push(`  ${line}`);
  }
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
  context: ToolRenderContext,
): ToolCallPresentation {
  const fallbackTitle = tool.name || tool.toolId;
  if (presentation === undefined) {
    const generic = genericRenderer.renderCall(
      parseArguments(tool.argumentsText),
    );
    return {
      title: fallbackTitle,
      subject: generic?.subject,
      detail: generic?.detail ?? rawArgumentLines(tool, context),
    };
  }
  return {
    ...presentation,
    title: presentation.title.length > 0 ? presentation.title : fallbackTitle,
  };
}

/**
 * Arguments that are not JSON yet, shown verbatim and bounded.
 *
 * A streaming fragment has no line structure to bound, so this band is bounded
 * by characters. It is bounded *here*, before the line budget applies, so a
 * single 100 kB fragment cannot pass through as "one line".
 */
function rawArgumentLines(
  tool: CorrelatedTool,
  context: ToolRenderContext,
): string[] {
  const text = tool.argumentsText.trim();
  // Character bound first, then hand ordinary lines to the shell's line
  // budget: a fragment is bounded whether its bulk is width or height.
  return previewText(text, context, RAW_FRAGMENT_PREVIEW_CHARS)
    .flatMap((chunk) => chunk.split("\n"))
    .map((line) => role.meta(line));
}

function resultBody(
  renderer: ToolPresentationRenderer,
  result: ToolExecutionResult,
  args: unknown,
  context: ToolRenderContext,
): string[] {
  const specialized: ToolResultPresentation | undefined =
    renderer.renderResult?.(result, args);
  const body = specialized ?? genericResultLines(result);

  // A failure or denial reason is runtime-published prose and is always
  // shown, whatever the renderer decided about the body. It is prose the
  // runtime wrote, so it gets its own budget rather than none at all: the
  // beginning of the explanation is always on screen, and a pathologically
  // long one still cannot fill the terminal.
  const reason: string[] = [];
  if (result.status.type === "failed") {
    reason.push(...toLines(result.status.error).map((line) => role.error(line)));
  }
  if (result.status.type === "denied") {
    reason.push(...toLines(result.status.reason).map((line) => role.warning(line)));
  }
  return [
    ...preview(reason, context, "reason line"),
    ...(body.summary ?? []),
    ...preview(body.detail ?? [], context),
  ];
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
