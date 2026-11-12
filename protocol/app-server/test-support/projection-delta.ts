/** Independent full-event oracle used by both reference clients' tests. */
import type {RuntimeClientCheckpoint, RuntimeClientEvent, SnapshotDelta, TranscriptDelta} from '../v79.ts';

export function listDelta<T>(base: readonly T[], next: readonly T[]) {
  const values: T[] = [];
  const used = new Set<number>();
  const slots = next.map(value => {
    const prior = base.findIndex((row, index) => !used.has(index) && JSON.stringify(row) === JSON.stringify(value));
    if (prior >= 0) { used.add(prior); return prior; }
    values.push(value);
    return base.length + values.length - 1;
  });
  return {slots, values};
}

function transcriptDelta(base:RuntimeClientCheckpoint['snapshot']['transcript'], next:RuntimeClientCheckpoint['snapshot']['transcript']):TranscriptDelta {
  const {entries=[],content=[],collections=[],deferred_entries=[],...page}=next;
  return {page:{...page,entries:[]},entries:listDelta(base.entries??[],entries),content:listDelta(base.content??[],content),collections:listDelta(base.collections??[],collections),deferred_entries:listDelta(base.deferred_entries??[],deferred_entries)};
}
function snapshotDelta(base:RuntimeClientCheckpoint['snapshot'], next:RuntimeClientCheckpoint['snapshot']):SnapshotDelta {
  const edit: Record<string,unknown>={};
  const lists=['messages','message_descriptors','publication_settlements','trace_updates','pending_interactions','jobs','agents','statuses'];
  for(const key of Object.keys({...base,...next})) {
    const a=(base as unknown as Record<string,unknown>)[key],b=(next as unknown as Record<string,unknown>)[key];
    if(JSON.stringify(a)===JSON.stringify(b))continue;
    edit[key]=key==='transcript'?transcriptDelta(base.transcript,next.transcript):lists.includes(key)?listDelta((a??[])as unknown[],(b??[])as unknown[]):{value:b??null};
  }
  return edit as SnapshotDelta;
}
export type LiteralBase = {transcript: RuntimeClientCheckpoint['snapshot']['transcript']; descriptors: RuntimeClientCheckpoint['snapshot']['message_descriptors']; checkpoint:RuntimeClientCheckpoint['snapshot']};
export function initialLiterals(base: RuntimeClientCheckpoint): LiteralBase {return {transcript:base.snapshot.transcript,descriptors:base.snapshot.message_descriptors,checkpoint:base.snapshot};}
export function advanceLiterals(base:LiteralBase,event:RuntimeClientEvent):LiteralBase {
  let next=base;
  for(const fact of event.type==='projection_transition'?event.events:[event]) {
    if(fact.type==='checkpoint_advanced') next=initialLiterals(fact.checkpoint);
    else if(fact.type==='read_domains_updated') next={...next,transcript:fact.transcript};
    else if(fact.type==='message_descriptor_committed') next={...next,descriptors:fact.head};
  }
  return next;
}
export function deltaEvent(base: RuntimeClientCheckpoint, event: RuntimeClientEvent, literal=initialLiterals(base)): RuntimeClientEvent {
  if (event.type !== 'projection_transition') return event;
  const events = event.events.map((fact): RuntimeClientEvent => {
    if (fact.type === 'read_domains_updated') {
      const {entries = [], content = [], collections = [], deferred_entries = [], ...page} = fact.transcript;
      return {...fact, type: 'read_domains_patched', transcript: {
        page: {...page, entries: []},
        entries: listDelta(literal.transcript.entries ?? [], entries),
        content: listDelta(literal.transcript.content ?? [], content),
        collections: listDelta(literal.transcript.collections ?? [], collections),
        deferred_entries: listDelta(literal.transcript.deferred_entries ?? [], deferred_entries),
      }};
    }
    if(fact.type==='checkpoint_advanced') return {...fact,type:'checkpoint_patched',checkpoint:{...fact.checkpoint,snapshot:snapshotDelta(literal.checkpoint,fact.checkpoint.snapshot)}};
    if (fact.type === 'message_descriptor_committed') return {...fact, type: 'message_descriptor_patched', head: listDelta([...literal.descriptors, fact.descriptor], fact.head)};
    return fact;
  });
  return {type: 'projection_delta', events, metadata: {
    base_cursor: base.cursor, read_cut: {
      conversation_id:event.metadata.read_cut.conversation_id, activation_id:event.metadata.read_cut.activation_id,
      revision:event.metadata.read_cut.revision, through:event.metadata.read_cut.through,
      source_cut:{value:event.metadata.read_cut.source_cut}, live_publication:{value:event.metadata.read_cut.live_publication},
      canonical_foreground:{value:event.metadata.read_cut.canonical_foreground},
      domain_revisions:{removed:Object.keys(base.read_cut.domain_revisions).filter(key=>!(key in event.metadata.read_cut.domain_revisions)),values:event.metadata.read_cut.domain_revisions},
      collection_cuts:{removed:Object.keys(base.read_cut.collection_cuts).filter(key=>!(key in event.metadata.read_cut.collection_cuts)),values:event.metadata.read_cut.collection_cuts},
    },
    content: listDelta(base.content, event.metadata.content),
    collections: listDelta(base.collections, event.metadata.collections),
  }};
}
