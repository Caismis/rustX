import type {TraceFieldSource,TraceLocation,RuntimeClientToolOccurrence} from './v57.ts';
import {attemptIdentitySourceKey} from './attempt-identity.ts';
import {canonicalLocatorKey,contentRefKey} from './content-identity.ts';

/** Source selectors remain closed identity evidence; loading bytes never replaces them. */
export function traceFieldKey(source:TraceFieldSource):string {
 switch(source.type) {
  case 'value':return JSON.stringify([source.type,source.field,source.domain,source.item_offset,source.path,source.encoded_bytes]);
  case 'content':return JSON.stringify([source.type,source.field,source.binding.path,source.binding.source_path,contentRefKey(source.binding.content_ref)]);
 }
}
/** The Trace cache supplies conversation isolation; a missing owner does not group rows. */
export function traceAttemptKey(location:Pick<TraceLocation,'attempt_id'|'attempt_id_source'>):string|undefined {
 if(location.attempt_id!=null)return JSON.stringify(['inline',location.attempt_id]);
 if(location.attempt_id_source)return JSON.stringify(['source',attemptIdentitySourceKey(location.attempt_id_source)]);
}
export function traceStepKey(location:TraceLocation):string|undefined {
 if(location.step_id!=null)return JSON.stringify(['inline',location.step_id]);
 if(location.step_id_source)return JSON.stringify(['source',traceFieldKey(location.step_id_source)]);
}
export function sameTraceOwner(left:TraceLocation,right:TraceLocation):boolean {
 const owner=traceAttemptKey(left);return owner!=null&&owner===traceAttemptKey(right);
}
export function sameTraceStep(left:TraceLocation,right:TraceLocation):boolean {
 const step=traceStepKey(left);return sameTraceOwner(left,right)&&step!=null&&step===traceStepKey(right);
}
export function traceOccurrenceKey(occurrence:RuntimeClientToolOccurrence):string {
 return JSON.stringify([canonicalLocatorKey(occurrence.locator),occurrence.block_index]);
}

export function traceFields(record:import('./v57.ts').TraceRecord):TraceFieldSource[] {
 const request=record.request;
 const fields=[...record.fields,record.location.step_id_source,...(record.tool?.fields??[]),
  ...record.calls.flatMap(call=>call.fields),request?.request_id_source,request?.assistant_message_id_source,request?.model_source,
  ...(request?.context_additions.flatMap(context=>context.fields)??[])].filter((field):field is TraceFieldSource=>field!=null);
 return [...new Map(fields.map(field=>[traceFieldKey(field),field])).values()];
}

/** Only canonical Message metadata proves an accepted Message locator. */
export function traceCanonicalLocator(record:import('./v57.ts').TraceRecord) {
 for(const field of record.fields){
  if(field.field!=='message_id'||field.type!=='content')continue;
  const source=field.binding.content_ref.source;
  if(source.type==='canonical_message_block'&&source.component==='message_metadata')return source.locator;
 }
}
export function traceRequestAccepts(request:import('./v57.ts').TraceRecord,assistant:import('./v57.ts').TraceRecord):boolean {
 if(!request.request||assistant.kind!=='assistant')return false;
 const accepted=request.request.accepted_message_locator,canonical=traceCanonicalLocator(assistant);
 if(accepted&&canonical)return canonicalLocatorKey(accepted)===canonicalLocatorKey(canonical);
 return request.request.assistant_message_id!=null&&assistant.message_id!=null&&request.request.assistant_message_id===assistant.message_id;
}
