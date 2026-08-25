/**
 * The working indicator and the footer/status bar.
 *
 * Both answer the same question — *what is the runtime doing right now?* —
 * and both answer it only from facts the runtime published. There is no timer
 * here, no inactivity threshold, no "it has been quiet so it must be
 * thinking". Every state below names the projection field that proves it:
 *
 * ```text
 * Compacting context…    context.compaction_in_progress
 * Waiting for approval…   pendingInteractions for the active attempt
 * Running <tool>…         a foreground execution in state `running`
 * Preparing tool call…    a foreground execution in state `assembled`
 * Thinking…               the streaming message's latest block is reasoning
 * Streaming response…     the streaming message's latest block is text
 * Admitted…               attempt phase `admitted`
 * Working…                attempt phase `running` with nothing more specific
 * ```
 *
 * A phase rustX does not publish is not shown.
 */

import type { PresentationState } from "../../presentation/state.ts";
import type {
  ModelInvocationView,
  RuntimeClientOutcome,
  SessionView,
} from "../../protocol/types.ts";
import { correlateTools, runningTools } from "../../presentation/tools.ts";
import {
  activeBackground,
  describeReasoning,
  sessionLabel,
  unavailableCapabilities,
} from "../../presentation/selectors.ts";
import { role, style, plainText, plainWidth } from "../theme.ts";

/**
 * The working label, or `undefined` when the runtime is not working.
 *
 * A settled attempt is not work, and neither is an attempt this client has
 * merely asked to cancel: cancellation acceptance is not a runtime phase, and
 * inventing a `Cancelling…` state would be the client asserting a lifecycle
 * rustX did not publish.
 */
export function workingStatus(state: PresentationState): string | undefined {
  if (state.context.compaction_in_progress) {
    return "Compacting context…";
  }
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
    const questions = waiting.filter((interaction) => interaction.kind.type === "question");
    const approvals = waiting.length - questions.length;
    if (questions.length > 0 && approvals === 0) {
      return questions.length === 1 ? "Waiting for an answer…" : `Waiting for ${questions.length} answers…`;
    }
    if (questions.length > 0) {
      return `Waiting for ${waiting.length} human responses…`;
    }
    return waiting.length === 1
      ? `Waiting for approval of ${waiting[0]!.kind.type === "approval" ? waiting[0]!.kind.tool_name : "tool"}…`
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

  return "Working…";
}

// ---------------------------------------------------------------------------
// Footer
// ---------------------------------------------------------------------------

/**
 * The model identities, each shown exactly when it is a distinct fact.
 *
 * Three runtime facts can differ at once and the footer must not lose one:
 *
 * ```text
 * configured   what the session asks for            SessionModelView.configured
 * effective    what the runtime would actually use  SessionModelView.effective
 * attempt      what the running attempt froze       AttemptModelView.primary
 * ```
 *
 * When they coincide the footer compresses to one bare model name, which is
 * the common case. As soon as any two differ every one of them is labelled,
 * so `cfg A · eff B · attempt C` is unambiguous and no reader can conclude
 * that the running attempt already moved to the configured model.
 *
 * All three are priority 0: a narrow terminal drops other segments and
 * wraps, but it never silently omits a model identity and never truncates one
 * into a different, shorter, wrong identity.
 */
function modelSegments(state: PresentationState): Segment[] {
  const configured = state.sessionModel.configured.model;
  const effective = state.sessionModel.effective.model;
  const attempt = state.attempt?.model.primary.model;

  const distinct =
    configured !== effective ||
    (attempt !== undefined && attempt !== effective);
  if (!distinct) {
    return [{ text: role.accent(configured), priority: 0 }];
  }

  const segments: Segment[] = [
    { text: role.accent(`cfg ${configured}`), priority: 0 },
  ];
  if (effective !== configured) {
    segments.push({ text: role.accent(`eff ${effective}`), priority: 0 });
  }
  if (attempt !== undefined && attempt !== effective) {
    segments.push({ text: role.pending(`attempt ${attempt}`), priority: 0 });
  }
  return segments;
}

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
 * The footer follows Pi's compact status rhythm while keeping the M9.4
 * Session and model distinctions visible. Context usage is the latest
 * runtime/provider-published input usage divided by the context window
 * published for that model; it is not a client-recomputed history occupancy.
 * Nothing here reads a catalog file, talks to a provider, or exposes client
 * plumbing.
 */
export function renderFooter(
  state: PresentationState,
  connectionState: string,
  width = 120,
  session?: SessionView,
): string {
  const segments = footerSegments(state, connectionState, session);
  return layout(segments, width).join("\n");
}

/** The footer's segments, in display order. Exported for deterministic tests. */
export function footerSegments(
  state: PresentationState,
  connectionState: string,
  session?: SessionView,
): Segment[] {
  const segments: Segment[] = [];
  const attempt = state.attempt;

  segments.push(...modelSegments(state));
  if (session !== undefined) {
    segments.push({
      text: role.chrome(`session ${session.name} · node ${session.active_node}`),
      priority: 1,
    });
  }

  segments.push({
    text: role.meta(`provider ${providerLabel(state.sessionModel.effective)}`),
    priority: 1,
  });

  const working = workingStatus(state);
  if (working !== undefined) {
    segments.push({ text: role.pending(working), priority: 0 });
  } else if (attempt?.phase.type === "settled") {
    segments.push({
      text: outcomeTone(attempt.phase.outcome),
      priority: 1,
    });
  }

  if (attempt?.lastUsage !== undefined) {
    segments.push({
      text: role.meta(
        `↑${compact(attempt.lastUsage.input_tokens)} ↓${compact(attempt.lastUsage.output_tokens)}`,
      ),
      priority: 2,
    });
  } else if (attempt !== undefined && attempt.phase.type !== "settled") {
    segments.push({ text: role.meta("tokens pending"), priority: 3 });
  }

  segments.push({ text: role.meta(contextLabel(state)), priority: 1 });

  const approvalLabel =
    state.effectiveApprovalMode === "full_access" ? "FULL ACCESS" : "policy";
  const approvalPending =
    state.pendingApprovalMode === undefined
      ? ""
      : ` → ${state.pendingApprovalMode === "full_access" ? "FULL ACCESS" : "policy"}`;
  segments.push({
    text: role.pending(`approval ${approvalLabel}${approvalPending}`),
    priority: 0,
  });

  const pending = (state.inbound.pending ?? []).length;
  if (pending > 0) {
    segments.push({ text: role.pending(`queued ${pending}`), priority: 1 });
  }
  const background = activeBackground(state).length;
  if (background > 0) {
    segments.push({ text: style.magenta(`background ${background}`), priority: 1 });
  }
  const interactions = state.pendingInteractions.length;
  if (interactions > 0) {
    segments.push({
      text: role.pending(`human input ${interactions}`),
      priority: 0,
    });
  }
  if (state.runtimeShutdown) {
    segments.push({ text: role.error("draining"), priority: 0 });
  }
  const unavailable = unavailableCapabilities(state);
  if (unavailable.length > 0) {
    segments.push({
      text: role.warning(
        `${unavailable.length} ${unavailable.length === 1 ? "optional capability" : "optional capabilities"} unavailable`,
      ),
      priority: 2,
    });
  }
  segments.push({
    text: role.chrome(connectionState === "connected" ? "online" : "offline"),
    priority: 1,
  });
  segments.push({
    text: role.meta("Ctrl+L model · /help commands"),
    priority: 3,
  });
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
  const thousands = Number((tokens / 1_000).toFixed(1));
  return `${thousands}k`;
}

/** Whether the compact welcome block still has a real turn to introduce. */
export function startupVisible(state: PresentationState): boolean {
  return state.transcript.length === 0;
}

/**
 * A compact context indicator based only on the latest published usage.
 *
 * The numerator is not a tokenization of the transcript. It is the input
 * token count the runtime published for the latest attempt, and the
 * denominator is the published context window for that attempt's frozen
 * model. With no published usage yet, the numerator stays unknown.
 */
export function contextLabel(state: PresentationState): string {
  const usage = state.attempt?.lastUsage;
  const window =
    usage === undefined
      ? state.sessionModel.effective.contextWindow
      : state.attempt?.model.primary.contextWindow ??
        state.sessionModel.effective.contextWindow;
  if (usage === undefined || window <= 0) {
    return `context —/${compact(window)}`;
  }
  const percentage = Math.min(100, Math.round((usage.input_tokens / window) * 100));
  return `context ${percentage}%/${compact(window)}`;
}

/** The model's display provider, derived from the published model reference. */
export function providerLabel(model: ModelInvocationView): string {
  const separator = model.model.indexOf("/");
  const provider = separator > 0 ? model.model.slice(0, separator) : undefined;
  return provider === undefined ? protocolLabel(model.protocol) : provider;
}

/** The published protocol's human-facing label. This is cosmetic only. */
export function protocolLabel(protocol: ModelInvocationView["protocol"]): string {
  switch (protocol) {
    case "openai_chat_completions":
      return "Chat Completions";
    case "openai_responses":
      return "Responses";
    case "anthropic_messages":
      return "Messages";
    default:
      return protocol;
  }
}

function outcomeTone(outcome: RuntimeClientOutcome): string {
  switch (outcome.type) {
    case "completed":
      return role.success("ready");
    case "cancelled":
      return role.warning("cancelled");
    case "timed_out":
      return role.warning("timed out");
    case "limit_exceeded":
      return role.warning("limit exceeded");
    case "failed":
      return role.error("failed");
    default:
      return role.meta("settled");
  }
}

/**
 * The compact welcome block shown before the first real transcript turn.
 *
 * It uses the effective model and native Session projection only. Attachment
 * ids, cursors, request ids, capability revisions, and storage paths stay in
 * `/debug` or out of the client entirely.
 */
export function renderStartup(
  state: PresentationState,
  session?: SessionView,
  width = 120,
): string {
  const model = state.sessionModel.effective;
  const lines = [
    role.strong("rustX"),
    `${role.meta("model")} ${role.accent(model.model)}`,
    `${role.meta(`provider ${providerLabel(model)} · ${protocolLabel(model.protocol)}`)} · ${role.meta(contextLabel(state))} · ${role.meta(`reasoning ${describeReasoning(model)}`)}`,
  ];
  if (session !== undefined) {
    lines.push(
      `${role.meta("session")} ${role.accent(sessionLabel(session))} · ${role.meta(`node ${session.active_node}`)}`,
    );
  }
  lines.push(
    role.meta("Ctrl+L model · Esc cancel · Ctrl+T reasoning · Ctrl+O tools · /help commands"),
  );
  return lines.map((line) => fit(line, width)).join("\n");
}

function fit(text: string, width: number): string {
  if (plainWidth(text) <= width) {
    return text;
  }
  return `${[...plainText(text)].slice(0, Math.max(0, width - 1)).join("")}…`;
}
