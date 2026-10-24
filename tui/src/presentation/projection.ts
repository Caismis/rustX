import { attemptOwnerKey } from '../../../protocol/app-server/attempt-identity.ts';
import { messageDescriptors } from './message-descriptors.ts';
import { contentRefKey } from '../../../protocol/app-server/content-identity.ts';
import { displayInteraction } from './interaction-display.ts';
import type { ContentBinding } from '../../../protocol/app-server/v57.ts';
import { foldRuntimeEvent, checkpointSnapshot, contentBindings, observationCheckpoint, refineToolSlots, statusIdentity, transitionFacts } from '../../../protocol/app-server/projection.ts';
/**
 * The presentation reducer.
 *
 * Two pure entry points define the whole state model:
 *
 * ```text
 * replaceFromSnapshot(snapshot, cursor)   -> state      (authoritative repair)
 * reduce(state, protocolEvent)            -> nextState  (incremental fold)
 * ```
 *
 * Both are total functions over runtime facts. There is no third path that
 * invents state, and no event is ever synthesized locally to "fill a gap":
 * when the incremental stream can no longer be trusted, the caller replaces
 * from a fresh snapshot rather than replaying what the UI thinks happened.
 *
 * # Why this is not a second runtime state machine
 *
 * Every field is copied from a runtime-published value. The reducer decides
 * *where a fact goes in the render tree*, never *what the fact is*: it starts
 * no execution, admits nothing, drains nothing, settles nothing, and can
 * produce no state the runtime did not publish. Discard it at any moment and
 * one bounded checkpoint rebuilds it exactly.
 *
 * Identity is always runtime-provided — `attempt_id`, `message_id`,
 * `block_index`, `tool_call_id`, `execution_id`. Nothing is identified by its
 * position on screen.
 */

import type {
  AgentStatusView,
  RuntimeClientCursor,
  RuntimeClientEvent,
  RuntimeClientSnapshot,
  RuntimeClientTranscriptEntry,
  RuntimeClientTranscriptPage,
  SessionModelView,
} from "../protocol/app-server.ts";
import { EXACT_ZERO, compareExact } from "../protocol/app-server.ts";
import {
  isHiddenContextMessage,
  validateTranscriptCursorContract,
} from "./wire-contract.ts";

/**
 * One routed observation, exactly as `session/event` carries it.
 *
 * The attachment target is not part of the fold: routing and stale fencing
 * happen above this layer, so the reducer only ever sees an observation that
 * already belongs to this projection.
 */
export interface ProjectedEvent {
  cursor: RuntimeClientCursor;
  event: RuntimeClientEvent;
}
import { parseSnapshot } from "./todos.ts";
import type {
  PresentationState,
  StreamingBlock,
  StreamingMessage,
  TranscriptEntry,
} from "./state.ts";

/** The initial state of a client that has not attached yet. */
export function emptyPresentationState(
  sessionModel: SessionModelView | null,
): PresentationState {
  return {
    conversationId: "",
    workflows: { revision: EXACT_ZERO, runs: [], omitted_runs: 0 },
    cursor: EXACT_ZERO,
    transcript: [],
    inbound: { pending: [], last_drain: undefined },
    pendingInteractions: [],
    messageDescriptors: [],
    deferredTranscriptEntries: [],
    publicationSettlements: [],
    jobs: [],
    agents: [],
    statuses: [],
    context: { compaction_in_progress: false, compaction_count: 0 },
    capabilities: { configured_tools: [], revision: EXACT_ZERO, tools: [], skills: [] },
    resources: emptyResources(),
    sessionModel,
    modelStatus: null,
    effectivePlugins: null,
    settingsEvidence: "live_session",
    todos: undefined,
    goal: null,
    runtimeShutdown: false,
    durabilityFailure: null,
    adoptionEligibility: "unavailable",
    effectiveApprovalMode: "policy",
  };
}

/** The resource projection of a runtime that has published none yet. */
function emptyResources(): PresentationState["resources"] {
  return {
    inspection: {
      definitions: [], resource_diagnostics: [],
      main: null,
      agents: {},
      workflows: {},
      sources: {},
      skills: [],
      skill_diagnostics: [],
    },
    revision: EXACT_ZERO,
    context_files: [],
    agent_profile: false,
  };
}

/**
 * Replaces the whole projection from an authoritative snapshot.
 *
 * This is the only repair path. It derives the complete semantic projection
 * from the authoritative snapshot; there is no client-owned submission row
 * to preserve. TUI inspection and transient feedback surfaces live outside
 * this runtime-derived projection, so an authoritative repair cannot
 * reconstruct or carry them accidentally.
 */
const historyBindings = new WeakMap<PresentationState,ContentBinding[]>();
const nativeSnapshots = new WeakMap<PresentationState, RuntimeClientSnapshot>();
export function replaceFromSnapshot(snapshot: RuntimeClientSnapshot, cursor: RuntimeClientCursor): PresentationState {
  const state = presentSnapshot(snapshot, cursor);
  nativeSnapshots.set(state, snapshot);
  return state;
}
export function presentationCheckpoint(state: PresentationState) {
  const snapshot = nativeSnapshots.get(state);
  return snapshot ? observationCheckpoint(snapshot, state.cursor) : undefined;
}
export function deferredContent(state: PresentationState) {
  const snapshot = nativeSnapshots.get(state);
  const bindings=new Map((historyBindings.get(state) ?? []).map(binding=>[contentRefKey(binding.content_ref),binding]));
  for(const binding of snapshot ? contentBindings(snapshot) : [])bindings.set(contentRefKey(binding.content_ref),binding);
  for(const entry of state.transcript) {
    if(entry.kind!=='committed'&&entry.kind!=='attempt_terminal')continue;
    const process=entry.kind==='committed'?entry.turnProcess:entry.turn;
    const reference=process?.outcome==='completed'&&process.final_message_id_deferred?process.final_message_id_source:undefined;
    if(reference)bindings.set(contentRefKey(reference),{path:['transcript',entry.cursor,'turn_process','final_message_id'],content_ref:reference,deferred:true});
  }
  for(const descriptor of messageDescriptors(state)) {
    const process=descriptor.turn_process;
    const reference=process?.outcome==='completed'&&process.final_message_id_deferred?process.final_message_id_source:undefined;
    if(reference)bindings.set(contentRefKey(reference),{path:['message_descriptors',JSON.stringify(descriptor.locator),'turn_process','final_message_id'],content_ref:reference,deferred:true});
  }
  for(const [index,status] of state.statuses.entries()) {
    if(status.rendered_ref) bindings.set(contentRefKey(status.rendered_ref),{path:['statuses',String(index),'rendered'],content_ref:status.rendered_ref,deferred:true});
    const metadata=status.opportunities.fresh_inbound?.target_metadata_ref;
    if(metadata) bindings.set(contentRefKey(metadata),{path:['statuses',String(index),'opportunities','fresh_inbound','target_metadata'],content_ref:metadata,deferred:true});
  }
  return [...bindings.values()].filter(binding=>binding.deferred);
}
/**
 * Folds one routed observation into the next state.
 *
 * The shared native fold (`protocol/app-server/projection.ts`) is the single
 * authority for every fact it owns, and it evaluates each event — a whole
 * `projection_transition` batch included — exactly once. Everything the
 * terminal shows of those facts is read back from the folded native snapshot
 * by {@link nativeFields}; nothing here re-derives or overwrites them.
 *
 * What remains local is the terminal's display adaptation of the transcript
 * ({@link adaptTranscript}): the loaded history window joined with durable
 * pages read earlier, rows appended at their durable cursors, and the one
 * streaming row. It reads the folded snapshot where the runtime owns the
 * answer and never feeds anything back into it.
 *
 * Pure: the input state is never mutated, so a caller may keep the previous
 * value (for a diff, a test assertion, or an undo of a render).
 */
export function reduce(state: PresentationState, protocolEvent: ProjectedEvent): PresentationState {
  const { cursor, event } = protocolEvent;
  if (event.type === 'checkpoint_advanced') return replaceFromSnapshot(checkpointSnapshot(event.checkpoint), cursor);
  const native = nativeSnapshots.get(state);
  if (!native) throw new Error('No authoritative checkpoint for Runtime Client observation.');
  const projected = foldRuntimeEvent(native, event);
  const adaptation: Adaptation = { projected, displayedInbound: new Map((native.inbound.pending ?? []).map(item => [item.message.id, item.sequence])) };
  let view: TranscriptView = transcriptView(state);
  for (const fact of transitionFacts(event)) view = adaptTranscript(view, fact, adaptation);
  const next: PresentationState = { ...view, ...nativeFields(projected), cursor };
  nativeSnapshots.set(next, projected);
  preserveHistoryBindings(state, next);
  return next;
}

/** The terminal-owned transcript display state, beside the native facts. */
type TranscriptView = Pick<PresentationState, 'transcript' | 'transcriptNextCursor' | 'statistics' | 'deferredTranscriptEntries'>;

function transcriptView(state: TranscriptView): TranscriptView {
  return {
    transcript: state.transcript,
    transcriptNextCursor: state.transcriptNextCursor,
    statistics: state.statistics,
    deferredTranscriptEntries: state.deferredTranscriptEntries,
  };
}

/** The transcript view one native snapshot alone describes. */
function snapshotTranscript(snapshot: RuntimeClientSnapshot): TranscriptView {
  return {
    transcript: orderTranscript([...(snapshot.transcript.entries ?? []).map(transcriptEntryFromWire), ...streamingRows(snapshot)]),
    statistics: snapshot.transcript.statistics,
    transcriptNextCursor: snapshot.transcript.next_cursor ?? undefined,
    deferredTranscriptEntries: boundedDeferred(snapshot.transcript.deferred_entries ?? []),
  };
}

/** Rebuild only the finite native streaming head: provider block indexes are
 * identities, while the native projection owns ordinal insertion/eviction. */
function withNativeStreaming(view: TranscriptView, projected: RuntimeClientSnapshot): TranscriptView {
  return { ...view, transcript: [...view.transcript.filter(isDurableTranscriptEntry), ...streamingRows(projected)] };
}

/**
 * What transcript adaptation reads while it walks one observation's facts.
 *
 * `projected` is the native snapshot after the complete observation, so a
 * fact inside a batch reads the batch's settled native head. The native fold
 * owns the pending inbound queue; `displayedInbound` is only the display
 * bookkeeping of which committed user rows currently show a pending inbound
 * message (by message id, with its native sequence), seeded from the native
 * queue before the observation and advanced fact by fact, so an edit or
 * removal inside a batch rewrites exactly the row it displayed.
 */
interface Adaptation {
  readonly projected: RuntimeClientSnapshot;
  readonly displayedInbound: Map<string, string>;
}

/** Adapts the terminal transcript to one native fact. */
function adaptTranscript(view: TranscriptView, event: RuntimeClientEvent, adaptation: Adaptation): TranscriptView {
  const { projected, displayedInbound } = adaptation;
  switch (event.type) {
    case 'checkpoint_advanced':
      displayedInbound.clear();
      for (const item of event.checkpoint.snapshot.inbound.pending ?? []) displayedInbound.set(item.message.id, item.sequence);
      return snapshotTranscript(event.checkpoint.snapshot);
    case 'projection_transition':
      return event.events.reduce((current, item) => adaptTranscript(current, item, adaptation), view);
    case 'read_domains_updated':
      return mergeNativeTranscript(view, {
        ...view,
        transcript: orderTranscript([...(event.transcript.entries ?? []).map(transcriptEntryFromWire), ...view.transcript.filter(entry => entry.kind === 'streaming')]),
        transcriptNextCursor: event.transcript.next_cursor ?? undefined,
        statistics: event.transcript.statistics,
      });
    // Native content and deferred publication settlement change the loaded
    // window itself; the terminal joins the fresh native window to its pages.
    case 'block_content_advanced':
    case 'assistant_publication_deferred_settled':
      return mergeNativeTranscript(view, snapshotTranscript(projected));
    case 'assistant_message_started':
    case 'tool_call_started':
    case 'tool_call_assembled':
      return withNativeStreaming(view, projected);
    // Loaded committed rows carry their own bounded occurrence slots.
    case 'tool_execution_started':
    case 'tool_execution_progress':
    case 'tool_execution_settled':
    case 'tool_result_committed':
    case 'tool_occurrence_rebound':
      return { ...view, transcript: view.transcript.map(row => row.kind === 'committed' && row.toolCalls ? { ...row, toolCalls: refineToolSlots(row.toolCalls, event) } : row) };
    case 'attempt_settled':
      // An unsettled streaming message belongs to an attempt that ended
      // without committing; drop the partial render, keep committed history.
      return { ...view, transcript: dropStreaming(view.transcript, event.attempt_id) };
    case 'interaction_audit_requested':
      return { ...view, transcript: appendTranscriptEntry(view.transcript, {
        kind: 'interaction_requested',
        key: `interaction-requested:${event.audit.event_id}`,
        cursor: event.transcript_cursor,
        eventId: event.audit.event_id,
        timestamp: event.audit.timestamp,
        attemptId: event.audit.attempt_id,
        turnId: event.audit.turn_id,
        interactionId: event.audit.interaction_id,
        subject: event.audit.subject,
      }) };
    case 'interaction_audit_settled':
      return { ...view, transcript: appendTranscriptEntry(view.transcript, {
        kind: 'interaction_settled',
        key: `interaction-settled:${event.audit.event_id}`,
        cursor: event.transcript_cursor,
        eventId: event.audit.event_id,
        timestamp: event.audit.timestamp,
        attemptId: event.audit.attempt_id,
        turnId: event.audit.turn_id,
        interactionId: event.audit.interaction_id,
        settlement: event.audit.settlement,
      }) };
    case 'assistant_publication_settled':
      // The stream settled without ever becoming a canonical Assistant
      // message, so the in-flight card is dropped exactly as the Rust
      // projection drops it. The audit is committed-for-release output, not
      // canonical conversation history, so it enters the transcript only as
      // a typed non-canonical audit and its proposed tool calls never create
      // a foreground execution slot.
      return { ...view, transcript: appendTranscriptEntry(dropStreaming(view.transcript, event.attempt_id), {
        kind: 'publication_audit',
        key: `publication:${event.audit.stream_id}`,
        cursor: event.transcript_cursor,
        audit: event.audit,
      }) };
    case 'message_committed': {
      const transcriptCursor = validateTranscriptCursorContract(event.message, event.transcript_cursor);
      if (isHiddenContextMessage(event.message)) return view;
      if (transcriptCursor === undefined) throw new Error('visible message_committed event is missing its durable transcript cursor');
      const messageId = messageIdOf(event.message);
      // The wire spells "no committing attempt" as `null`; the projection
      // spells it as absent, once, here.
      const committingAttempt = event.attempt_id ?? undefined;
      const transcript = event.message.role === 'assistant' && committingAttempt !== undefined
        ? dropStreaming(view.transcript, committingAttempt) : view.transcript;
      // Canonical content and execution-terminal Journal facts are independent:
      // ToolResultCommitted carries C evidence; only ToolExecutionSettled
      // changes the foreground execution phase.
      return { ...view, transcript: appendTranscriptEntry(transcript, {
        kind: 'committed',
        key: `committed:${messageId}`,
        messageId,
        cursor: transcriptCursor,
        attemptId: committingAttempt,
        message: event.message,
        nativeFactsPending: event.message.role === 'assistant',
      }) };
    }
    case 'inbound_enqueued': {
      // Durable acceptance is the display frontier. Context facts remain
      // model-visible runtime input but are hidden from ordinary chat.
      const transcriptCursor = validateTranscriptCursorContract(event.message, event.transcript_cursor);
      if (isHiddenContextMessage(event.message)) return view;
      if (transcriptCursor === undefined) throw new Error('visible inbound_enqueued event is missing its durable transcript cursor');
      displayedInbound.set(event.message.id, event.sequence);
      return { ...view, transcript: appendTranscriptEntry(view.transcript, {
        kind: 'committed',
        key: `committed:${event.message.id}`,
        messageId: event.message.id,
        cursor: transcriptCursor,
        message: { role: 'user', ...event.message },
      }) };
    }
    case 'pending_inbound_changed': {
      // An edited pending message replaces its row; a removed one leaves it.
      const previous = new Set(displayedInbound.keys());
      const pending = new Map(event.pending.map(item => [item.message.id, item.message]));
      displayedInbound.clear();
      for (const item of event.pending) displayedInbound.set(item.message.id, item.sequence);
      return { ...view, transcript: view.transcript.flatMap(entry => {
        if (entry.kind !== 'committed' || !previous.has(entry.messageId)) return [entry];
        const message = pending.get(entry.messageId);
        return message ? [{ ...entry, message: { role: 'user' as const, ...message } }] : [];
      }) };
    }
    case 'inbound_drained':
      // Drained messages are ordinary history now; their rows stay as shown.
      for (const [id, sequence] of displayedInbound) if (BigInt(sequence) <= BigInt(event.watermark)) displayedInbound.delete(id);
      return view;
    case 'trace_changed': // Trace is a read capability; this client has no Trajectory UI.
    case 'goal_changed':
    case 'workflows_updated':
    case 'attempt_started':
    case 'attempt_turn_updated':
    case 'attempt_usage_updated':
    case 'message_descriptor_committed':
    case 'pending_head_changed':
    case 'interaction_pending':
    case 'interaction_settled':
    case 'interaction_removed':
    case 'context_compaction_started':
    case 'context_compaction_failed':
    case 'context_compacted':
    case 'agent_status_composed':
    case 'job_updated':
    case 'agent_updated':
    case 'collection_changed':
    case 'capability_updated':
    case 'resource_generation_updated':
    case 'session_model_changed':
    case 'runtime_shutdown':
    case 'runtime_durability_failed':
    case 'configuration_adoption_eligibility_changed':
      return view;
    default: {
      // The generated event vocabulary is closed and this switch is
      // exhaustive over it. An unhandled variant is a compile error, not a
      // silently ignored runtime fact.
      const exhaustiveEvent: never = event;
      throw new Error(`unreachable Runtime Client event: ${String(exhaustiveEvent)}`);
    }
  }
}

function presentSnapshot(
  snapshot: RuntimeClientSnapshot,
  cursor: RuntimeClientCursor,
): PresentationState {
  return { ...snapshotTranscript(snapshot), ...nativeFields(snapshot), cursor };
}

/** The one streaming row a native snapshot's in-flight Assistant message is. */
function streamingRows(snapshot: RuntimeClientSnapshot): StreamingMessage[] {
  const attempt = snapshot.attempt ?? undefined;
  const inFlight = attempt?.in_flight ?? undefined;
  if (attempt === undefined || inFlight === undefined) return [];
  // A snapshot taken mid-stream carries every accumulated delta through its
  // cursor, so the streaming message is rebuilt exactly as it would have
  // been observed incrementally.
  return [{
    kind: "streaming",
    key: `streaming:${attemptOwnerKey({conversation_id:snapshot.conversation_id,...attempt})}:${inFlight.message_id}`,
    attemptId: attempt.attempt_id ?? undefined,
    attemptSource: attempt.attempt_id_source ?? undefined,
    messageId: inFlight.message_id,
    blocks: (inFlight.blocks ?? []).map(
      (block): StreamingBlock =>
        block.type === "tool_call"
          ? {
              kind: "tool_call",
              blockIndex: block.block_index,
              callId: block.call_id ?? undefined,
              toolId: block.tool_id ?? undefined,
              name: block.name,
              argumentsText: block.arguments,
            }
          : {
              kind: block.type,
              blockIndex: block.block_index,
              text: block.text,
            },
    ),
  }];
}

/**
 * Every fact the shared native fold owns, read from its snapshot.
 *
 * Each section is normalized exactly once, here. The wire omits an empty
 * collection and spells "absent" as `null`; the projection carries one shape
 * so no renderer has to ask the same question twice.
 */
function nativeFields(snapshot: RuntimeClientSnapshot): Omit<PresentationState, keyof TranscriptView | 'cursor'> {
  const attempt = snapshot.attempt ?? undefined;
  return {
    conversationId: snapshot.conversation_id,
    goal: snapshot.goal ?? null,
    workflows: snapshot.workflows,
    attempt:
      attempt === undefined
        ? undefined
        : {
            attemptId: attempt.attempt_id ?? undefined,
            attemptSource: attempt.attempt_id_source ?? undefined,
            phase: attempt.phase,
            turn: attempt.turn,
            lastUsage: attempt.last_usage ?? undefined,
            model: attempt.model ?? undefined,
            executionSettings: attempt.execution_settings ?? undefined,
            foreground: [...(attempt.foreground ?? [])],
          },
    inbound: {
      pending: [...(snapshot.inbound.pending ?? [])],
      last_drain: snapshot.inbound.last_drain ?? undefined,
    },
    pendingInteractions: (snapshot.pending_interactions ?? []).map(displayInteraction),
    messageDescriptors: [...snapshot.message_descriptors],
    publicationSettlements: [...snapshot.publication_settlements ?? []],
    // Native display transitions define the head positions and evictions.
    // The terminal renderer never grows a second Agent/Job collection.
    jobs: [...(snapshot.jobs ?? [])],
    agents: [...(snapshot.agents ?? [])],
    // The runtime's bounded composition window, verbatim. Every annotation
    // the transcript draws is reconstructed from this list and the placement
    // facts each status carries, so a repair never has to consult — or
    // carry over — the state it replaces.
    statuses: dedupeStatuses(snapshot.statuses ?? []),
    context: snapshot.context ?? {
      compaction_in_progress: false,
      compaction_count: 0,
    },
    capabilities: snapshot.capabilities,
    resources: snapshot.resources ?? emptyResources(),
    // The runtime derives the list from the whole Ledger, so this is the
    // one repair path for it too: an attach, a resume, and a reload after
    // compaction all open on exactly the list canonical history holds,
    // however far back the last `todo` result now sits.
    //
    // `null` is the composition fact and is kept as `undefined`: a runtime
    // that composes no Todo extension has no current list, which is not the
    // same as an empty one (Issue #259).
    todos:
      snapshot.todos === null || snapshot.todos === undefined
        ? undefined
        : (parseSnapshot(snapshot.todos) ?? { tasks: [], next_id: "1" }),
    sessionModel: snapshot.model ?? null,
    modelStatus: snapshot.model_status ?? null,
    effectivePlugins: snapshot.effective_plugins ?? null,
    settingsEvidence: snapshot.settings_evidence,
    runtimeShutdown: snapshot.shutting_down,
    durabilityFailure: snapshot.durability_failure ?? null,
    adoptionEligibility: snapshot.configuration_adoption_eligibility.status,
    effectiveApprovalMode: snapshot.effective_approval_mode,
  };
}

/** Refresh native facts without throwing away a safely joined history window.
 * Fresh facts win by exact durable identity AND cursor. Only immutable entries
 * strictly before the new window can survive outside it; gaps require replacement.
 */
function mergeNativeTranscript(state: TranscriptView, fresh: TranscriptView): TranscriptView {
  const incoming = fresh.transcript.filter(isDurableTranscriptEntry);
  const previous = state.transcript.filter(isDurableTranscriptEntry);
  if (!incoming.some(entry => previous.some(old => old.key === entry.key && old.cursor === entry.cursor))) return fresh;
  const first = incoming[0]!;
  if (compareExact(first.cursor, previous[0]!.cursor) < 0) return fresh;
  const older: DurableTranscriptEntry[] = [];
  for (const old of previous) {
    const match = incoming.find(entry => entry.key === old.key || entry.cursor === old.cursor);
    if (match) {
      if (match.key !== old.key || match.cursor !== old.cursor) return fresh;
    } else {
      if (compareExact(old.cursor, first.cursor) >= 0 ||
          (old.kind === "committed" && old.nativeFactsPending)) return fresh;
      older.push(old);
    }
  }
  return {
    ...fresh,
    transcript: boundedHistory(orderTranscript([...older, ...fresh.transcript])),
    transcriptNextCursor: state.transcriptNextCursor,
  };
}

function boundedDeferred(rows: import('../../../protocol/app-server/v57.ts').CheckpointPageItem[]) {
 const unique=new Map(rows.map(row=>[row.offset,row])),kept:import('../../../protocol/app-server/v57.ts').CheckpointPageItem[]=[];let bytes=0;
 for(const row of [...unique.values()].reverse()){const size=new TextEncoder().encode(JSON.stringify(row)).length;if(kept.length===512||bytes+size>4*1024*1024)break;bytes+=size;kept.push(row);}
 return kept.reverse();
}
const HISTORY_CACHE_BYTES=8*1024*1024,HISTORY_CACHE_ITEMS=512;
function boundedHistory(entries: TranscriptEntry[], oldest=false): TranscriptEntry[] {
 const kept:TranscriptEntry[]=[];let bytes=0;
 for(const entry of oldest?entries:[...entries].reverse()) {
   const size=new TextEncoder().encode(JSON.stringify(entry)).length;
   if(kept.length===HISTORY_CACHE_ITEMS||bytes+size>HISTORY_CACHE_BYTES)break;
   bytes+=size;kept.push(entry);
 }
 return oldest?kept:kept.reverse();
}
/** Merges an older durable page ahead of the currently loaded transcript. */
function preserveHistoryBindings(previous:PresentationState,next:PresentationState,incoming:ContentBinding[] = []) {
 const ids=new Set(next.transcript.filter(isDurableTranscriptEntry).map(entry=>entry.kind==='committed'?entry.message.id:undefined));
 const merged=new Map<string,ContentBinding>();
 for(const binding of [...historyBindings.get(previous)??[],...incoming]) {
   if(binding.content_ref.source.type==='canonical_message_block' && binding.content_ref.source.locator.type==='by_message_id' && !ids.has(binding.content_ref.source.locator.message_id))continue;
   merged.set(contentRefKey(binding.content_ref),binding);
 }
 // Every retained descriptor came from at most one finite loaded row page.
 const values=[...merged.values()];let bytes=0;const kept:ContentBinding[]=[];
 for(const binding of values){bytes+=new TextEncoder().encode(JSON.stringify(binding)).length;if(kept.length>=4096||bytes>4*1024*1024)break;kept.push(binding);}
 historyBindings.set(next,kept);
}
export function mergeTranscriptPage(
  state: PresentationState,
  page: RuntimeClientTranscriptPage,
): PresentationState {
  const older = (page.entries ?? []).map(transcriptEntryFromWire);
  const merged = deduplicateTranscript([...state.transcript, ...older]);
  const next={ ...state,deferredTranscriptEntries:boundedDeferred([...state.deferredTranscriptEntries,...page.deferred_entries??[]]),transcript:boundedHistory(orderTranscript(merged),true),transcriptNextCursor:page.next_cursor??undefined };
  const native=nativeSnapshots.get(state);if(native)nativeSnapshots.set(next,native);preserveHistoryBindings(state,next,[...page.content??[],...(page.deferred_entries??[]).flatMap(row=>row.content)]);
  return next;
}

// ---------------------------------------------------------------------------
// Internals
// ---------------------------------------------------------------------------

function transcriptEntryFromWire(
  entry: RuntimeClientTranscriptEntry,
): TranscriptEntry {
  switch (entry.item.type) {
    case "attempt_terminal":
      return { kind: "attempt_terminal", key: `attempt:${attemptOwnerKey(entry.item.turn)}`, cursor: entry.cursor, turn: entry.item.turn };
    case "message": {
      const messageId = messageIdOf(entry.item.message);
      return {
        kind: "committed",
        key: `committed:${messageId}`,
        messageId,
        cursor: entry.cursor,
        message: entry.item.message,
        ...(entry.tool_calls==null?{}:{toolCalls:[...entry.tool_calls]}),
        nativeFactsPending: entry.response_pending === true || (entry.tool_calls ?? []).some(call => call.state.type !== "settled"),
        ...(entry.completed_response == null ? {} : { completedResponse: entry.completed_response }),
        ...(entry.turn_process == null ? {} : { turnProcess: entry.turn_process }),
      };
    }
    case "publication_audit":
      return {
        kind: "publication_audit",
        key: `publication:${entry.item.audit.stream_id}`,
        cursor: entry.cursor,
        audit: entry.item.audit,
      };
    case "interaction_requested":
      return {
        kind: "interaction_requested",
        key: `interaction-requested:${entry.item.event_id}`,
        cursor: entry.cursor,
        eventId: entry.item.event_id,
        timestamp: entry.item.timestamp,
        attemptId: entry.item.attempt_id,
        turnId: entry.item.turn_id,
        interactionId: entry.item.interaction_id,
        subject: entry.item.subject,
      };
    case "interaction_settled":
      return {
        kind: "interaction_settled",
        key: `interaction-settled:${entry.item.event_id}`,
        cursor: entry.cursor,
        eventId: entry.item.event_id,
        timestamp: entry.item.timestamp,
        attemptId: entry.item.attempt_id,
        turnId: entry.item.turn_id,
        interactionId: entry.item.interaction_id,
        settlement: entry.item.settlement,
      };
  }
}

function appendTranscriptEntry(
  transcript: TranscriptEntry[],
  entry: TranscriptEntry,
): TranscriptEntry[] {
  const existing = transcript.find((candidate) => candidate.key === entry.key);
  if (existing !== undefined) {
    if (
      existing.kind !== "streaming" &&
      entry.kind !== "streaming" &&
      existing.cursor !== entry.cursor
    ) {
      throw new Error(
        `transcript fact ${entry.key} changed durable cursor from ${existing.cursor} to ${entry.cursor}`,
      );
    }
    return transcript;
  }
  return orderTranscript([...transcript, entry]);
}

type DurableTranscriptEntry = Exclude<TranscriptEntry, StreamingMessage>;

function isDurableTranscriptEntry(
  entry: TranscriptEntry,
): entry is DurableTranscriptEntry {
  return entry.kind !== "streaming";
}

/** Orders every durable item by the cursor allocated by `transcript_order`. */
function orderTranscript(transcript: TranscriptEntry[]): TranscriptEntry[] {
  const durable = transcript
    .filter(isDurableTranscriptEntry)
    .sort((left, right) => compareExact(left.cursor, right.cursor));
  const streaming = transcript.filter((entry) => entry.kind === "streaming");
  return [...durable, ...streaming];
}

/** Deduplicates the same durable fact without using identity to infer order. */
function deduplicateTranscript(transcript: TranscriptEntry[]): TranscriptEntry[] {
  const seen = new Map<string, TranscriptEntry>();
  for (const entry of transcript) {
    const existing = seen.get(entry.key);
    if (
      existing !== undefined &&
      isDurableTranscriptEntry(existing) &&
      isDurableTranscriptEntry(entry) &&
      existing.cursor !== entry.cursor
    ) {
      throw new Error(
        `transcript fact ${entry.key} has conflicting durable cursors`,
      );
    }
    if (existing === undefined) {
      seen.set(entry.key, entry);
    }
  }
  return [...seen.values()];
}

/**
 * Appends one composed status, keyed by its canonical status message.
 *
 * The runtime bounds its own window, so this never trims: trimming here would
 * make the incremental fold disagree with the snapshot for no reason.
 */
/**
 * Applies one runtime status-window transition.
 *
 * Retention is owned by the runtime projection and nowhere else. This client
 * does not know the window bound, does not count entries, and does not decide
 * what to drop: it removes exactly the composition the runtime says its own
 * admission evicted, then applies the identity-keyed admission. Folding every
 * transition from a snapshot at cursor `C` therefore rebuilds precisely the
 * window the runtime holds at `C`, past the bound as well as below it.
 *
 * A second retention rule here would be a second policy, and the two would
 * diverge the moment the bound was crossed — a live fold keeping a
 * composition a later snapshot repair would silently drop.
 */
/** Keeps one entry per composition identity, in the runtime's own order. */
function dedupeStatuses(statuses: AgentStatusView[]): AgentStatusView[] {
  const seen = new Set<string>();
  const kept: AgentStatusView[] = [];
  for (const status of statuses) {
    const identity = statusIdentity(status);
    if (seen.has(identity)) {
      continue;
    }
    seen.add(identity);
    kept.push(status);
  }
  return kept;
}

function messageIdOf(message: {
  id?: string;
  role?: string;
}): string {
  return message.id ?? "";
}

function dropStreaming(
  transcript: TranscriptEntry[],
  attemptId: string,
): TranscriptEntry[] {
  return transcript.filter(
    (entry) => entry.kind !== "streaming" || entry.attemptId !== attemptId,
  );
}

/** Durable read-only history while runtime preparation is pending. */
export function historyPresentation(conversationId: string, page: import('../protocol/app-server.ts').RuntimeClientTranscriptPage): PresentationState {
  return { ...emptyPresentationState(null), conversationId, transcript: (page.entries ?? []).map(transcriptEntryFromWire), statistics: page.statistics, transcriptNextCursor: page.next_cursor ?? undefined, deferredTranscriptEntries:boundedDeferred(page.deferred_entries??[]) };
}
