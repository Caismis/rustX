import {expect,it} from 'vitest';
import type {CheckpointReadCut,TraceLifecycle,TraceRecord} from '../../protocol/app-server/v51';
import {traceAttemptKey,traceStepKey} from '../../protocol/app-server/trace-identity';
import {beginTraceDetail,refreshTrace,replaceTrace,selectTrace} from '../src/client/trace';
import {modelRetryPlacements} from '../src/bindings/model-retry';
import {traceRecord,traceTool} from './trace-fixture';

const cut=(revision:string):CheckpointReadCut=>({conversation_id:'original',revision,through:'10',domain_revisions:{},collection_cuts:{}});
const lifecycle=(r:TraceRecord):TraceLifecycle=>({id:r.id,location:r.location,fields:r.fields,collections:r.collections,state:r.state,timing:r.timing,message_id:r.message_id,attachments:r.attachments,truncated:r.truncated,
 request:r.request?{failure_kind:r.request.failure_kind,usage:r.request.usage,generation:r.request.generation}:null,
 tool:r.tool?{started:r.tool.started,outcome:r.tool.outcome,detail:r.tool.detail}:null});

it('unchanged repairs retain projection inputs while advancing cuts and preserving the admitted detail',()=>{
 const record=traceRecord(1),page={read_cut:cut('1'),records:[record],next_cursor:null};
 const before=beginTraceDetail(selectTrace(replaceTrace(page),record.id),record.id);
 const incoming=structuredClone(record),next=refreshTrace(before,{read_cut:cut('2'),records:[incoming],next_cursor:null},[lifecycle(incoming)]);
 expect(next.page.records).toBe(before.page.records);
 expect(next.page.records[0]).toBe(record);
 expect(next.readCuts?.[record.id]).toEqual(cut('2'));
 expect(next.selection).toBe(before.selection);
 expect(next.details[record.id]).toBe(before.details[record.id]);
 expect(next.details[record.id].loading).toBe(true);
 expect(next.epoch).toBe(before.epoch);
});

it('a canonical-only result binding still changes a completed tool row and its field cut',()=>{
 const record=traceTool(1,{message_id:null}),before=replaceTrace({read_cut:cut('1'),records:[record],next_cursor:null});
 const current:TraceRecord={...record,message_id:'canonical-tool-result',fields:[{type:'value',field:'message_id',domain:'messages.0',item_offset:0,path:['message_id'],encoded_bytes:'23'}]};
 const next=refreshTrace(before,{read_cut:cut('2'),records:[structuredClone(record)]},[lifecycle(current)],undefined,true);
 expect(next.page.records).not.toBe(before.page.records);
 expect(next.page.records[0]).toMatchObject({state:'completed',message_id:'canonical-tool-result',fields:current.fields});
 expect(next.readCuts?.[record.id]).toEqual(cut('2'));
 expect(before.page.records[0].message_id).toBeNull();
});

it('an unknown tool outcome stays unknown until the native lifecycle supplies a positive outcome',()=>{
 const record=traceTool(1,{state:'outcome_unknown'});record.tool!.outcome='outcome_unknown';
 const before=replaceTrace({read_cut:cut('1'),records:[record]});
 const unchanged=refreshTrace(before,{read_cut:cut('2'),records:[structuredClone(record)]},[lifecycle(record)],undefined,true);
 expect(unchanged.page.records).toBe(before.page.records);
 expect(unchanged.page.records[0].tool?.outcome).toBe('outcome_unknown');
 const settled={...record,state:'completed' as const,tool:{...record.tool!,outcome:'success' as const}};
 const next=refreshTrace(unchanged,{read_cut:cut('3'),records:[]},[lifecycle(settled)],undefined,true);
 expect(next.page.records[0]).toMatchObject({state:'completed',tool:{outcome:'success'}});
 expect(next.page.records).not.toBe(unchanged.page.records);
});

it('inline key reuse observes changed aliases and never caches a deferred Source identity',()=>{
 const location=traceRecord(1).location;
 const first=traceAttemptKey(location),step=traceStepKey(location);
 expect(traceAttemptKey(location)).toBe(first);expect(traceStepKey(location)).toBe(step);
 location.attempt_id='attempt-b';location.step_id='step-b';
 expect(traceAttemptKey(location)).not.toBe(first);expect(traceStepKey(location)).not.toBe(step);
 location.attempt_id=null;location.attempt_id_source={type:'journal',domain:'journal.1',field:{sequence:1,path:['attempt_id'],encoded_bytes:20}};
 const deferred=traceAttemptKey(location);location.attempt_id_source.field.sequence=2;
 expect(traceAttemptKey(location)).not.toBe(deferred);
});

it('one grouped retry pass keeps native owners, predecessor matching and placement order',()=>{
 const a=traceRecord(1,{state:'failed'}),b=traceRecord(2,{state:'failed',location:{...a.location,attempt_id:'attempt-b'}}),retry=traceRecord(3,{state:'running'});
 a.request!.failure_kind='timeout';b.request!.failure_kind='timeout';
 retry.request!.previous_failure_kind='timeout';retry.request!.predecessor={availability:'available',request_id:a.request!.request_id,request_id_source:null};
 const clipped=traceRecord(4,{location:{...a.location,step_id:'later-step'},request:null});
 const records=Object.freeze([a,b,retry,clipped]);
 const result=modelRetryPlacements(records,new Set([retry.request!.assistant_message_id!]));
 expect(result).toHaveLength(2);
 expect(result[0]).toMatchObject({beforeMessageId:retry.request!.assistant_message_id,notice:{request:{id:retry.id},failure:{id:a.id}}});
 expect(result[0].notice.owner).toBe(clipped.location);
 expect(result[1].notice.failure?.id).toBe(b.id);
 expect(result[1].notice.owner).toBe(b.location);
});
