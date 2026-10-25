/** Pure Runtime Client read-model fold. Routing/cursor continuity belongs to
 * the attachment owner. No I/O, providers, React, or execution authority. */
import type { RuntimeClientSnapshot as Snapshot, RuntimeClientEvent as Event, InFlightBlock, ForegroundToolExecution, ContentRef, ContentBinding, CanonicalContentLocator, RuntimeClientCheckpoint, ProjectionMetadata, RuntimeClientCursor } from './v57.ts';
import { canonicalLocatorKey, contentSourceKey, sameContentRef } from './content-identity.ts';

const cuts = new WeakMap<Snapshot, ProjectionMetadata>();
const frontiers = new WeakMap<Snapshot, Map<string, ContentBinding>>();
const bindingLists=new WeakMap<Map<string,ContentBinding>,ContentBinding[]>();
const noBindings:ContentBinding[]=[];
/** Inline identity when finite, otherwise the exact native canonical locator. */
export function statusIdentity(status: {status_message_id?:string|null;source_locator?:CanonicalContentLocator|null}):string {
  if (status.status_message_id != null) return status.status_message_id;
  if (status.source_locator != null) return `status:${canonicalLocatorKey(status.source_locator)}`;
  throw new Error('Status observation has no authoritative identity.');
}

/** Exact native canonical identity; serialized object field order is irrelevant. */
export function sameCanonicalLocator(a: CanonicalContentLocator, b: CanonicalContentLocator): boolean {
  return a.type === 'by_message_id' ? b.type === 'by_message_id' && a.message_id === b.message_id
    : b.type === 'by_ledger_position' && a.position === b.position;
}
/** Deferred optional IDs never establish a tool occurrence by themselves. */
export function sameToolOccurrence(a: Pick<ForegroundToolExecution, 'occurrence'>, b: Pick<ForegroundToolExecution, 'occurrence'>): boolean {
  return a.occurrence.block_index === b.occurrence.block_index
    && sameCanonicalLocator(a.occurrence.locator, b.occurrence.locator);
}

type NativeToolFact = Extract<Event,{type:'tool_execution_started'|'tool_execution_progress'|'tool_execution_settled'|'tool_result_committed'|'tool_occurrence_rebound'}>;
/** Refine loaded caches only from an exact native occurrence; canonical C does not settle J. */
export function refineToolSlots(rows: ForegroundToolExecution[], event: NativeToolFact): ForegroundToolExecution[] {
  if (!event.occurrence || event.type === 'tool_execution_progress' && event.execution_id != null) return rows;
  const occurrence=event.type==='tool_occurrence_rebound'?{locator:event.previous_locator,block_index:event.occurrence.block_index}:event.occurrence;
  let changed=false;
  const next=rows.map(tool=>{
    if(!sameToolOccurrence(tool,{occurrence}))return tool;
    const updated=(():ForegroundToolExecution=>{switch(event.type){
      case 'tool_occurrence_rebound': return {...tool,occurrence:event.occurrence,metadata_ref:event.metadata_ref};
      case 'tool_result_committed': return {...tool,committed_result:event.result,committed_result_deferred:event.result_deferred,result_metadata_ref:event.result_metadata_ref};
      case 'tool_execution_started': return tool.state.type==='settled'?tool:{...tool,metadata_ref:event.metadata_ref??tool.metadata_ref,state:{type:'running',arguments:tool.state.arguments,progress:null}};
      case 'tool_execution_progress': return tool.state.type!=='running'?tool:{...tool,metadata_ref:event.metadata_ref??tool.metadata_ref,state:{...tool.state,progress:event.progress}};
      case 'tool_execution_settled': return tool.state.type==='settled'?tool:{...tool,metadata_ref:event.metadata_ref??tool.metadata_ref,result_source:event.result_source,result_metadata_ref:event.result_metadata_ref??tool.result_metadata_ref,state:{type:'settled',arguments:tool.state.arguments,result:event.result}};
    }})();
    changed ||= updated!==tool;
    return updated;
  });
  return changed?next:rows;
}
function refineToolObservations(state: Snapshot, event: NativeToolFact): Snapshot {
  if(event.type==='tool_execution_progress' && event.execution_id!=null || !event.occurrence)return state;
  const foreground=state.attempt?.foreground?refineToolSlots(state.attempt.foreground,event):state.attempt?.foreground;
  let entriesChanged=false,descriptorsChanged=false;
  const entries=state.transcript.entries?.map(entry=>{
    if(!entry.tool_calls)return entry;
    const rows=refineToolSlots(entry.tool_calls,event);
    if(rows===entry.tool_calls)return entry;
    entriesChanged=true;return {...entry,tool_calls:rows};
  });
  const descriptors=state.message_descriptors.map(descriptor=>{
    const rows=refineToolSlots(descriptor.tool_calls,event);
    if(rows===descriptor.tool_calls)return descriptor;
    descriptorsChanged=true;return {...descriptor,tool_calls:rows};
  });
  if(foreground===state.attempt?.foreground && !entriesChanged && !descriptorsChanged)return state;
  return {...state,
    attempt:state.attempt && foreground!==state.attempt.foreground?{...state.attempt,foreground}:state.attempt,
    transcript:entriesChanged?{...state.transcript,entries}:state.transcript,
    message_descriptors:descriptorsChanged?descriptors:state.message_descriptors,
  };
}

const contentKey = contentSourceKey;
export function checkpointSnapshot(checkpoint: RuntimeClientCheckpoint): Snapshot {
  const bindings = new Map<string, ContentBinding>();
  for (const binding of checkpoint.content) bindings.set(contentKey(binding.content_ref), binding);
  cuts.set(checkpoint.snapshot, {read_cut: checkpoint.read_cut, collections: checkpoint.collections, content: checkpoint.content});
  frontiers.set(checkpoint.snapshot, bindings);
  return checkpoint.snapshot;
}
/** Loaded byte ranges and their evidence are observation cache state, never a Cursor. */
export function contentBindings(snapshot: Snapshot): ContentBinding[] {
  const map=frontiers.get(snapshot);if(!map)return noBindings;
  let bindings=bindingLists.get(map);if(!bindings){bindings=[...map.values()];bindingLists.set(map,bindings);}return bindings;
}
/** Exact native observation metadata accompanies the complete folded state. */
export function observationCheckpoint(snapshot: Snapshot, cursor: RuntimeClientCursor): RuntimeClientCheckpoint | undefined {
  const metadata = cuts.get(snapshot);
  return metadata ? {snapshot, cursor, ...metadata} : undefined;
}
const sameSource = (a: ContentRef, b: ContentRef) => contentSourceKey(a) === contentSourceKey(b);
const samePath = (left: string[] | null | undefined, right: string[] | null | undefined) =>
  left == null ? right == null : right != null && left.length === right.length && left.every((value,index)=>value===right[index]);
const sameBinding = (left: ContentBinding, right: ContentBinding) =>
  sameContentRef(left.content_ref,right.content_ref) && left.deferred===right.deferred
  && samePath(left.path,right.path) && samePath(left.source_path,right.source_path);
export function transitionFacts(event: Event): Event[] {
  return event.type === 'projection_transition' ? event.events.flatMap(transitionFacts) : [event];
}
export function foldRuntimeEvent(state: Snapshot, event: Event): Snapshot {
  if (event.type === 'checkpoint_advanced') return checkpointSnapshot(event.checkpoint);
  if (event.type === 'projection_transition') {
    const next = { ...event.events.reduce((current, item) => foldRuntimeEvent(current, item), state) };
    cuts.set(next, event.metadata);
    const previous = frontiers.get(state);
    const same = previous?.size === event.metadata.content.length && event.metadata.content.every(binding => {
      const prior = previous.get(contentKey(binding.content_ref));
      return prior && sameBinding(prior,binding);
    });
    frontiers.set(next, same ? previous! : new Map(event.metadata.content.map(binding => [contentKey(binding.content_ref), binding])));
    return next;
  }
  const metadata = frontiers.get(state) ?? new Map<string, ContentBinding>();
  let next: Snapshot;
  if (event.type === 'block_content_advanced') {
    if (!['text','reasoning','refusal','tool_call_arguments'].includes(event.kind)) throw new Error('Unknown native content advancement kind.');
    const source=event.content_ref.source;
    if(event.content_ref.conversation_id!==state.conversation_id||source.type!=='live_publication_prefix'
      ||source.component!=='payload'||source.attempt_id!==event.attempt_id||source.message_id!==event.message_id
      ||source.block_index!==event.block_index||event.content_ref.version!==1
      ||event.content_ref.representation!==(event.kind==='tool_call_arguments'?'raw_json_utf8':'utf8_text')) throw new Error('Content advancement source or representation mismatch.');
    const owners=contentBindings(state).filter(binding=>binding.content_ref.source.type==='live_publication_prefix'&&binding.content_ref.source.message_id===event.message_id);
    if(owners.some(binding=>{
      const previous=binding.content_ref.source;
      return previous.type==='live_publication_prefix'&&(previous.stream_id!==source.stream_id||previous.turn_id!==source.turn_id||previous.publication_request_id!==source.publication_request_id);
    }))throw new Error('Content advancement changed publication identity.');
    const key = contentKey(event.content_ref), previous = metadata.get(key)?.content_ref;
    if (previous && (!sameSource(previous, event.content_ref) || previous.readable_end !== Number(event.base_length) || previous.content_revision >= event.content_ref.content_revision)) throw new Error('Content version or prefix mismatch.');
    const attempt = state.attempt;
    if (!attempt?.in_flight || attempt.attempt_id !== event.attempt_id || attempt.in_flight.message_id !== event.message_id) throw new Error('Content advancement names a stale Attempt or Message.');
    const loadedBlock = Number(event.position) < 32;
    const foreground = attempt.foreground?.find(tool => tool.message_id === event.message_id && tool.block_index === event.block_index);
    if (!previous && (loadedBlock || foreground) && Number(event.base_length) !== 0) throw new Error('Missing content prefix evidence.');
    if (event.content_ref.content_revision !== Number(event.content_revision) || !Number.isSafeInteger(Number(event.base_length)) || !Number.isSafeInteger(Number(event.end_length)) || event.content_ref.readable_end !== Number(event.end_length) || Number(event.end_length) < Number(event.base_length)) throw new Error('Invalid content frontier.');
    const blocks = attempt.in_flight.blocks ?? [], existing = blocks.find(block => block.block_index === event.block_index);
    const text = existing && 'text' in existing ? existing.text : existing && 'arguments' in existing ? existing.arguments : foreground?.state.arguments ?? '';
    const loaded = new TextEncoder().encode(text).length === Number(event.base_length);
    const inline = loaded && event.inline_delta != null && new TextEncoder().encode(text + event.inline_delta).length <= 16_384 ? text + event.inline_delta : '';
    const block = event.kind === 'tool_call_arguments' ? existing && existing.type === 'tool_call' ? { ...existing, arguments: inline } : undefined
      : { type: event.kind as 'text' | 'reasoning' | 'refusal', block_index: event.block_index, text: inline };
    if (!block && loadedBlock) throw new Error('Tool argument advancement has no assembled owner.');
    next = { ...state, attempt: { ...attempt, in_flight: { ...attempt.in_flight, blocks: loadedBlock && block ? place(blocks, block, Number(event.position), value => String(value.block_index)) : blocks },
      foreground: event.kind === 'tool_call_arguments' ? (attempt.foreground ?? []).map(tool => tool.message_id === event.message_id && tool.block_index === event.block_index ? { ...tool, state: { ...tool.state, arguments: inline } } : tool) : attempt.foreground } };
    const updated = new Map(metadata);
    const index = next.attempt!.in_flight!.blocks!.findIndex(value => value.block_index === event.block_index);
    if (loadedBlock || foreground) updated.set(key, { path: loadedBlock ? ['attempt', 'in_flight', 'blocks', String(index), event.kind === 'tool_call_arguments' ? 'arguments' : 'text'] : ['attempt', 'foreground', String(next.attempt!.foreground!.findIndex(tool => tool.message_id === event.message_id && tool.block_index === event.block_index)), 'state', 'arguments'], content_ref: event.content_ref, deferred: new TextEncoder().encode(inline).length !== Number(event.end_length) });
    const cut = cuts.get(state); if (cut) cuts.set(next, cut);
    frontiers.set(next, updated);
    return next;
  }
  next = foldOrdinaryEvent(state, event);
  const cut = cuts.get(state); if (cut) cuts.set(next, cut);
  // A settled publication owns no live byte cache. Replay may still carry its
  // expired locator and fold it without reading those bytes.
  const retired = (binding: ContentBinding): boolean => {
    const source = binding.content_ref.source;
    if (source.type !== 'live_publication_prefix') return false;
    if (event.type === 'attempt_started') return true;
    if (event.type === 'attempt_settled') return source.attempt_id === event.attempt_id;
    if (event.type === 'message_committed') return source.message_id === event.message.id;
    if (event.type === 'message_descriptor_committed') return event.descriptor.identity === source.message_id;
    if (event.type === 'assistant_publication_settled' || event.type === 'assistant_publication_deferred_settled') {
      const audit = event.type === 'assistant_publication_settled' ? event.audit : event.settlement;
      return audit.attempt_id === source.attempt_id && audit.message_id === source.message_id
        && audit.stream_id === source.stream_id && audit.turn_id === source.turn_id
        && audit.request_id === source.publication_request_id;
    }
    return false;
  };
  const retained = [...metadata].filter(([, binding]) => !retired(binding));
  frontiers.set(next, retained.length === metadata.size ? metadata : new Map(retained));
  return next;
}

const upsert = <T>(rows: readonly T[] | undefined, value: T, key: (row: T) => string): T[] => {
  const index = (rows ?? []).findIndex(row => key(row) === key(value));
  return index < 0 ? [...rows ?? [], value] : rows!.map((row, i) => i === index ? value : row);
};
/** Position is the native creation ordinal, independent of provider block IDs. */
const place = <T>(rows: readonly T[], value: T, position: number, key: (row: T) => string): T[] => {
  if (!Number.isSafeInteger(position) || position < 0) throw new Error('Invalid native collection position.');
  if (position >= 32) return [...rows];
  const next = rows.filter(row => key(row) !== key(value));
  next.splice(Math.min(position, next.length), 0, value);
  return next.slice(0, 32);
};
const interactionKey = (ref: { conversation_id: string; interaction_id: string }) => JSON.stringify([ref.conversation_id, ref.interaction_id]);

function foldOrdinaryEvent(state: Snapshot, event: Exclude<Event, {type: 'projection_transition' | 'checkpoint_advanced' | 'block_content_advanced'}>): Snapshot {
  const attempt = state.attempt;
  const withAttempt = (patch: Partial<NonNullable<Snapshot['attempt']>>) => attempt && attempt.attempt_id != null && 'attempt_id' in event && attempt.attempt_id === event.attempt_id
    ? { ...state, attempt: { ...attempt, ...patch } } : state;
  const withBlocks = (update: (blocks: InFlightBlock[]) => InFlightBlock[]) => attempt?.in_flight && 'message_id' in event && attempt.in_flight.message_id === event.message_id
    ? withAttempt({ in_flight: { ...attempt.in_flight, blocks: update(attempt.in_flight.blocks ?? []) } }) : state;
  switch (event.type) {
    case 'trace_changed': return state; // Separate read domain; cursor still advances.
    case 'read_domains_updated': return { ...state, transcript: event.transcript, trace: event.trace, todos: event.todos, context: { ...state.context!, occupancy: event.occupancy ?? undefined, projection_head: event.projection_head ?? undefined } };
    case 'goal_changed': return { ...state, goal: event.view };
    case 'workflows_updated': return { ...state, workflows: event.workflows };
    case 'attempt_started': return { ...state, attempt: { attempt_id: event.attempt_id, attempt_id_source: null, phase: { type: 'running' }, turn: 0, model: event.model, execution_settings: event.execution_settings, last_usage: undefined, in_flight: undefined, foreground: [] } };
    case 'attempt_settled': return { ...withAttempt({ phase: { type: 'settled', outcome: event.outcome } }), context: { ...state.context!, compaction_in_progress: false } };
    case 'attempt_turn_updated': return withAttempt({ turn: event.turn });
    case 'attempt_usage_updated': return withAttempt({ last_usage: event.usage });
    case 'assistant_message_started':
      return state.messages.some(message => message.id === event.message_id) ? state : withAttempt({ in_flight: { message_id: event.message_id, blocks: [] }, foreground: [] });
    case 'tool_call_started': {
      if (!attempt?.in_flight || attempt.in_flight.message_id !== event.message_id) return state;
      const next = withBlocks(blocks => place(blocks, { type: 'tool_call', block_index: event.block_index, call_id: event.call.id??null, tool_id: event.call.tool_id??null, name: event.call.name, metadata_ref:event.call.metadata_ref, arguments: '' } as InFlightBlock, Number(event.position), block => String(block.block_index)));
      return { ...next, attempt: { ...next.attempt!, foreground: place(attempt.foreground ?? [], { message_id: event.message_id, block_index: event.block_index, occurrence:{locator:{type:'by_message_id',message_id:event.message_id},block_index:event.block_index}, call_id: event.call.id??null, tool_id: event.call.tool_id??null, name: event.call.name, metadata_ref:event.call.metadata_ref, assembly_complete:false, committed_result_deferred:false, state: { type: 'assembled', arguments: '' } } as ForegroundToolExecution, Number(event.foreground_position), tool => String(tool.block_index)) } };
    }
    case 'tool_call_assembled': {
      if (!attempt?.in_flight || attempt.in_flight.message_id !== event.message_id) return state;
      const argumentsText = () => event.arguments_json;
      const next = withBlocks(blocks => blocks.map(block => block.type === 'tool_call' && block.block_index === event.block_index ? { ...block, call_id:event.call.id??null,tool_id:event.call.tool_id??null,name:event.call.name,metadata_ref:event.call.metadata_ref,arguments: argumentsText() } : block));
      return { ...next, attempt: { ...next.attempt!, foreground: (attempt.foreground ?? []).map(tool => tool.message_id===event.message_id&&tool.block_index===event.block_index ? { ...tool, call_id:event.call.id??null,tool_id:event.call.tool_id??null,name:event.call.name,metadata_ref:event.call.metadata_ref,assembly_complete:true,state: { ...tool.state, arguments: argumentsText() } } : tool) } };
    }
    case 'tool_execution_started': return refineToolObservations(state,event);
    case 'tool_execution_progress': return refineToolObservations(state,event);
    case 'tool_result_committed': return refineToolObservations(state,event);
    case 'tool_execution_settled': return refineToolObservations(state,event);
    case 'tool_occurrence_rebound': return refineToolObservations(state,event);
    case 'message_descriptor_committed': {
      const next=event.descriptor.role==='assistant'&&event.descriptor.identity===attempt?.in_flight?.message_id?withAttempt({in_flight:undefined}):state;
      return {...next,message_descriptors:event.head,messages:event.evicted_message_id==null?next.messages:next.messages.filter(message=>message.id!==event.evicted_message_id)};
    }
    case 'message_committed': {
      // Native canonical content always wins; a suffix never creates a seat.
      const next = event.message.role === 'assistant' && event.message.id === attempt?.in_flight?.message_id
        ? {...state,attempt:{...attempt,in_flight:undefined}} : state;
      // The loaded transcript window belongs to its captured native source cut.
      // Canonical commitment advances the display head; ReadDomainsUpdated alone
      // installs new window rows at a new cut.
      return { ...next, messages: upsert(state.messages.filter(message => message.id !== event.evicted_message_id), event.message, message => message.id) };
    }
    case 'assistant_publication_settled':
    case 'assistant_publication_deferred_settled': {
      const audit = event.type === 'assistant_publication_settled' ? event.audit : event.settlement;
      const next = {...state,publication_settlements:event.settlement_head};
      if (!attempt?.in_flight || attempt.attempt_id !== event.attempt_id
        || audit.attempt_id !== attempt.attempt_id || audit.message_id !== attempt.in_flight.message_id) return next;
      // Opened already establishes the publication identity at H=0, before
      // any payload binding exists. Retries may reuse the provisional Message.
      const opened = cuts.get(state)?.read_cut.live_publication?.source;
      if (opened?.type === 'live_publication_prefix' && opened.attempt_id === attempt.attempt_id
        && opened.message_id === attempt.in_flight.message_id
        && (opened.stream_id !== audit.stream_id || opened.turn_id !== audit.turn_id
          || opened.publication_request_id !== audit.request_id)) return next;
      const live = contentBindings(state).filter(binding => binding.content_ref.source.type === 'live_publication_prefix'
        && binding.content_ref.source.message_id === attempt.in_flight!.message_id);
      if (live.some(binding => {
        const source = binding.content_ref.source;
        return source.type === 'live_publication_prefix' && (source.stream_id !== audit.stream_id
          || source.turn_id !== audit.turn_id || source.publication_request_id !== audit.request_id);
      })) return next;
      return {...next,attempt:{...attempt,in_flight:undefined}};
    }
    // The native owner supplies a complete merged bounded head, including
    // rows exposed when another owner's pending request settles.
    case 'pending_head_changed': return {...state,pending_interactions:event.head};
    case 'interaction_pending': return { ...state, pending_interactions: upsert(state.pending_interactions, event.interaction, row => interactionKey(row.interaction)) };
    case 'interaction_settled':
    case 'interaction_removed': return { ...state, pending_interactions: (state.pending_interactions ?? []).filter(row => interactionKey(row.interaction) !== interactionKey(event.interaction)) };
    // Durable audit/response decorations arrive in the native read-domain cut.
    case 'interaction_audit_requested':
    case 'interaction_audit_settled': return state;
    case 'context_compaction_started': return { ...state, context: event.context };
    case 'context_compaction_failed': return { ...state, context: event.context };
    case 'context_compacted': return { ...state, context: event.context };
    case 'agent_status_composed': return { ...state, statuses: upsert((state.statuses ?? []).filter(status => !(event.evicted_status_message_id != null && status.status_message_id === event.evicted_status_message_id) && !(event.evicted_status_locator != null && status.source_locator != null && sameCanonicalLocator(status.source_locator, event.evicted_status_locator))), event.status, statusIdentity) };
    case 'pending_inbound_changed': return { ...state, inbound: { ...state.inbound, pending: event.pending } };
    case 'inbound_enqueued': {
      const existing = state.inbound.pending?.find(item => item.sequence === event.sequence);
      if (existing) {
        if (existing.message.id !== event.message.id) throw new Error('Pending inbound occurrence identity changed.');
        return state;
      }
      return { ...state, inbound: { ...state.inbound, pending: [...state.inbound.pending ?? [], { sequence: event.sequence, revision: "0", message: event.message }] } };
    }
    case 'inbound_drained': return { ...state, inbound: { pending: (state.inbound.pending ?? []).filter(item => BigInt(item.sequence) > BigInt(event.watermark)), last_drain: { watermark: event.watermark, count: event.count } } };
    case 'job_updated': {
      if(Number(event.position)>=32)return state;
      const head=(state.jobs??[]).filter(row=>row.job_id!==event.evicted_job_id);
      const exists=head.some(row=>row.job_id===event.job.job_id);
      return {...state,jobs:exists?head.map(row=>row.job_id===event.job.job_id?event.job:row):[event.job,...head]};
    }
    case 'agent_updated': {
      if(Number(event.position)>=32)return state;
      const head=(state.agents??[]).filter(row=>row.agent_id!==event.evicted_agent_id);
      const exists=head.some(row=>row.agent_id===event.agent.agent_id);
      return {...state,agents:exists?head.map(row=>row.agent_id===event.agent.agent_id?event.agent:row):[event.agent,...head]};
    }
    // Native collection evidence is installed by the complete outer transition.
    case 'collection_changed': return {...state};
    case 'capability_updated': return { ...state, capabilities: event.capabilities };
    case 'resource_generation_updated': return { ...state, effective_plugins: event.plugins, model: event.model, model_status: event.model_status, effective_approval_mode: event.approval_mode, capabilities: event.capabilities, resources: event.resources };
    case 'session_model_changed': return { ...state, model: event.model, model_status: event.model_status };
    case 'runtime_shutdown': return { ...state, shutting_down: true };
    case 'runtime_durability_failed': return { ...state, durability_failure: { operation: event.operation, diagnostic: event.diagnostic } };
    // Runtime-domain advisory state; never a configuration-application change.
    case 'configuration_adoption_eligibility_changed': return { ...state, configuration_adoption_eligibility: event.eligibility };
    default: { const exhaustive: never = event; throw new Error(`Unknown Runtime Client event: ${exhaustive}`); }
  }
}
