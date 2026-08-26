/**
 * The one visual entity of one logical tool call.
 *
 * ```text
 * assembled            ◇ Bash $ cargo test --all
 *
 * running (+progress)  ◐ Bash $ cargo test --all · running · 842/900
 *
 * settled              ✓ Bash $ cargo test --all · ok · 2.8s · exit 0
 *                      test result: ok. 842 passed
 *                      … 17 more lines · ctrl+o to expand
 * ```
 *
 * The whole card is drawn on one background band — pending, settled-well, or
 * settled-badly — which is Pi's visual grammar for a tool call. The band is
 * chosen by {@link cardBackground} and filled by the app shell, the one layer
 * that knows the terminal width. It restates the lifecycle rather than
 * carrying it: three bands cannot express six settlements, so the status
 * words below are what actually say `denied`, `timed out`, or `interrupted`.
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
 * A card is laid out in bands, and every one of them is bounded:
 *
 * ```text
 * header         glyph, title, runtime lifecycle   clipped, always visible
 * subject        the one-line identity of the call bounded, always visible,
 *                                                  drawn on the header line
 * call detail    argument JSON, a diff, a command  bounded when collapsed
 * reason         failure / denial prose            bounded when collapsed
 * result summary runtime-published counts          bounded, always visible
 * result detail  the body                          bounded when collapsed
 * truncation     the runtime's own TruncationState always visible
 * ```
 *
 * **Every** band above carries a finite budget, and each budget is finite in
 * two dimensions: a line count *and* a content length. One dimension is not a
 * bound. `{"payload": "<100 kB>"}` is three pretty-printed lines, a 50 kB
 * path is one line, and a 50 kB denial reason is one line — a height-only
 * budget shows all three in full. {@link ../preferences.ts} holds the one
 * policy, {@link ./tool-renderers.ts} holds the one function that applies it,
 * and renderers never see the collapse context, which is what makes the bound
 * impossible for a present or future renderer to forget.
 *
 * The two *detail* bands get their own budget rather than sharing one, so a
 * call with a huge argument object never squeezes its result off the screen
 * (and the reverse).
 *
 * Expanding a card spends only facts the client already holds — the published
 * arguments and the committed result — so it issues no runtime request, no
 * re-execution, no filesystem read, and no refetch. The subject stays on the
 * header line either way; expanded, it is the complete published value.
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
import {
  HEADER_BUDGET,
  SUBJECT_BUDGET,
  SUMMARY_BUDGET,
} from "../preferences.ts";
import { sanitizeData, sanitizeField, sanitizeLine } from "../../sanitize.ts";
import { type BackgroundRole, role, style } from "../theme.ts";
import {
  type ToolCallPresentation,
  type ToolPresentationRenderer,
  type ToolRenderContext,
  type ToolResultPresentation,
  bounded,
  boundedLine,
  clipText,
  genericRenderer,
  genericResultLines,
  parseArguments,
  preview,
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

/**
 * The background band one card is drawn on.
 *
 * Three bands for six settlements, so the band is never the only statement
 * of what happened: it separates "still working" from "settled well" from
 * "settled badly", and the card's own status words carry which of the four
 * bad settlements it was. A card with no lifecycle yet — a `call` part whose
 * result renders further down — is still in flight and gets the pending
 * band.
 */
export function cardBackground(
  lifecycle: ToolLifecycle | undefined,
): BackgroundRole {
  if (lifecycle === undefined || lifecycle.type !== "settled") {
    return "toolPending";
  }
  return lifecycle.result.status.type === "success"
    ? "toolSuccess"
    : "toolError";
}

/** Renders one correlated tool call as one card, or as one part of one. */
export function renderToolCard(
  published: CorrelatedTool,
  context: ToolRenderContext,
  part: ToolCardPart = "full",
): string {
  // Nothing below this line reads `published`. Everything a renderer, a
  // budget, or the header touches comes from the reduced copy, because the
  // reduction has to happen while it is still possible to tell content from
  // styling. See {@link drawableTool}.
  const tool = drawableTool(published);
  const args = sanitizeData(parseArguments(tool.argumentsText));
  const renderer = rendererFor(tool.toolId);
  const call = presentCall(renderer.renderCall(args), tool, args);
  const lines: string[] = [];

  if (part === "continuation") {
    // The terminal continuation of the same entity, at the canonical position
    // of the committed result. It repeats the call's identity — title and
    // subject — so it reads as that call settling rather than as a second
    // tool record, and it carries the runtime lifecycle.
    lines.push(
      `${role.chrome("↳")} ${statusGlyph(tool.lifecycle)} ${header(call, context)}${statusSuffix(tool.lifecycle)}`,
    );
    pushResult(lines, renderer, tool, args, context);
    return drawable(lines);
  }

  // The `call` part carries no lifecycle suffix on purpose. Its result is
  // rendered below, at the result's own canonical position, and restating the
  // settlement here would report one runtime fact twice.
  lines.push(
    `${part === "call" ? role.meta("◇") : statusGlyph(tool.lifecycle)} ${header(call, context)}${
      part === "call" ? `${role.chrome(" · ")}${role.meta("result below")}` : statusSuffix(tool.lifecycle)
    }`,
  );
  for (const line of preview(call.detail ?? [], context, "argument line")) {
    lines.push(`  ${line}`);
  }

  if (part === "full") {
    pushResult(lines, renderer, tool, args, context);
  }
  return drawable(lines);
}

/**
 * The card's one content boundary: the entity as this terminal may draw it.
 *
 * A card composes lines out of two externally-derived sources — the model's
 * own tool arguments and the tool's own output — and neither is validated
 * for *this* terminal. The call band is the sharper case: it is drawn from
 * arguments while the assistant message is still streaming, so a call the
 * runtime will reject has already been printed by the time it is rejected,
 * and no amount of input validation downstream can unprint it.
 *
 * The reduction happens **here**, before a renderer or the theme has touched
 * anything, and that ordering is the whole point. A card is assembled out of
 * styled fragments, and once it is assembled an `ESC` the theme wrote and an
 * `ESC` that arrived in a tool argument are indistinguishable — so a filter
 * on the finished line that spares "this client's own colours" spares the
 * model's `ESC[8m` too, and hidden text, a forged colour, or a reset theme
 * goes to the terminal. Reducing the entity first means every renderer,
 * present and future, is handed content that was never dangerous, and the
 * line filter below is left with nothing but layout to enforce.
 *
 * Argument *text* and parsed argument *values* are reduced separately on
 * purpose: `"\u001b"` inside published JSON is six harmless characters in the
 * text and one `ESC` after `JSON.parse`, so reducing only the text would
 * leave every renderer that reads a field wide open.
 */
function drawableTool(tool: CorrelatedTool): CorrelatedTool {
  return {
    callId: tool.callId,
    toolId: sanitizeField(tool.toolId),
    name: sanitizeField(tool.name),
    argumentsText: sanitizeField(tool.argumentsText, true),
    // One reduction for the whole lifecycle: a runtime-published progress
    // message, a failure reason, result text, result JSON, and any field a
    // future result grows are all covered by construction rather than by a
    // list this function would have to keep current.
    lifecycle: sanitizeData(tool.lifecycle) as ToolLifecycle,
    committed: tool.committed,
  };
}

/**
 * The card's one *layout* boundary.
 *
 * Content is already reduced by {@link drawableTool}, so what is left to
 * enforce is the row count: because no surviving line can contain a line
 * break, the number of physical rows a card draws is exactly the number of
 * lines it built — which is what every band budget above already assumed.
 * The styling this client emitted survives, and by this point that is the
 * only styling there is.
 */
function drawable(lines: string[]): string {
  return lines.map(sanitizeLine).join("\n");
}

/**
 * The one identity line of a card: what was called, and on what.
 *
 * Title and subject share the line, the way Pi draws a tool call — `read
 * src/lib.rs`, `$ cargo test` — rather than stacking the subject underneath.
 * Both halves are still bounded by their own contracts: the title is header
 * chrome and clipped unconditionally, the subject is externally derived and
 * bounded to one line.
 */
function header(call: ToolCallPresentation, context: ToolRenderContext): string {
  const subject = boundedSubject(call, context);
  return subject === undefined ? title(call) : `${title(call)} ${subject}`;
}

/**
 * The card title, which is header chrome and therefore never expandable.
 *
 * Usually a renderer constant (`Bash`), but the fallback is the runtime's
 * published tool name or tool id, which is externally derived. Clipped
 * unconditionally: a header is drawn the same open or shut, so there is no
 * expanded form in which the rest could appear.
 */
function title(call: ToolCallPresentation): string {
  return role.toolTitle(style.bold(clipText(call.title, HEADER_BUDGET.maxChars)));
}

/**
 * The subject band, which is one line by contract and one line in fact.
 *
 * A renderer builds a subject from published arguments — a path, a command's
 * first line, a pattern — so it is externally derived in both dimensions: it
 * may contain a newline, and it may be 50 kB long. The shell enforces the
 * band rather than trusting every renderer to.
 *
 * The rule, stated once: the subject is always exactly one line; collapsed it
 * is bounded to {@link SUBJECT_BUDGET} with an inline elision marker, and
 * expanded it is the complete published value the client already holds.
 */
function boundedSubject(
  call: ToolCallPresentation,
  context: ToolRenderContext,
): string | undefined {
  if (call.subject === undefined || call.subject.length === 0) {
    return undefined;
  }
  const [first, ...rest] = call.subject.split("\n");
  const head = boundedLine(first ?? "", SUBJECT_BUDGET, context);
  return `${head}${rest.length === 0 ? "" : role.meta(" …")}`;
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
  args: unknown,
): ToolCallPresentation {
  const fallbackTitle = tool.name || tool.toolId;
  if (presentation === undefined) {
    // The same reduced arguments the specialized renderer was handed: a
    // second `parseArguments` here would decode the published text again and
    // hand the generic renderer values nothing had reduced.
    const generic = genericRenderer.renderCall(args);
    return {
      title: fallbackTitle,
      subject: generic?.subject,
      detail: generic?.detail ?? rawArgumentLines(tool),
    };
  }
  return {
    ...presentation,
    title: presentation.title.length > 0 ? presentation.title : fallbackTitle,
  };
}

/**
 * Arguments that are not JSON yet, shown verbatim as ordinary detail.
 *
 * A streaming fragment is frequently one unbroken line and has no structure
 * to exploit, which is exactly the case the two-dimensional budget exists
 * for. Nothing special happens here: the band goes through the same bound as
 * every other, and the content budget is what stops it.
 */
function rawArgumentLines(tool: CorrelatedTool): string[] {
  const text = tool.argumentsText.trim();
  return text.length === 0
    ? []
    : text.split("\n").map((line) => role.meta(line));
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

  // A failure or denial reason is runtime-published prose, shown whatever the
  // renderer decided about the body, and shown *here* rather than in the
  // status header. The header states the settlement the runtime published —
  // `failed`, `denied` — and this band carries the explanation, bounded. Prose
  // of unbounded length has no business in an always-visible, never-collapsed
  // header, and putting it in both places reported one fact twice.
  const reason: string[] = [];
  if (result.status.type === "failed") {
    reason.push(...toLines(result.status.error).map((line) => role.error(line)));
  }
  if (result.status.type === "denied") {
    reason.push(...toLines(result.status.reason).map((line) => role.warning(line)));
  }
  // Verbatim tool output reads as output, not as answer: one colour for the
  // whole body, the way Pi draws it. The band is applied here rather than in
  // a renderer so no present or future renderer can opt its output out of
  // looking like output.
  const detail = (body.detail ?? []).map((line) => role.toolOutput(line));
  return [
    ...preview(reason, context, "reason line"),
    // `summary` is contractually a short structural fact — `42 matches`,
    // `wrote 120 bytes` — and is bounded anyway. A documented contract a
    // renderer could forget is not an invariant, and this band is always
    // visible, so it is the obvious way to smuggle arbitrary tool prose past
    // progressive disclosure. Now it is not.
    ...bounded(body.summary ?? [], SUMMARY_BUDGET, context, "summary line"),
    ...preview(detail, context),
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

/**
 * The runtime's own settlement, spelled out. Never softened.
 *
 * The header names the settlement and nothing else. `failed` and `denied`
 * both carry runtime-published prose, and prose belongs in the bounded reason
 * band below — restating it here would put an unbounded, uncollapsible string
 * in the header and report the same fact twice. `cancelled` keeps its reason
 * because a `CancellationReason` is a small typed enum, not prose.
 */
export function statusLabel(result: ToolExecutionResult): string {
  switch (result.status.type) {
    case "success":
      return role.success("ok");
    case "failed":
      return role.error("failed");
    case "denied":
      return role.warning("denied");
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

/**
 * The runtime's own progress, compactly.
 *
 * `message` is runtime-published prose drawn into a header that has no
 * expanded form, so it is clipped like every other header fragment.
 */
export function describeProgress(
  progress: { message?: string; completed?: number; total?: number } | undefined,
): string | undefined {
  if (progress === undefined) {
    return undefined;
  }
  const pieces: string[] = [];
  if (progress.message !== undefined) {
    pieces.push(clipText(progress.message, HEADER_BUDGET.maxChars));
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

/**
 * A compact duration from the runtime's own millisecond measurement.
 *
 * The unit is chosen from the *rounded* value, not the raw one. Choosing the
 * bucket first and rounding inside it lets a value round out of its own
 * bucket: 59,999 ms rendered as `60.0s`, and 119,999 ms as `1m60s`, because
 * `floor(ms / 60000)` and `round(ms % 60000 / 1000)` were rounded
 * independently. Rounding once and deriving both components from that single
 * value makes the seconds component 0–59 by construction.
 */
export function formatDuration(durationMs: number): string {
  if (durationMs < 1_000) {
    return `${durationMs}ms`;
  }
  const tenths = Math.round(durationMs / 100);
  if (tenths < 600) {
    return `${(tenths / 10).toFixed(1)}s`;
  }
  const totalSeconds = Math.round(durationMs / 1_000);
  const minutes = Math.floor(totalSeconds / 60);
  const seconds = totalSeconds % 60;
  return `${minutes}m${String(seconds).padStart(2, "0")}s`;
}
