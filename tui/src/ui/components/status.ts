/**
 * The working indicator and the footer/status bar.
 *
 * Working status answers what is executing or waiting now. The footer answers
 * which stable model, policy and context apply. Both use native facts only.
 * There is no timer here, no inactivity threshold, no "it has been quiet so it must be
 * thinking". Every state below names the projection field that proves it:
 *
 * ```text
 * Compacting context…    context.compaction_in_progress
 * Waiting for approval…   pendingInteractions across the supervised tree
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
  SessionView,
} from "../../protocol/types.ts";
import { correlateTools, runningTools } from "../../presentation/tools.ts";
import {
  approvalLabel,
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
  const waiting = state.pendingInteractions;
  if (waiting.length > 0) {
    const kind = waiting[0]!.request.kind;
    if (waiting.some((interaction) => interaction.request.kind.type !== kind.type)) {
      return `Waiting for ${waiting.length} human responses…`;
    }
    switch (kind.type) {
      case "questionnaire":
        return waiting.length === 1
          ? "Waiting for questionnaire…"
          : `Waiting for ${waiting.length} questionnaires…`;
      case "review":
        return waiting.length === 1
          ? "Waiting for human review…"
          : `Waiting for ${waiting.length} human reviews…`;
      case "approval":
        return waiting.length === 1
          ? `Waiting for approval of ${kind.tool_name}…`
          : `Waiting for ${waiting.length} approvals…`;
    }
  }
  const attempt = state.attempt;
  if (attempt === undefined || attempt.phase.type === "settled") {
    return undefined;
  }
  if (attempt.phase.type === "admitted") {
    return "Admitted…";
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
 * Current/frozen truth comes first; a different effective model is explicitly
 * next. Configured disagreement is secondary (full diagnostics in /settings).
 * Settled attempts remain history rather than pretending to govern new work.
 * Width degradation drops whole segments, never prefixes of model references.
 */
function modelSegments(state: PresentationState): Segment[] {
  if (state.sessionModel === null) return [{ text: "historical model unavailable", priority: 1 }];
  const configured = state.sessionModel.configured.model;
  const effective = state.sessionModel.effective.model;
  const attempt = state.attempt?.phase.type === "settled"
    ? undefined : state.attempt?.model?.primary.model;

  const distinct =
    configured !== effective ||
    (attempt !== undefined && attempt !== effective);
  if (!distinct) {
    return [{ text: role.accent(configured), priority: 0, model: true }];
  }

  const segments: Segment[] = [];
  if (attempt !== undefined && attempt !== effective) {
    segments.push({ text: role.accent(`attempt ${attempt}`), compact: role.accent(attempt), priority: 0, model: true });
    segments.push({ text: role.accent(`next ${effective}`), priority: 0, model: true });
  } else {
    segments.push({ text: role.accent(`eff ${effective}`), priority: 0, model: true });
  }
  if (configured !== effective) {
    segments.push({ text: role.meta(`cfg ${configured}`), priority: 1, model: true });
  }
  return segments;
}

/**
 * One footer segment.
 *
 * `priority` is the retention order: 0 is essential, and
 * higher numbers are given up first when the terminal is narrow. Degrading is
 * dropping whole segments, never truncating a model name into a lie.
 */
interface Segment {
  text: string;
  priority: number;
  /** Complete alternative label, never a sliced identity. */
  compact?: string;
  model?: boolean;
}

/** The presentation label for the currently attached conversation. */
export interface ConversationContext {
  conversationId: string;
  /** The parent identity when this view was opened from a subagent row. */
  parentConversationId?: string;
  /** Direct `--inspect-conversation` attachments have no parent in this UI. */
  readOnly?: boolean;
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
  conversation?: ConversationContext,
): string {
  const segments = footerSegments(state, connectionState, session, conversation);
  return layout(segments, width).join("\n");
}

interface FooterFacts {
  state: PresentationState | undefined;
  connection: string;
  session?: SessionView;
  conversation?: ConversationContext;
}

/** The shell asks for layout on every render, including terminal-only resizes. */
export class FooterView {
  readonly #facts: () => FooterFacts;
  constructor(facts: () => FooterFacts) { this.#facts = facts; }
  invalidate(): void {}
  render(width: number): string[] {
    const facts = this.#facts();
    if (!facts.state) return [];
    const pad = width > 2 ? " " : "";
    return renderFooter(facts.state, facts.connection, Math.max(1, width - pad.length * 2), facts.session, facts.conversation)
      .split("\n").map((line) => `${pad}${line}${pad}`);
  }
}

/** The footer's segments, in display order. Exported for deterministic tests. */
export function footerSegments(
  state: PresentationState,
  connectionState: string,
  session?: SessionView,
  conversation?: ConversationContext,
): Segment[] {
  const models = modelSegments(state);
  const segments = models.slice(0, 1);
  // No cached selection: snapshots/events are the only approval authority.
  if (state.effectiveApprovalMode != null) {
    const mode = approvalLabel(state.effectiveApprovalMode).toUpperCase();
    const pending = state.pendingApprovalMode == null ? "" :
      ` · next attempt ${approvalLabel(state.pendingApprovalMode).toUpperCase()}`;
    segments.push({ text: (state.effectiveApprovalMode === "full_access" ? role.warning : role.meta)(`approval ${mode}${pending}`), priority: 0, compact: role.meta(`${mode}${state.pendingApprovalMode == null ? "" : `; next ${approvalLabel(state.pendingApprovalMode).toUpperCase()}`}`) });
  }
  segments.push(...models.slice(1));
  if (conversation?.parentConversationId != null) {
    segments.push({ text: role.accent("read-only · Esc parent"), priority: 0 });
  } else if (conversation?.readOnly) {
    segments.push({ text: role.accent("read-only inspection"), priority: 0 });
  }
  if (state.runtimeShutdown) segments.push({ text: role.warning("draining"), priority: 0 });
  if (connectionState && connectionState !== "connected") {
    segments.push({ text: role.warning(connectionState), priority: 0 });
  }
  const context = contextLabel(state);
  if (context) segments.push({ text: role.meta(context), priority: 1 });
  const unavailable = unavailableCapabilities(state).length;
  if (unavailable) segments.push({ text: role.warning(`${unavailable} optional ${unavailable === 1 ? "capability" : "capabilities"} unavailable`), priority: 2 });
  if (typeof session?.name === "string" && session.name.trim()) {
    segments.push({ text: role.meta(`session ${session.name}`), priority: 3 });
  }
  const usage = state.attempt?.lastUsage;
  if (usage) segments.push({ text: role.meta(`↑${compact(usage.input_tokens)} ↓${compact(usage.output_tokens)}`), priority: 4 });
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
    if (lines.length <= (kept.some((segment) => segment.priority > 0) ? 1 : MAX_FOOTER_LINES) && lines.every((line) => plainWidth(line) <= width)) {
      return lines;
    }
    const droppable = kept
      .map((segment, index) => ({ segment, index }))
      .filter((entry) => entry.segment.priority > 0)
      .sort((left, right) => right.segment.priority - left.segment.priority);
    const victim = droppable[0];
    if (victim === undefined) {
      // First use complete compact labels, then explicitly defer model detail
      // if the terminal cannot physically hold every essential identity. Never
      // turn a long model reference into a plausible shorter reference.
      const compact = kept.map((segment) => ({ ...segment, text: segment.compact ?? segment.text }));
      const compactLines = pack(compact, width, separator);
      if (compactLines.length <= MAX_FOOTER_LINES && compactLines.every((line) => plainWidth(line) <= width)) return compactLines;
      const deferred = compact.filter((segment) => !segment.model);
      deferred.push({ text: role.meta("model: /settings"), priority: 1 });
      const bounded: Segment[] = [];
      for (const segment of deferred) {
        if (plainWidth(segment.text) > width) continue;
        if (pack([...bounded, segment], width, separator).length <= MAX_FOOTER_LINES) bounded.push(segment);
      }
      return pack(bounded, width, separator);
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
      ? (state.sessionModel?.effective.contextWindow ?? 0)
      : state.attempt?.model?.primary.contextWindow ?? 0;
  if (window <= 0) return "";
  if (usage === undefined) {
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
  if (state.sessionModel === null) return role.meta("rustX · historical inspection · live model unavailable");
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
