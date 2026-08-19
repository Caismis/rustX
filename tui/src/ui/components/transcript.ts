/**
 * The semantic transcript grammar.
 *
 * The transcript is not a log of protocol records. Each canonical role and
 * content type has its own presentation, and the assistant's answer is the
 * primary content on the screen rather than one labelled row among many:
 *
 * ```text
 * user            ▌ the question, verbatim
 * assistant text  ordinary Markdown, no banner
 * reasoning       dimmed, or one `Thinking…` marker when hidden
 * refusal         explicitly a refusal, never an answer
 * tool_call       one correlated tool card (see ./tool-card.ts)
 * tool result     folded into that card, not repeated
 * system          dimmed diagnostic with its authority
 * ```
 *
 * Canonical block order is preserved exactly. When a model emits
 * `reasoning, text, tool_call, text`, that is what the reader sees, in that
 * sequence, streaming and committed alike.
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
} from "../../presentation/state.ts";
import {
  type ToolCorrelation,
  correlateTools,
  isFoldedToolResult,
} from "../../presentation/tools.ts";
import { type PresentationPreferences, isExpanded } from "../preferences.ts";
import { role, style } from "../theme.ts";
import { renderToolCard } from "./tool-card.ts";

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
    }
  | { kind: "text"; key: string; text: string };

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

/** The whole transcript, including the client's unacknowledged echoes. */
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
  for (const pending of state.pendingSubmissions) {
    // Explicitly marked unacknowledged so it can never read as canonical
    // history. The runtime's own inbound fact replaces it.
    blocks.push({
      kind: "text",
      key: `pending:${pending.key}`,
      text: [
        role.meta("▌ awaiting runtime acknowledgement"),
        ...bar(pending.text, style.dim),
      ].join("\n"),
    });
  }
  return blocks;
}

/** The independently styled blocks of one transcript entry. */
export function renderEntryBlocks(
  entry: TranscriptEntry,
  context: TranscriptContext,
): TranscriptBlock[] {
  return entry.kind === "streaming"
    ? renderStreaming(entry, context)
    : renderCommitted(entry, context);
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
      const lines =
        labels.length === 0
          ? []
          : [role.meta(`▌ ${labels.join(" · ")}`)];
      return [
        {
          kind: "text",
          key: entry.key,
          text: [...lines, ...bar(body, role.user)].join("\n"),
        },
      ];
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
            blocks.push(toolBlock(key, block.id, context));
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

    case "tool":
      // The canonical result is presented inside its call's card. Rendering it
      // again here is exactly the duplication Issue #79 removes. A result
      // whose call has *no* transcript anchor — the assistant message was
      // never committed — still has to be visible, so it renders its own card
      // in place.
      return isFoldedToolResult(context.correlation, message.tool_call_id)
        ? []
        : [toolBlock(entry.key, message.tool_call_id, context)];

    case "system":
      return [
        {
          kind: "text",
          key: entry.key,
          text: [
            role.meta(`▌ system · ${message.authority}`),
            ...bar(
              message.content.map((block) => block.text).join("\n"),
              role.meta,
            ),
          ].join("\n"),
        },
      ];

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
        blocks.push(toolBlock(key, block.callId, context));
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
    return { kind: "text", key, text: role.reasoning("✻ Thinking…") };
  }
  return {
    kind: "markdown",
    key,
    markdown: texts.join("\n\n"),
    // Applied as Markdown's default text style so the renderer reapplies it
    // after nested spans reset their own ANSI styling.
    defaultTextStyle: { color: role.reasoning },
  };
}

function refusalBlock(key: string, text: string): TranscriptBlock {
  return {
    kind: "text",
    key,
    text: [role.warning("⊘ refusal"), ...bar(text, role.warning)].join("\n"),
  };
}

function toolBlock(
  key: string,
  callId: string,
  context: TranscriptContext,
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
    text: renderToolCard(tool, {
      expanded: isExpanded(context.preferences, callId),
      previewLines: context.preferences.previewLines,
    }),
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
