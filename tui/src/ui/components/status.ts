/**
 * The working indicator and the footer/status bar.
 *
 * Both answer the same question — *what is the runtime doing right now?* —
 * and both answer it only from facts the runtime published. There is no timer
 * here, no inactivity threshold, no "it has been quiet so it must be
 * thinking". Every state below names the projection field that proves it:
 *
 * ```text
 * Waiting for approval…   pendingInteractions for the active attempt
 * Running <tool>…         a foreground execution in state `running`
 * Preparing tool call…    a foreground execution in state `assembled`
 * Thinking…               the streaming message's latest block is reasoning
 * Streaming response…     the streaming message's latest block is text
 * Admitted…               attempt phase `admitted`
 * Working…                attempt phase `running` with nothing more specific
 * ```
 *
 * A phase rustX does not publish is not shown. Context compaction, for
 * instance, is published as a completed fact (`context_compacted`) and never
 * as an in-progress one, so there is no `Compacting context…` state to prove.
 */

import type { PresentationState } from "../../presentation/state.ts";
import { correlateTools, runningTools } from "../../presentation/tools.ts";
import { activeBackground, outcomeLabel } from "../../presentation/selectors.ts";
import { role, style, plainWidth } from "../theme.ts";

/**
 * The working label, or `undefined` when the runtime is not working.
 *
 * A settled attempt is not work, and neither is an attempt this client has
 * merely asked to cancel: cancellation acceptance is not a runtime phase, and
 * inventing a `Cancelling…` state would be the client asserting a lifecycle
 * rustX did not publish.
 */
export function workingStatus(state: PresentationState): string | undefined {
  const attempt = state.attempt;
  if (attempt === undefined || attempt.phase.type === "settled") {
    return undefined;
  }
  if (attempt.phase.type === "admitted") {
    return "Admitted…";
  }

  const waiting = state.pendingInteractions.filter(
    (interaction) => interaction.attempt_id === attempt.attemptId,
  );
  if (waiting.length > 0) {
    return waiting.length === 1
      ? `Waiting for approval of ${waiting[0]!.kind.tool_name}…`
      : `Waiting for ${waiting.length} approvals…`;
  }

  const correlation = correlateTools(state);
  const running = runningTools(correlation);
  if (running.length > 0) {
    const names = running.map((tool) => tool.name || tool.toolId);
    return `Running ${names.join(", ")}…`;
  }

  const assembling = (attempt.foreground ?? []).some(
    (execution) => execution.state.type === "assembled",
  );
  if (assembling) {
    return "Preparing tool call…";
  }

  const streaming = state.transcript.findLast(
    (entry) => entry.kind === "streaming" && entry.attemptId === attempt.attemptId,
  );
  if (streaming?.kind === "streaming") {
    const latest = streaming.blocks[streaming.blocks.length - 1];
    if (latest?.kind === "reasoning") {
      return "Thinking…";
    }
    if (latest?.kind === "text" || latest?.kind === "refusal") {
      return "Streaming response…";
    }
    if (latest?.kind === "tool_call") {
      return "Preparing tool call…";
    }
  }

  return `Working… (turn ${attempt.turn})`;
}

// ---------------------------------------------------------------------------
// Footer
// ---------------------------------------------------------------------------

/**
 * One footer segment.
 *
 * `priority` is how badly the segment deserves the space: 0 never drops, and
 * higher numbers are given up first when the terminal is narrow. Degrading is
 * dropping whole segments, never truncating a model name into a lie.
 */
interface Segment {
  text: string;
  priority: number;
}

/** How many footer lines a wide terminal may use. */
const MAX_FOOTER_LINES = 2;

/**
 * The footer, laid out for the available width.
 *
 * Everything here is a runtime fact plus the client's own connection state,
 * which the client genuinely owns. Nothing is computed that rustX does not
 * publish: there is no context-window percentage, no cost, and no provider
 * price table, because the Runtime Client publishes none of those.
 */
export function renderFooter(
  state: PresentationState,
  connectionState: string,
  width = 120,
): string {
  const segments = footerSegments(state, connectionState);
  return layout(segments, width).join("\n");
}

/** The footer's segments, in display order. Exported for deterministic tests. */
export function footerSegments(
  state: PresentationState,
  connectionState: string,
): Segment[] {
  const segments: Segment[] = [];
  const attempt = state.attempt;

  // The session's *desired* model. Always first, never dropped.
  segments.push({ text: role.accent(state.sessionModel.configured.model), priority: 0 });

  if (attempt !== undefined) {
    // The attempt froze its model at admission. While it runs on A and the
    // session has moved to B, both are shown; the footer never claims the
    // running attempt already uses B.
    if (attempt.model.primary.model !== state.sessionModel.effective.model) {
      segments.push({
        text: role.pending(`attempt ${attempt.model.primary.model}`),
        priority: 0,
      });
    }
    segments.push({
      text:
        attempt.phase.type === "settled"
          ? role.meta(outcomeLabel(attempt.phase.outcome))
          : role.pending(`${attempt.phase.type} · turn ${attempt.turn}`),
      priority: 1,
    });
    if (attempt.lastUsage !== undefined) {
      segments.push({
        text: role.meta(
          `${compact(attempt.lastUsage.input_tokens)}in ${compact(attempt.lastUsage.output_tokens)}out`,
        ),
        priority: 2,
      });
    } else if (attempt.phase.type !== "settled") {
      segments.push({ text: role.meta("usage pending"), priority: 3 });
    }
  }

  const pending = (state.inbound.pending ?? []).length;
  if (pending > 0) {
    segments.push({ text: role.pending(`inbox ${pending}`), priority: 1 });
  }
  const background = activeBackground(state).length;
  if (background > 0) {
    segments.push({ text: style.magenta(`bg ${background}`), priority: 1 });
  }
  const interactions = state.pendingInteractions.length;
  if (interactions > 0) {
    segments.push({
      text: role.pending(`approvals ${interactions}`),
      priority: 0,
    });
  }
  if (state.runtimeShutdown) {
    segments.push({ text: role.error("shutting down"), priority: 0 });
  }
  segments.push({ text: role.meta(`cap r${state.capabilities.revision}`), priority: 3 });
  segments.push({ text: role.chrome(connectionState), priority: 1 });
  return segments;
}

/**
 * Packs segments into at most {@link MAX_FOOTER_LINES} lines of `width`.
 *
 * Narrow terminals degrade by dropping the least important segments, in
 * priority order, until the rest fit. They never produce one unbounded line
 * and never silently rewrite a fact to make it shorter.
 */
function layout(segments: Segment[], width: number): string[] {
  const separator = " · ";
  let kept = segments;
  for (;;) {
    const lines = pack(kept, width, separator);
    if (lines.length <= MAX_FOOTER_LINES) {
      return lines;
    }
    const droppable = kept
      .map((segment, index) => ({ segment, index }))
      .filter((entry) => entry.segment.priority > 0)
      .sort((left, right) => right.segment.priority - left.segment.priority);
    const victim = droppable[0];
    if (victim === undefined) {
      // Everything left is essential. A terminal too narrow for the essential
      // facts gets more rows; it never gets a footer that quietly omits one.
      return lines;
    }
    kept = kept.filter((_, index) => index !== victim.index);
  }
}

function pack(segments: Segment[], width: number, separator: string): string[] {
  const lines: string[] = [];
  let current = "";
  for (const segment of segments) {
    const candidate = current.length === 0 ? segment.text : `${current}${role.chrome(separator)}${segment.text}`;
    if (current.length > 0 && plainWidth(candidate) > width) {
      lines.push(current);
      current = segment.text;
      continue;
    }
    current = candidate;
  }
  if (current.length > 0) {
    lines.push(current);
  }
  return lines.length === 0 ? [""] : lines;
}

/** Token counts, shortened but never rounded into a different number class. */
function compact(tokens: number): string {
  if (tokens < 10_000) {
    return String(tokens);
  }
  return `${(tokens / 1_000).toFixed(1)}k`;
}
