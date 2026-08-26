/**
 * The semantic transcript grammar.
 *
 * The transcript is not a log of protocol records. Each canonical role and
 * content type has its own presentation, and the assistant's answer is the
 * primary content on the screen rather than one labelled row among many:
 *
 * ```text
 * user            the question on its own background band
 * assistant text  ordinary Markdown, no banner
 * reasoning       dimmed italic, or one `Thinking...` marker when hidden
 * refusal         explicitly a refusal, never an answer
 * tool_call       one correlated tool card on a state-coloured band
 *                 (see ./tool-card.ts)
 * tool result     folded into that card when folding preserves canonical
 *                 order, otherwise that card's continuation at its own
 *                 canonical position — never repeated, never reordered
 * system          dimmed diagnostic with its authority
 * ```
 *
 * A block may name a *background band* — the visual grammar Pi uses, and the
 * one this client now follows. The band is a layout fact, not a text
 * decoration: a block says which band it belongs to and the app shell fills
 * the terminal width with it, because the shell is the only layer that knows
 * how wide the line is. A band never carries meaning of its own that the
 * text does not also state: a denied tool call says `denied`, and the band
 * colour is a second reading of the same runtime fact, never the only one.
 *
 * Canonical block order is preserved exactly. When a model emits
 * `reasoning, text, tool_call, text`, that is what the reader sees, in that
 * sequence, streaming and committed alike — and when the result of that
 * `tool_call` commits, it appears *after* the trailing text, because that is
 * where the canonical conversation puts it.
 *
 * Two things this file must never do: invent content the provider did not
 * publish (an absent reasoning body stays absent), and let a presentation
 * preference change a semantic fact (hiding reasoning hides *rendering*, it
 * does not turn reasoning into an answer or alter what rustX requested).
 */

import type { DefaultTextStyle } from "@earendil-works/pi-tui";

import type {
  PresentationState,
  StreamingMessage,
  TranscriptCommitted,
  TranscriptEntry,
  TranscriptInteractionRequested,
  TranscriptInteractionSettled,
  TranscriptPublicationAudit,
} from "../../presentation/state.ts";
import type { RuntimeClientOutcome } from "../../protocol/types.ts";
import {
  type ToolCorrelation,
  correlateTools,
  isFoldedToolResult,
  isSplitToolCall,
} from "../../presentation/tools.ts";
import {
  type PresentationPreferences,
  isToolCallExpanded,
} from "../preferences.ts";
import { type BackgroundRole, role, style } from "../theme.ts";
import {
  type ToolCardPart,
  cardBackground,
  renderToolCard,
} from "./tool-card.ts";

/**
 * One rendered transcript block.
 *
 * `markdown` blocks are laid out by Pi's Markdown renderer; `text` blocks are
 * shown verbatim. Tool output and user input are verbatim on purpose — a
 * shell transcript that gets Markdown-formatted is a shell transcript that
 * lies about its own bytes.
 */
export type TranscriptBlock =
  | {
      kind: "markdown";
      key: string;
      markdown: string;
      defaultTextStyle?: DefaultTextStyle;
      /** The background band this block is drawn on, when it has one. */
      background?: BackgroundRole;
    }
  | {
      kind: "text";
      key: string;
      text: string;
      /** The background band this block is drawn on, when it has one. */
      background?: BackgroundRole;
    };

/** Everything the transcript renderer needs beyond the projection itself. */
export interface TranscriptContext {
  preferences: PresentationPreferences;
  correlation: ToolCorrelation;
}

/** Builds the render context for one state. */
export function transcriptContext(
  state: PresentationState,
  preferences: PresentationPreferences,
): TranscriptContext {
  return { preferences, correlation: correlateTools(state) };
}

/** The whole transcript, including durable messages and historical audits. */
export function renderTranscript(
  state: PresentationState,
  preferences: PresentationPreferences,
  correlation?: ToolCorrelation,
): TranscriptBlock[] {
  const context =
    correlation === undefined
      ? transcriptContext(state, preferences)
      : { preferences, correlation };
  const blocks: TranscriptBlock[] = [];
  for (const entry of state.transcript) {
    blocks.push(...renderEntryBlocks(entry, context));
  }
  const attemptOutcome = renderAttemptOutcome(state);
  if (attemptOutcome !== undefined) {
    blocks.push(attemptOutcome);
  }
  return blocks;
}

/**
 * Renders a terminal non-success attempt outcome beside the interrupted
 * conversation. The outcome is already authoritative in the projection;
 * missing assistant text, EOF, detach, or restart never becomes a failure
 * here.
 */
export function renderAttemptOutcome(
  state: PresentationState,
): TranscriptBlock | undefined {
  const phase = state.attempt?.phase;
  if (phase?.type !== "settled" || phase.outcome.type === "completed") {
    return undefined;
  }

  const { heading, detail, colour } = describeOutcome(phase.outcome);
  const lines = [colour(`▌ ${heading}`)];
  if (detail !== undefined && detail.length > 0) {
    lines.push(...bar(bound(detail), colour));
  }
  return {
    kind: "text",
    key: `attempt:${state.attempt!.attemptId}:outcome`,
    text: lines.join("\n"),
  };
}

function describeOutcome(
  outcome: Exclude<RuntimeClientOutcome, { type: "completed" }>,
): {
  heading: string;
  detail?: string;
  colour: (text: string) => string;
} {
  switch (outcome.type) {
    case "cancelled":
      return {
        heading: `cancelled · ${outcome.reason}`,
        colour: role.warning,
      };
    case "timed_out":
      return { heading: "timed out", colour: role.warning };
    case "limit_exceeded":
      return {
        heading: `limit exceeded · ${outcome.limit}`,
        colour: role.warning,
      };
    case "failed":
      if (outcome.error.type === "model") {
        return {
          heading: `request failed · ${outcome.error.kind}`,
          detail: outcome.error.message,
          colour: role.error,
        };
      }
      return {
        heading: `runtime failed · ${outcome.error.error.type}`,
        detail: outcome.error.error.message ?? outcome.error.error.name,
        colour: role.error,
      };
    default:
      return { heading: "request settled", colour: role.meta };
  }
}

const OUTCOME_DETAIL_LIMIT = 512;

function bound(text: string): string {
  if (text.length <= OUTCOME_DETAIL_LIMIT) {
    return text;
  }
  return `${text.slice(0, OUTCOME_DETAIL_LIMIT - 1)}…`;
}

/** The independently styled blocks of one transcript entry. */
export function renderEntryBlocks(
  entry: TranscriptEntry,
  context: TranscriptContext,
): TranscriptBlock[] {
  switch (entry.kind) {
    case "streaming":
      return renderStreaming(entry, context);
    case "committed":
      return renderCommitted(entry, context);
    case "publication_audit":
      return renderPublicationAudit(entry);
    case "interaction_requested":
      return renderInteractionRequested(entry);
    case "interaction_settled":
      return renderInteractionSettled(entry);
  }
}

/** Renders noncanonical publication output without creating Tool Plane cards. */
function renderPublicationAudit(
  entry: TranscriptPublicationAudit,
): TranscriptBlock[] {
  const audit = entry.audit;
  const heading =
    audit.kind === "incomplete" ? "incomplete" : "unaccepted";
  const blocks: TranscriptBlock[] = [
    {
      kind: "text",
      key: `${entry.key}:heading`,
      text: role.warning(`▌ assistant output · ${heading} · noncanonical`),
    },
  ];
  for (const [index, block] of audit.content.entries()) {
    const key = `${entry.key}:${index}`;
    switch (block.kind) {
      case "text":
        blocks.push({ kind: "markdown", key, markdown: block.text });
        break;
      case "reasoning":
        blocks.push({
          kind: "text",
          key,
          text: role.reasoning(block.text),
        });
        break;
      case "refusal":
        blocks.push(refusalBlock(key, block.text));
        break;
      case "proposed_tool_call":
        blocks.push({
          kind: "text",
          key,
          text: [
            role.warning(
              `◇ proposed tool call · ${block.name} · not accepted · not executed`,
            ),
            ...bar(block.arguments, style.dim),
          ].join("\n"),
        });
        break;
    }
  }
  return blocks;
}

/** Renders durable interaction evidence as historical, non-actionable text. */
function renderInteractionRequested(
  entry: TranscriptInteractionRequested,
): TranscriptBlock[] {
  const lines = [role.meta("▌ historical interaction · requested · not actionable")];
  if (entry.subject.type === "approval") {
    lines.push(
      role.warning(`approval · ${entry.subject.tool_name} · proposed call ${entry.subject.call_id}`),
      ...bar(entry.subject.reason, style.dim),
      ...bar(`arguments digest: ${entry.subject.arguments_digest}`, style.dim),
    );
  } else {
    lines.push(role.warning("questionnaire"));
    for (const question of entry.subject.questionnaire.questions) {
      lines.push(...bar(`${question.header}: ${question.question}`, style.dim));
      lines.push(
        ...bar(
          `options: ${question.options.map((option) => option.label).join(", ")}`,
          style.dim,
        ),
      );
    }
  }
  return [{ kind: "text", key: entry.key, text: lines.join("\n") }];
}

/** Renders every settled interaction value without making it actionable. */
function renderInteractionSettled(
  entry: TranscriptInteractionSettled,
): TranscriptBlock[] {
  const lines = [
    role.meta(
      `▌ historical interaction · settled · ${settlementLabel(entry.settlement)}`,
    ),
  ];
  if (entry.settlement.type === "denied" || entry.settlement.type === "cancelled") {
    lines.push(...bar(entry.settlement.reason, style.dim));
  } else if (entry.settlement.type === "questionnaire_submitted") {
    lines.push(
      ...bar(
        entry.settlement.submission.answers
          .map((answer) => `${answer.question_index}: ${answer.answer.type}`)
          .join(", "),
        style.dim,
      ),
    );
  }
  return [{ kind: "text", key: entry.key, text: lines.join("\n") }];
}

function settlementLabel(
  settlement: TranscriptInteractionSettled["settlement"],
): string {
  switch (settlement.type) {
    case "approved":
      return "approved";
    case "denied":
      return "denied";
    case "questionnaire_submitted":
      return "questionnaire submitted";
    case "questionnaire_declined":
      return "questionnaire declined";
    case "cancelled":
      return "cancelled";
  }
}

// ---------------------------------------------------------------------------
// Committed history
// ---------------------------------------------------------------------------

function renderCommitted(
  entry: TranscriptCommitted,
  context: TranscriptContext,
): TranscriptBlock[] {
  const message = entry.message;
  switch (message.role) {
    case "user": {
      // Provenance is metadata, never a different role: a runtime-originated
      // inbound message is labelled, not disguised as a human turn.
      const labels: string[] = [];
      if (message.source !== "human") {
        labels.push(sourceLabel(message.source));
      }
      if (message.kind === "compaction_summary") {
        labels.push("compaction summary");
      }
      const body = message.content
        .map((block) => (block.type === "text" ? block.text : `(${block.type})`))
        .join("\n");
      const blocks: TranscriptBlock[] = [];
      if (labels.length > 0) {
        // Provenance stays outside the band: it is what the runtime says
        // about the turn, not part of what the turn said.
        blocks.push({
          kind: "text",
          key: `${entry.key}:provenance`,
          text: role.meta(labels.join(" · ")),
        });
      }
      blocks.push({
        kind: "markdown",
        key: entry.key,
        markdown: body,
        defaultTextStyle: { color: role.user },
        background: "user",
      });
      return blocks;
    }

    case "assistant": {
      const blocks: TranscriptBlock[] = [];
      let reasoningRun: string[] = [];
      let runIndex = 0;

      const flushReasoning = () => {
        if (reasoningRun.length === 0) {
          return;
        }
        blocks.push(
          reasoningBlock(
            `${entry.key}:reasoning:${runIndex}`,
            reasoningRun,
            context,
          ),
        );
        reasoningRun = [];
        runIndex += 1;
      };

      message.content.forEach((block, index) => {
        const key = `${entry.key}:${index}`;
        if (block.type === "reasoning") {
          // Consecutive reasoning blocks are grouped visually. Grouping never
          // reorders and never merges across a non-reasoning block.
          reasoningRun.push(
            block.text ?? role.meta("(the provider exposed no reasoning text)"),
          );
          return;
        }
        flushReasoning();
        switch (block.type) {
          case "text":
            blocks.push({ kind: "markdown", key, markdown: block.text });
            break;
          case "refusal":
            blocks.push(refusalBlock(key, block.text));
            break;
          case "tool_call":
            blocks.push(toolBlock(key, block.id, context, anchorPart(context, block.id)));
            break;
          case "image":
            blocks.push({ kind: "text", key, text: role.meta("(image)") });
            break;
          default:
            break;
        }
      });
      flushReasoning();
      return blocks;
    }

    case "tool": {
      // Three cases, all decided by the correlation's fold invariant:
      //
      //   folded          the call's card already shows this result
      //   split           the call's card is above; this is its continuation
      //   no anchor       the assistant message was never committed, so this
      //                   result is the only place the call is visible
      //
      // Rendering a folded result again is the duplication Issue #79 removes;
      // folding an unfoldable one would move it across canonical content.
      const callId = message.tool_call_id;
      if (isFoldedToolResult(context.correlation, callId)) {
        return [];
      }
      const part: ToolCardPart = context.correlation.anchoredCalls.has(callId)
        ? "continuation"
        : "full";
      return [toolBlock(entry.key, callId, context, part)];
    }

    default:
      return [];
  }
}

// ---------------------------------------------------------------------------
// The streaming assistant message
// ---------------------------------------------------------------------------

function renderStreaming(
  entry: StreamingMessage,
  context: TranscriptContext,
): TranscriptBlock[] {
  const blocks: TranscriptBlock[] = [];
  let reasoningRun: string[] = [];
  let runIndex = 0;

  const flushReasoning = () => {
    if (reasoningRun.length === 0) {
      return;
    }
    blocks.push(
      reasoningBlock(`${entry.key}:reasoning:${runIndex}`, reasoningRun, context),
    );
    reasoningRun = [];
    runIndex += 1;
  };

  for (const block of entry.blocks) {
    const key = `${entry.key}:${block.blockIndex}`;
    if (block.kind === "reasoning") {
      reasoningRun.push(block.text);
      continue;
    }
    flushReasoning();
    switch (block.kind) {
      case "text":
        // Streaming and committed assistant text render identically, so the
        // screen does not reflow when the message commits.
        blocks.push({ kind: "markdown", key, markdown: block.text });
        break;
      case "refusal":
        blocks.push(refusalBlock(key, block.text));
        break;
      case "tool_call":
        blocks.push(
          toolBlock(key, block.callId, context, anchorPart(context, block.callId)),
        );
        break;
      default:
        break;
    }
  }
  flushReasoning();
  return blocks;
}

// ---------------------------------------------------------------------------
// Semantic block components
// ---------------------------------------------------------------------------

/**
 * Reasoning, shown or summarized.
 *
 * Hidden reasoning collapses to one marker for the whole run — never to
 * nothing (that would hide that the model reasoned at all) and never to
 * assistant text (that would promote it to an answer).
 */
function reasoningBlock(
  key: string,
  texts: string[],
  context: TranscriptContext,
): TranscriptBlock {
  if (!context.preferences.reasoningVisible) {
    return {
      kind: "text",
      key,
      text: style.italic(role.reasoning("Thinking...")),
    };
  }
  return {
    kind: "markdown",
    key,
    markdown: texts.join("\n\n"),
    // Applied as Markdown's default text style so the renderer reapplies it
    // after nested spans reset their own ANSI styling.
    defaultTextStyle: { color: role.reasoning, italic: true },
  };
}

function refusalBlock(key: string, text: string): TranscriptBlock {
  return {
    kind: "text",
    key,
    text: [role.warning("⊘ refusal"), ...bar(text, role.warning)].join("\n"),
  };
}

/**
 * How a call's own `tool_call` anchor is drawn.
 *
 * `call` when a committed result exists that may not fold into it: the result
 * follows below, at its canonical position, as this card's continuation.
 */
function anchorPart(context: TranscriptContext, callId: string): ToolCardPart {
  return isSplitToolCall(context.correlation, callId) ? "call" : "full";
}

function toolBlock(
  key: string,
  callId: string,
  context: TranscriptContext,
  part: ToolCardPart = "full",
): TranscriptBlock {
  const tool = context.correlation.byCallId.get(callId);
  if (tool === undefined) {
    // Unreachable through the correlation, which seeds itself from these very
    // blocks. Kept total rather than throwing inside a render.
    return { kind: "text", key, text: role.meta(`◇ tool call ${callId}`) };
  }
  return {
    kind: "text",
    key,
    text: renderToolCard(
      tool,
      {
        // One expansion state for the whole entity: expanding a split call
        // expands both of its parts, because they are one card.
        expanded: isToolCallExpanded(context.preferences, callId),
        budget: context.preferences.previewBudget,
      },
      part,
    ),
    // The band restates the lifecycle the card's own status words already
    // carry. A `call` part is still in flight by construction — its result
    // is rendered below, at the result's canonical position — so it keeps
    // the pending band whatever the settlement below it says.
    background: cardBackground(part === "call" ? undefined : tool.lifecycle),
  };
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/** Prefixes every line with a coloured bar, keeping the body verbatim. */
function bar(body: string, colour: (text: string) => string): string[] {
  const lines = body.split("\n");
  return lines.map((line) => `${colour("▌")} ${line}`);
}

function sourceLabel(source: unknown): string {
  if (typeof source === "object" && source !== null && "agent" in source) {
    return `agent ${(source as { agent: { agent_id: string } }).agent.agent_id}`;
  }
  return String(source);
}
