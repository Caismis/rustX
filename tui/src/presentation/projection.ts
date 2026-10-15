import { attemptOwnerKey } from '../../../protocol/app-server/attempt-identity.ts';
import { messageDescriptors } from './message-descriptors.ts';
import { contentRefKey } from '../../../protocol/app-server/content-identity.ts';
import { refineToolSlots } from '../../../protocol/app-server/projection.ts';
import { displayInteraction, type DisplayInteraction } from './interaction-display.ts';
import type { ContentBinding } from '../../../protocol/app-server/v50.ts';
import { foldRuntimeEvent, checkpointSnapshot, contentBindings, observationCheckpoint, statusIdentity } from '../../../protocol/app-server/projection.ts';
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
  AttemptModelView,
  RoutedInteraction,
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
  AttemptPresentation,
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
const TOOL_EVENT_TYPES=['tool_call_started','tool_call_assembled','tool_call_rebound','tool_execution_rebound','tool_occurrence_rebound','tool_execution_started','tool_execution_progress','tool_result_committed','tool_execution_settled'] as const;
type NativeToolEvent=Extract<RuntimeClientEvent,{type:typeof TOOL_EVENT_TYPES[number]}>;
function isNativeToolEvent(event:RuntimeClientEvent): event is NativeToolEvent {
  return (TOOL_EVENT_TYPES as readonly string[]).includes(event.type);
}

export function reduce(state: PresentationState, protocolEvent: ProjectedEvent): PresentationState {
  const native = nativeSnapshots.get(state), event = protocolEvent.event;
  if (event.type === 'projection_transition') {
    let next = state;
    for (const item of event.events) next = reduce(next, { cursor: protocolEvent.cursor, event: item });
    if (native) nativeSnapshots.set(next, foldRuntimeEvent(native, event));
    preserveHistoryBindings(state,next);
    return next;
  }
  if (event.type === 'checkpoint_advanced') return replaceFromSnapshot(checkpointSnapshot(event.checkpoint), protocolEvent.cursor);
  if (event.type === 'block_content_advanced') {
    if (!native) throw new Error('No checkpoint for content advancement.');
    const next=mergeNativeTranscript(state,replaceFromSnapshot(foldRuntimeEvent(native,event),protocolEvent.cursor));
    nativeSnapshots.set(next,foldRuntimeEvent(native,event));preserveHistoryBindings(state,next);return next;
  }
  if (event.type === 'assistant_publication_deferred_settled') {
    if (!native) throw new Error('No authoritative checkpoint for publication settlement.');
    const projected = foldRuntimeEvent(native, event);
    const next = mergeNativeTranscript(state, replaceFromSnapshot(projected, protocolEvent.cursor));
    nativeSnapshots.set(next, projected);
    preserveHistoryBindings(state, next);
    return next;
  }
  if (isNativeToolEvent(event)) {
    if(!native)throw new Error('No authoritative checkpoint for Tool observation.');
    const projected=foldRuntimeEvent(native,event);
    const head=presentSnapshot(projected,protocolEvent.cursor);
    const next={...state,cursor:protocolEvent.cursor,attempt:head.attempt,messageDescriptors:[...projected.message_descriptors],
      transcript:state.transcript.map(row=>row.kind==='committed' && row.toolCalls && ['tool_execution_started','tool_execution_progress','tool_execution_settled','tool_result_committed','tool_occurrence_rebound'].includes(event.type)
        ?{...row,toolCalls:refineToolSlots(row.toolCalls??[],event as Parameters<typeof refineToolSlots>[1])}:row)};
    if(['tool_call_started','tool_call_assembled','tool_call_rebound'].includes(event.type)) {
      next.transcript=[...state.transcript.filter(row=>row.kind!=='streaming'),...head.transcript.filter(row=>row.kind==='streaming')];
    }
    nativeSnapshots.set(next,projected);
    preserveHistoryBindings(state,next);
    return next;
  }
  const next = reduceOrdinaryState(state, { ...protocolEvent, event });
  if (native) {
    const projected=foldRuntimeEvent(native,event);
    nativeSnapshots.set(next,projected);
    // Native display transitions define the head positions and evictions.
    // The terminal renderer never grows a second Agent/Job collection.
    next.jobs=[...(projected.jobs ?? [])];
    next.agents=[...(projected.agents ?? [])];
    next.statuses=[...(projected.statuses ?? [])];
    next.pendingInteractions=(projected.pending_interactions??[]).map(displayInteraction);
    next.todos=projected.todos==null?undefined:parseSnapshot(projected.todos);
    next.messageDescriptors=[...projected.message_descriptors];
    next.publicationSettlements=[...projected.publication_settlements ?? []];
    // Rebuild only the finite native streaming head: provider block indexes
    // are identities, while the projection owns ordinal insertion/eviction.
    if (["tool_call_started", "tool_call_assembled", "tool_call_rebound", "tool_execution_rebound", "assistant_message_started"].includes(event.type)) {
      const streaming = presentSnapshot(projected, protocolEvent.cursor).transcript.filter(row=>row.kind==="streaming");
      next.transcript = [...next.transcript.filter(row=>row.kind!=="streaming"), ...streaming];
    }
    if(next.attempt && projected.attempt && attemptOwnerKey({conversation_id:projected.conversation_id,...projected.attempt})===attemptOwnerKey({conversation_id:next.conversationId,attempt_id:next.attempt.attemptId,attempt_id_source:next.attempt.attemptSource}))next.attempt={...next.attempt,foreground:[...(projected.attempt.foreground??[])]};
  }
  preserveHistoryBindings(state,next);
  return next;
}

function presentSnapshot(
  snapshot: RuntimeClientSnapshot,
  cursor: RuntimeClientCursor,
): PresentationState {
  const transcript: TranscriptEntry[] = orderTranscript(
    (snapshot.transcript.entries ?? []).map(transcriptEntryFromWire),
  );

  // Every section below is normalized exactly once, here. The wire omits an
  // empty collection and spells "absent" as `null`; the projection carries one
  // shape so no renderer has to ask the same question twice.
  const attempt = snapshot.attempt ?? undefined;
  const inFlight = attempt?.in_flight ?? undefined;
  if (attempt !== undefined && inFlight !== undefined) {
    // A snapshot taken mid-stream carries every accumulated delta through its
    // cursor, so the streaming message is rebuilt exactly as it would have
    // been observed incrementally.
    transcript.push({
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
    });
  }

  return {
    conversationId: snapshot.conversation_id,
    goal: snapshot.goal ?? null,
    workflows: snapshot.workflows,
    cursor,
    transcript: orderTranscript(transcript),
    statistics: snapshot.transcript.statistics,
    transcriptNextCursor: snapshot.transcript.next_cursor ?? undefined,
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
    deferredTranscriptEntries: boundedDeferred(snapshot.transcript.deferred_entries??[]),
    publicationSettlements: [...snapshot.publication_settlements ?? []],
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
function mergeNativeTranscript(state: PresentationState, fresh: PresentationState): PresentationState {
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

function boundedDeferred(rows: import('../../../protocol/app-server/v50.ts').CheckpointPageItem[]) {
 const unique=new Map(rows.map(row=>[row.offset,row])),kept:import('../../../protocol/app-server/v50.ts').CheckpointPageItem[]=[];let bytes=0;
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

/**
 * Folds one protocol event into the next state.
 *
 * Pure: the input state is never mutated, so a caller may keep the previous
 * value (for a diff, a test assertion, or an undo of a render).
 */
function reduceOrdinaryState(
    state: PresentationState,
    protocolEvent: ProjectedEvent & { event: Exclude<ProjectedEvent['event'], NativeToolEvent | { type: 'projection_transition' | 'checkpoint_advanced' | 'block_content_advanced' }> },
): PresentationState {
  const event = protocolEvent.event;
  const transcriptCursor =
    event.type === "message_committed" || event.type === "inbound_enqueued"
      ? validateTranscriptCursorContract(
          event.message,
          event.transcript_cursor,
        )
      : undefined;
  const next = { ...state, cursor: protocolEvent.cursor };

  switch (event.type) {
    case "read_domains_updated": {
      const fresh = mergeNativeTranscript(state, { ...next,
        transcript: orderTranscript([...(event.transcript.entries ?? []).map(transcriptEntryFromWire), ...state.transcript.filter(entry => entry.kind === 'streaming')]),
        transcriptNextCursor: event.transcript.next_cursor ?? undefined,
        statistics: event.transcript.statistics,
      });
      return { ...fresh, context: { ...state.context, occupancy: event.occupancy, projection_head: event.projection_head },
        todos: event.todos == null ? undefined : parseSnapshot(event.todos) };
    }
    case "goal_changed":
      return { ...next, goal: event.view };
    case "workflows_updated":
      return { ...state, cursor: protocolEvent.cursor, workflows: event.workflows };
    case "attempt_started":
      // The frozen model travels with the event, so the active attempt's
      // model is known without a snapshot round trip and without inference.
      //
      // Composed Agent Statuses are deliberately untouched. They are
      // historical facts of the conversation, not attempt-scoped live state,
      // and the runtime's own window keeps them across attempts too — so
      // clearing them here would make this client disagree with the snapshot
      // it would repair from.
      next.attempt = startAttempt(event.attempt_id, event.model, event.execution_settings);
      return next;

    case "attempt_settled":
      if (state.attempt?.attemptId === event.attempt_id) {
        next.attempt = {
          ...state.attempt,
          phase: { type: "settled", outcome: event.outcome },
        };
      }
      // An unsettled streaming message belongs to an attempt that ended
      // without committing; drop the partial render, keep committed history.
      next.transcript = state.transcript.filter(
        (entry) =>
          entry.kind !== "streaming" || entry.attemptId !== event.attempt_id,
      );
      next.context = { ...state.context, compaction_in_progress: false };
      return next;

    case "attempt_turn_updated":
      if (state.attempt?.attemptId === event.attempt_id) {
        next.attempt = { ...state.attempt, turn: event.turn };
      }
      return next;

    case "pending_head_changed":
    case "message_descriptor_committed":
      return next;
    case "trace_changed":
      return next; // Trace is a read capability; this client has no Trajectory UI.

    case "attempt_usage_updated":
      if (state.attempt?.attemptId === event.attempt_id) {
        next.attempt = { ...state.attempt, lastUsage: event.usage };
      }
      return next;

    case "interaction_pending":
      next.pendingInteractions = upsertInteraction(
        state.pendingInteractions,
        displayInteraction(event.interaction),
      );
      return next;

    case "interaction_settled":
      next.pendingInteractions = state.pendingInteractions.filter(
        (interaction) => !sameInteraction(interaction.interaction, event.interaction),
      );
      return next;

    case "interaction_removed":
      next.pendingInteractions = state.pendingInteractions.filter(
        (interaction) => !sameInteraction(interaction.interaction, event.interaction),
      );
      return next;

    case "interaction_audit_requested":
      next.transcript = appendTranscriptEntry(state.transcript, {
        kind: "interaction_requested",
        key: `interaction-requested:${event.audit.event_id}`,
        cursor: event.transcript_cursor,
        eventId: event.audit.event_id,
        timestamp: event.audit.timestamp,
        attemptId: event.audit.attempt_id,
        turnId: event.audit.turn_id,
        interactionId: event.audit.interaction_id,
        subject: event.audit.subject,
      });
      return next;

    case "interaction_audit_settled":
      next.transcript = appendTranscriptEntry(state.transcript, {
        kind: "interaction_settled",
        key: `interaction-settled:${event.audit.event_id}`,
        cursor: event.transcript_cursor,
        eventId: event.audit.event_id,
        timestamp: event.audit.timestamp,
        attemptId: event.audit.attempt_id,
        turnId: event.audit.turn_id,
        interactionId: event.audit.interaction_id,
        settlement: event.audit.settlement,
      });
      return next;

    case "context_compaction_started":
      next.context = { ...state.context, compaction_in_progress: true };
      return next;

    case "context_compaction_failed":
      next.context = { ...state.context, compaction_in_progress: false };
      return next;

    case "context_compacted":
      next.context = event.context;
      return next;

    case "assistant_message_started":
      next.transcript = [
        ...dropStreaming(state.transcript, event.attempt_id),
        {
          kind: "streaming",
          key: `streaming:${event.attempt_id}:${event.message_id}`,
          attemptId: event.attempt_id,
          messageId: event.message_id,
          blocks: [],
        },
      ];
      return next;

    case "assistant_publication_deferred_settled":
      return next; // Complete deferred source fold is owned by the shared reducer above.
    case "assistant_publication_settled":
      // The stream settled without ever becoming a canonical Assistant
      // message, so the in-flight card is dropped exactly as the Rust
      // projection drops it. The audit is committed-for-release output, not
      // canonical conversation history, so it enters the transcript only as
      // a typed non-canonical audit and its proposed tool calls never create
      // a foreground execution slot.
      next.transcript = appendTranscriptEntry(
        dropStreaming(state.transcript, event.attempt_id),
        {
          kind: "publication_audit",
          key: `publication:${event.audit.stream_id}`,
          cursor: event.transcript_cursor,
          audit: event.audit,
        },
      );
      return next;

    case "message_committed": {
      if (isHiddenContextMessage(event.message)) {
        return next;
      }
      if (transcriptCursor === undefined) {
        throw new Error(
          "visible message_committed event is missing its durable transcript cursor",
        );
      }
      const messageId = messageIdOf(event.message);
      // The wire spells "no committing attempt" as `null`; the projection
      // spells it as absent, once, here.
      const committingAttempt = event.attempt_id ?? undefined;
      const transcript =
        event.message.role === "assistant" && committingAttempt !== undefined
          ? dropStreaming(state.transcript, committingAttempt)
          : state.transcript;
      next.transcript = appendTranscriptEntry(transcript, {
        kind: "committed",
        key: `committed:${messageId}`,
        messageId,
        cursor: transcriptCursor,
        attemptId: committingAttempt,
        message: event.message,
        nativeFactsPending: event.message.role === "assistant",
      });
      // Canonical Tool content is independently displayable. The native Todo
      // observation owner alone replaces the current task projection.
      // Canonical content and execution-terminal Journal facts are independent.
      // ToolResultCommitted carries C evidence; only ToolExecutionSettled changes
      // the foreground execution phase.
      return next;
    }

    case "agent_status_composed":
      // The shared native fold installs exactly the published head/eviction.
      return next;

    case "inbound_enqueued":
      next.inbound = {
        ...state.inbound,
        pending: [
          ...(state.inbound.pending ?? []),
          { sequence: event.sequence, revision: "0", message: event.message },
        ],
      };
      // Durable acceptance is the display frontier. Context facts remain
      // model-visible runtime input but are hidden from ordinary chat.
      if (!isHiddenContextMessage(event.message)) {
        if (transcriptCursor === undefined) {
          throw new Error(
            "visible inbound_enqueued event is missing its durable transcript cursor",
          );
        }
        next.transcript = appendTranscriptEntry(state.transcript, {
          kind: "committed",
          key: `committed:${event.message.id}`,
          messageId: event.message.id,
          cursor: transcriptCursor,
          message: { role: "user", ...event.message },
        });
      }
      return next;

    case "pending_inbound_changed": {
      const previous = new Set((state.inbound.pending ?? []).map(item => item.message.id));
      const pending = new Map(event.pending.map(item => [item.message.id, item.message]));
      next.inbound = { ...state.inbound, pending: event.pending };
      next.transcript = state.transcript.flatMap(entry => {
        if (entry.kind !== "committed" || !previous.has(entry.messageId)) return [entry];
        const message = pending.get(entry.messageId);
        return message ? [{ ...entry, message: { role: "user" as const, ...message } }] : [];
      });
      return next;
    }
    case "inbound_drained":
      next.inbound = {
        pending: (state.inbound.pending ?? []).filter(
          (item) => BigInt(item.sequence) > BigInt(event.watermark),
        ),
        last_drain: { watermark: event.watermark, count: event.count },
      };
      return next;

    case "collection_changed":
      return next;

    case "job_updated":
      next.jobs = upsertBackground(state.jobs, event.job);
      return next;

    case "agent_updated":
      next.agents = upsertSubagent(state.agents, event.agent);
      return next;

    case "capability_updated":
      next.capabilities = event.capabilities;
      return next;

    case "resource_generation_updated":
      // A reload commits the resource generation and the capability
      // generation it was composed against as one fact, and publishes them
      // as one event at one cursor. Both halves are folded here, together:
      // there is no cut of this reducer at which the client holds the new
      // capability generation beside the resource generation the same
      // reload retired.
      next.capabilities = event.capabilities;
      next.resources = event.resources;
      next.effectivePlugins = event.plugins ?? null;
      next.sessionModel = event.model ?? null;
      next.modelStatus = event.model_status;
      next.effectiveApprovalMode = event.approval_mode;
      return next;

    case "session_model_changed":
      // The session's desired model only. A running attempt keeps the model
      // it froze; nothing about `state.attempt` changes here.
      next.sessionModel = event.model ?? null;
      next.modelStatus = event.model_status;
      return next;

    case "runtime_shutdown":
      next.runtimeShutdown = true;
      return next;

    case "runtime_durability_failed":
      // The runtime entered its explicit degraded state: no new durable
      // admission may begin. This is a runtime-published fact like any other
      // and is carried, not interpreted — the client never decides that a
      // runtime is degraded, and never decides that it has recovered.
      next.durabilityFailure = {
        operation: event.operation,
        diagnostic: event.diagnostic,
      };
      return next;

    case "configuration_adoption_eligibility_changed":
      // Runtime-domain advisory state, carried as published; `/session adopt`
      // still submits the inspected candidate to the native gate.
      next.adoptionEligibility = event.eligibility.status;
      return next;

    default:
      // The generated event vocabulary is closed and this switch is
      // exhaustive over it. An unhandled variant is a compile error, not a
      // silently ignored runtime fact.
      const exhaustiveEvent: never = event;
      throw new Error(
        `unreachable Runtime Client event: ${String(exhaustiveEvent)}`,
      );
  }
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

function startAttempt(
  attemptId: string,
  model: AttemptModelView | null | undefined,
  executionSettings:
    | import("../protocol/app-server.ts").AdmittedSettings
    | null
    | undefined,
): AttemptPresentation {
  return {
    attemptId,
    phase: { type: "running" },
    turn: 0,
    model: model ?? undefined,
    executionSettings: executionSettings ?? undefined,
    foreground: [],
  };
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

function upsertBackground(
  background: PresentationState["jobs"],
  execution: PresentationState["jobs"][number],
): PresentationState["jobs"] {
  const index = background.findIndex(
    (entry) => entry.job_id === execution.job_id,
  );
  if (index === -1) {
    return [...background, execution];
  }
  const updated = [...background];
  updated[index] = execution;
  return updated;
}

function upsertSubagent(
  agents: PresentationState["agents"],
  subagent: PresentationState["agents"][number],
): PresentationState["agents"] {
  const index = agents.findIndex(
    (entry) => entry.agent_id === subagent.agent_id,
  );
  if (index === -1) {
    return [...agents, subagent];
  }
  const updated = [...agents];
  updated[index] = subagent;
  return updated;
}

function upsertInteraction(
  interactions: DisplayInteraction[],
  interaction: DisplayInteraction,
): DisplayInteraction[] {
  const index = interactions.findIndex((entry) =>
    sameInteraction(entry.interaction, interaction.interaction)
  );
  if (index === -1) {
    return [...interactions, interaction];
  }
  const updated = [...interactions];
  updated[index] = interaction;
  return updated;
}

function sameInteraction(
  left: RoutedInteraction["interaction"],
  right: RoutedInteraction["interaction"],
): boolean {
  return (
    left.conversation_id === right.conversation_id &&
    left.interaction_id === right.interaction_id
  );
}

/** Durable read-only history while runtime preparation is pending. */
export function historyPresentation(conversationId: string, page: import('../protocol/app-server.ts').RuntimeClientTranscriptPage): PresentationState {
  return { ...emptyPresentationState(null), conversationId, transcript: (page.entries ?? []).map(transcriptEntryFromWire), statistics: page.statistics, transcriptNextCursor: page.next_cursor ?? undefined, deferredTranscriptEntries:boundedDeferred(page.deferred_entries??[]) };
}
