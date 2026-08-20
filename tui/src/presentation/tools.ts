/**
 * The presentation join between committed tool facts and live execution.
 *
 * rustX publishes one logical tool call as several *different* facts, on
 * purpose:
 *
 * ```text
 * assistant tool_call block   committed conversation content
 * foreground execution        attempt-scoped execution lifecycle
 * tool result message         committed conversation content
 * ```
 *
 * Their semantic ownership stays separate — this module never merges it. It
 * only answers a presentation question: *which visual entity does each fact
 * belong to?* The answer is always the runtime's own {@link ToolCallId}. No
 * correlation here uses a tool name, argument equality, list position,
 * timing, or textual adjacency, so two concurrent calls of the same tool with
 * identical arguments stay two entities.
 *
 * Lifecycle is never computed here either. It is read from the runtime's
 * foreground projection, or from the presence of a committed canonical
 * result, and those two agree because both come from the same settlement.
 * Nothing in this file infers "running because no result yet" or "succeeded
 * because the output looks fine".
 *
 * ## Where a committed result is drawn
 *
 * A canonical `role: "tool"` message arrives *after* the assistant message
 * that requested it, and rustX's canonical model constrains almost nothing
 * beyond that. `AssistantMessageBlock` holds a plain
 * `Vec<AssistantContentBlock>`, so a `tool_call` need not be the last block
 * of its message; and `StructuralIndex::build` rejects only duplicate calls,
 * duplicate results, and orphan results, so a result message need not
 * immediately follow the message that requested it. Both of these are shapes
 * the projection can hold and this client must render correctly:
 *
 * ```text
 * Assistant: text A, tool_call X, text B      Assistant: tool_call X
 * ToolResult X                                User: U
 *                                             ToolResult X
 * ```
 *
 * In the first, drawing the result inside the `X` card moves it before
 * `text B`. In the second, it moves it before the user's message. Both
 * reorder canonical content, so neither may fold. Hence the invariant:
 *
 * > A committed tool result folds into its call's card only if every
 * > canonical fact it would be moved across belongs to the same foldable
 * > batch. Fold eligibility is therefore a property of the **complete
 * > canonical interval between the call anchor and the result position** —
 * > never of the owning assistant message alone.
 *
 * Concretely, a *batch* is the trailing unbroken run of `tool_call` blocks of
 * one assistant entry, and it folds only when both hold:
 *
 * ```text
 * inside the message   nothing but tool_calls follows the batch's first call
 * across the interval  every entry from the anchor through the batch's last
 *                      committed result is a result of that same batch
 * ```
 *
 * The second condition is what a per-message rule cannot express, and it is
 * checked against the ordered `PresentationState.transcript` — the same
 * authoritative projection everything else here reads. A `User`, `System`, or
 * unrelated `Assistant` message anywhere in that interval unfolds the batch.
 *
 * The decision is per *batch*, all or nothing. Folding only the calls whose
 * results happen to be adjacent would move some results across their own
 * siblings, so a batch that cannot fold coherently splits coherently: every
 * call in it is drawn in two parts of one entity — the call at its
 * `tool_call` block, and its terminal continuation at the canonical result
 * message. That is one identity in canonical order, not the pre-#79
 * duplication of a raw call block, a separate running card, and a separate
 * result block.
 *
 * All of it is derived, every time, from the current transcript. There is no
 * `alreadyFolded` set, no memory of the last render, and no incremental batch
 * state, so a fresh authoritative snapshot reaches the same decision as a
 * state folded event by event.
 */

import type {
  ForegroundToolExecution,
  ToolCallId,
  ToolExecutionResult,
  ToolId,
  ToolProgress,
} from "../protocol/types.ts";
import type { PresentationState, TranscriptEntry } from "./state.ts";

/**
 * The runtime-owned lifecycle of one correlated tool entity.
 *
 * Deliberately the same three states the Runtime Client publishes. There is
 * no client-invented fourth state and no "unknown" fallback that a renderer
 * could mistake for a settlement.
 */
export type ToolLifecycle =
  | { type: "assembled" }
  | { type: "running"; progress?: ToolProgress }
  | { type: "settled"; result: ToolExecutionResult };

/** One logical tool call, as one visual entity. */
export interface CorrelatedTool {
  /** The stable runtime identity this entity is keyed by. */
  callId: ToolCallId;
  toolId: ToolId;
  /** The model-facing tool name at call time, as the runtime published it. */
  name: string;
  /**
   * The arguments exactly as published: assembled canonical JSON once the
   * call is complete, or the accumulated fragment prefix while it streams.
   */
  argumentsText: string;
  lifecycle: ToolLifecycle;
  /**
   * Whether the owning assistant message is committed canonical history.
   *
   * Presentation only: a settled entity in an uncommitted stream is still a
   * settled execution.
   */
  committed: boolean;
}

/** The correlated tool view of one presentation state. */
export interface ToolCorrelation {
  /** Every known logical call, by its runtime identity. */
  byCallId: ReadonlyMap<ToolCallId, CorrelatedTool>;
  /**
   * Calls whose assistant `tool_call` block is in the transcript.
   *
   * Their card renders at that block. Whether the committed result renders
   * there too, or as a continuation at its own canonical position, is
   * {@link foldedResults}.
   */
  anchoredCalls: ReadonlySet<ToolCallId>;
  /** Calls with a committed canonical result message in the transcript. */
  anchoredResults: ReadonlySet<ToolCallId>;
  /**
   * Calls whose committed result is drawn *inside* the call's own card.
   *
   * A subset of {@link anchoredCalls}: exactly the calls of a batch that
   * folds — one whose trailing `tool_call` run is unbroken *and* whose
   * canonical interval from anchor to last result crosses nothing else. A
   * committed result outside this set renders as the terminal continuation
   * of its card, at its own canonical position.
   */
  foldedResults: ReadonlySet<ToolCallId>;
  /**
   * Executions with no transcript anchor at all.
   *
   * Normally empty: a foreground execution exists because the assistant asked
   * for it, and that request is a transcript block. It becomes non-empty only
   * when the owning stream was dropped — an attempt that settled without
   * committing — and those executions still deserve to be visible rather than
   * silently disappearing.
   */
  orphans: CorrelatedTool[];
}

/** Lifecycle ordering, so a settled entity never regresses to running. */
const LIFECYCLE_RANK = { assembled: 0, running: 1, settled: 2 } as const;

/**
 * Correlates every tool fact in the state into one entity per call id.
 *
 * Pure and total: given the same state it returns the same correlation, and
 * a state rebuilt from a fresh snapshot correlates identically to one folded
 * incrementally.
 */
export function correlateTools(state: PresentationState): ToolCorrelation {
  const byCallId = new Map<ToolCallId, CorrelatedTool>();
  const anchoredCalls = new Set<ToolCallId>();
  const anchoredResults = new Set<ToolCallId>();
  const batches: CallBatch[] = [];

  // 1. Conversation content: the assistant's own call blocks, streaming or
  //    committed, in canonical order. Each entry's transcript position is
  //    kept, because fold eligibility is a question about positions.
  state.transcript.forEach((entry, index) => {
    for (const call of transcriptCalls(entry)) {
      anchoredCalls.add(call.callId);
      byCallId.set(call.callId, call);
    }
    const batch = batchOf(entry, index);
    if (batch !== undefined) {
      batches.push(batch);
    }
  });

  // 2. Conversation content: committed canonical results, and where each one
  //    sits in canonical order.
  const resultPositions = new Map<ToolCallId, number>();
  state.transcript.forEach((entry, index) => {
    if (entry.kind !== "committed" || entry.message.role !== "tool") {
      return;
    }
    const message = entry.message;
    anchoredResults.add(message.tool_call_id);
    resultPositions.set(message.tool_call_id, index);
    const existing = byCallId.get(message.tool_call_id);
    byCallId.set(message.tool_call_id, {
      callId: message.tool_call_id,
      toolId: existing?.toolId ?? message.tool_id,
      name: existing?.name ?? "",
      argumentsText: existing?.argumentsText ?? "",
      lifecycle: { type: "settled", result: message.result },
      committed: true,
    });
  });

  // 3. Execution lifecycle: the attempt's own foreground projection. It is
  //    the authority on assembled/running/settled and on progress.
  for (const execution of state.attempt?.foreground ?? []) {
    const existing = byCallId.get(execution.call_id);
    const lifecycle = foregroundLifecycle(execution);
    byCallId.set(execution.call_id, {
      callId: execution.call_id,
      toolId: execution.tool_id,
      // A published name always wins over an empty one, whichever fact
      // carried it first.
      name: existing?.name || execution.name,
      argumentsText: existing?.argumentsText || argumentsOf(execution),
      lifecycle: laterOf(existing?.lifecycle, lifecycle),
      committed: existing?.committed ?? false,
    });
  }

  const orphans = [...byCallId.values()].filter(
    (tool) =>
      !anchoredCalls.has(tool.callId) && !anchoredResults.has(tool.callId),
  );
  // 4. The fold plan, derived from the whole ordered transcript rather than
  //    from any one message's tail.
  const foldedResults = new Set<ToolCallId>();
  for (const batch of batches) {
    if (!foldsInPlace(batch, resultPositions, state.transcript)) {
      continue;
    }
    for (const callId of batch.callIds) {
      if (anchoredCalls.has(callId) && anchoredResults.has(callId)) {
        foldedResults.add(callId);
      }
    }
  }
  return { byCallId, anchoredCalls, anchoredResults, foldedResults, orphans };
}

/** The correlated entity of one call id, when the state knows of it. */
export function correlatedTool(
  correlation: ToolCorrelation,
  callId: ToolCallId,
): CorrelatedTool | undefined {
  return correlation.byCallId.get(callId);
}

/**
 * Whether a committed tool result message is already shown by its call's card.
 *
 * True only when folding preserves canonical order across the whole interval
 * between call and result — see the fold invariant at the top of this file.
 * Rendering a folded result again as its own record would duplicate one
 * runtime fact in two places; rendering an *unfoldable* one inside the call
 * card would reorder canonical content instead.
 */
export function isFoldedToolResult(
  correlation: ToolCorrelation,
  callId: ToolCallId,
): boolean {
  return correlation.foldedResults.has(callId);
}

/**
 * Whether the card at a call's transcript anchor draws the call alone.
 *
 * That happens when the call has a committed result the anchor may not fold:
 * the result is drawn as a continuation further down, at its own canonical
 * position.
 */
export function isSplitToolCall(
  correlation: ToolCorrelation,
  callId: ToolCallId,
): boolean {
  return (
    correlation.anchoredResults.has(callId) &&
    !correlation.foldedResults.has(callId)
  );
}

/** The tools the runtime currently reports as running. */
export function runningTools(correlation: ToolCorrelation): CorrelatedTool[] {
  return [...correlation.byCallId.values()].filter(
    (tool) => tool.lifecycle.type === "running",
  );
}

// ---------------------------------------------------------------------------
// Internals
// ---------------------------------------------------------------------------

/**
 * One assistant entry's trailing run of `tool_call` blocks, and where it sits.
 *
 * The batch is the unit of the fold decision. It is the message's *trailing*
 * run because a call followed by more content in the same message can never
 * fold: its result would move before that content.
 */
interface CallBatch {
  /** The transcript position of the assistant entry that issued the calls. */
  anchor: number;
  /** The calls of the batch, in canonical block order. */
  callIds: ToolCallId[];
}

/**
 * The foldable-shaped batch of one assistant entry, when it has one.
 *
 * `undefined` when the entry issues no calls, or when non-`tool_call` content
 * follows its first call — the intra-message half of the invariant. The
 * decision is per *message*, never per call: a mixed message that folded only
 * its trailing calls would reorder the remaining results against the folded
 * ones.
 */
function batchOf(entry: TranscriptEntry, anchor: number): CallBatch | undefined {
  const blocks = callIdByBlock(entry);
  const first = blocks.findIndex((callId) => callId !== undefined);
  if (first < 0) {
    return undefined;
  }
  // Everything from the first call onward must itself be a call. When it is,
  // that tail *is* the batch, because no call can precede `first`.
  const tail = blocks.slice(first);
  return tail.every((callId) => callId !== undefined)
    ? { anchor, callIds: tail as ToolCallId[] }
    : undefined;
}

/**
 * Whether one batch's committed results may be drawn at the call anchor.
 *
 * The interval half of the invariant, and the part a per-message rule cannot
 * express. Folding moves every committed result of the batch up to the anchor,
 * so it crosses every canonical entry in between. That is safe exactly when
 * each of those entries is itself a result of this same batch — a `User`,
 * `System`, or unrelated `Assistant` message in the interval, or a result
 * belonging to some other call, means folding would reorder canonical content.
 *
 * Nothing here consults timing, adjacency heuristics, or provider behaviour:
 * the interval is read straight off the ordered transcript.
 */
function foldsInPlace(
  batch: CallBatch,
  resultPositions: ReadonlyMap<ToolCallId, number>,
  transcript: readonly TranscriptEntry[],
): boolean {
  const positions = batch.callIds
    .map((callId) => resultPositions.get(callId))
    .filter((position): position is number => position !== undefined);
  if (positions.length === 0) {
    // Nothing committed yet, so nothing to move. A card with no committed
    // result renders whole at its anchor either way.
    return false;
  }
  const last = Math.max(...positions);
  const members = new Set(batch.callIds);
  for (let index = batch.anchor + 1; index <= last; index += 1) {
    const entry = transcript[index];
    if (
      entry?.kind === "committed" &&
      entry.message.role === "tool" &&
      members.has(entry.message.tool_call_id)
    ) {
      continue;
    }
    return false;
  }
  return true;
}

/** One entry's blocks as their call id, or `undefined`, in canonical order. */
function callIdByBlock(entry: TranscriptEntry): Array<ToolCallId | undefined> {
  if (entry.kind === "streaming") {
    return entry.blocks.map((block) =>
      block.kind === "tool_call" ? block.callId : undefined,
    );
  }
  if (entry.message.role !== "assistant") {
    return [];
  }
  return entry.message.content.map((block) =>
    block.type === "tool_call" ? block.id : undefined,
  );
}

function transcriptCalls(entry: TranscriptEntry): CorrelatedTool[] {
  if (entry.kind === "streaming") {
    return entry.blocks
      .filter((block) => block.kind === "tool_call")
      .map((block) => ({
        callId: block.callId,
        toolId: block.toolId,
        name: block.name,
        argumentsText: block.argumentsText,
        lifecycle: { type: "assembled" } as ToolLifecycle,
        committed: false,
      }));
  }
  if (entry.message.role !== "assistant") {
    return [];
  }
  return entry.message.content
    .filter((block) => block.type === "tool_call")
    .map((block) => ({
      callId: block.id,
      toolId: block.tool_id,
      name: block.name,
      argumentsText: stringifyArguments(block.arguments),
      lifecycle: { type: "assembled" } as ToolLifecycle,
      committed: true,
    }));
}

function foregroundLifecycle(execution: ForegroundToolExecution): ToolLifecycle {
  switch (execution.state.type) {
    case "running":
      return { type: "running", progress: execution.state.progress };
    case "settled":
      return { type: "settled", result: execution.state.result };
    default:
      return { type: "assembled" };
  }
}

function argumentsOf(execution: ForegroundToolExecution): string {
  return execution.state.arguments;
}

/**
 * The further-advanced of two lifecycles.
 *
 * On a tie the *existing* one wins, which means a committed canonical result
 * is preferred over the attempt's foreground copy of the same settlement.
 * They describe the same fact; the committed one is the durable one.
 */
function laterOf(
  existing: ToolLifecycle | undefined,
  incoming: ToolLifecycle,
): ToolLifecycle {
  if (existing === undefined) {
    return incoming;
  }
  return LIFECYCLE_RANK[incoming.type] > LIFECYCLE_RANK[existing.type]
    ? incoming
    : existing;
}

/**
 * Canonical arguments as text.
 *
 * The committed block carries parsed JSON while the streaming block carries
 * the raw fragment text; both are normalized to text so one card renders the
 * same call the same way before and after the commit.
 */
function stringifyArguments(value: unknown): string {
  try {
    return JSON.stringify(value) ?? String(value);
  } catch {
    return String(value);
  }
}
