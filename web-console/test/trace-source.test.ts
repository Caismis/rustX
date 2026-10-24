import {expect,it} from 'vitest';
import type {AttemptIdentitySource,CheckpointReadCut,TraceFieldSource} from '../../protocol/app-server/v57';
import {sameTraceOwner,sameTraceStep,traceAttemptKey,traceOccurrenceKey,traceRequestAccepts} from '../../protocol/app-server/trace-identity';
import {refreshTrace,replaceTrace,selectTrace,standaloneTraceCut,traceInterestBatches,inlineTraceDetail} from '../src/client/trace';
import {projectTrajectory} from '../src/app/trajectory/layout';
import {modelRetryNotices} from '../src/bindings/model-retry';
import {traceRecord} from './trace-fixture';
import {translator} from '../src/locale/translation';

const owner:AttemptIdentitySource={type:'journal',domain:'journal.1',field:{sequence:1,path:['attempt_id'],encoded_bytes:9000}};
const step:TraceFieldSource={type:'value',field:'step_id',domain:'journal.2',item_offset:0,path:['event','turn_id'],encoded_bytes:'9000'};
const location={attempt_id:null,attempt_id_source:owner,step_id:null,step_id_source:step};
const cut=(revision:string):CheckpointReadCut=>({conversation_id:'original',revision,through:revision,domain_revisions:{},collection_cuts:{}});
const tx=translator('en');

it('deferred original owners and steps group by exact immutable source while missing aliases never group',()=>{
 const a=traceRecord(1,{location}),b=traceRecord(2,{location:{...location,attempt_id_source:{field:{encoded_bytes:9000,path:['attempt_id'],sequence:1},type:'journal',domain:'journal.1'}}});
 expect(sameTraceOwner(a.location,b.location)).toBe(true);expect(sameTraceStep(a.location,b.location)).toBe(true);
 expect(projectTrajectory(tx,[a,b]).sections).toHaveLength(1);
 const missing={attempt_id:null,attempt_id_source:null,step_id:null,step_id_source:null};
 expect(sameTraceOwner(missing,missing)).toBe(false);expect(sameTraceStep(missing,missing)).toBe(false);
 expect(traceAttemptKey({...location,attempt_id_source:{...owner,domain:'journal.9'}})).not.toBe(traceAttemptKey(location));
 expect(traceOccurrenceKey({locator:{type:'by_ledger_position',position:9},block_index:0})).not.toBe(traceOccurrenceKey({locator:{type:'by_ledger_position',position:9},block_index:1}));
});
it('deferred retry predecessors reuse original request source and do not match unrelated null request aliases',()=>{
 const source:TraceFieldSource={type:'value',field:'request_id',domain:'journal.3',item_offset:0,path:['event','request_id'],encoded_bytes:'9000'};
 const first=traceRecord(3,{location}),retry=traceRecord(4,{location});
 first.request!.request_id=null;first.request!.request_id_source=source;first.request!.failure_kind='timeout';
 retry.request!.request_id=null;retry.request!.request_id_source={...source,domain:'journal.4'};retry.request!.previous_failure_kind='timeout';
 retry.request!.predecessor={availability:'available',request_id:null,request_id_source:source};
 expect(modelRetryNotices([first,retry],location)[0]?.failure).toBe(first);
 retry.request!.predecessor={availability:'available',request_id:null,request_id_source:{...source,domain:'journal.8'}};
 expect(modelRetryNotices([first,retry],location)[0]?.failure).toBeUndefined();
});
it('standalone page field cuts replace only represented rows; older selection keeps its original cut',()=>{
 const first=traceRecord(1),second=traceRecord(2),later=traceRecord(3);
 const initial=selectTrace(replaceTrace({read_cut:cut('2'),records:[first,second]}),first.id);
 const refreshed=refreshTrace(initial,{read_cut:cut('3'),records:[second,later]});
 expect(refreshed.readCuts?.[first.id]).toEqual(cut('2'));expect(refreshed.readCuts?.[later.id]).toEqual(cut('3'));
 expect(standaloneTraceCut({read_cut:cut('3'),records:[later]},'original')).toEqual(cut('3'));
 expect(()=>standaloneTraceCut({read_cut:null,records:[later]},'original')).toThrow('cut is unavailable');
 expect(()=>standaloneTraceCut({read_cut:{...cut('3'),activation_id:'foreign'},records:[later]},'original')).toThrow('cut is unavailable');
});
it('eight-interest batching covers all 512 rows plus an outside selection without erasing older cuts',()=>{
 const records=Array.from({length:512},(_,index)=>traceRecord(index)),selected=traceRecord(999);
 const cache={...replaceTrace({read_cut:cut('1'),records}),selection:selected,readCuts:{...replaceTrace({read_cut:cut('1'),records}).readCuts,[selected.id]:cut('0')}};
 const batches=traceInterestBatches(cache);expect(batches).toHaveLength(65);expect(batches.every(batch=>batch.length<=8)).toBe(true);
 expect(new Set(batches.flat()).size).toBe(513);expect(batches[0][0]).toBe(selected.position);
 const repaired=refreshTrace(cache,{read_cut:cut('2'),records:[records[0]]},[],undefined,true);
 expect(repaired.page.records.map(record=>record.id)).toEqual(records.map(record=>record.id));expect(repaired.readCuts?.[selected.id]).toEqual(cut('0'));expect(repaired.readCuts?.[records[0].id]).toEqual(cut('1'));
});
it('only actual accepted canonical locator links deferred provisional requests to Assistant messages',()=>{
 const request=traceRecord(1),assistant=traceRecord(2,{kind:'assistant',message_id:null,request:null});
 request.request!.assistant_message_id=null;request.request!.accepted_message_locator={type:'by_ledger_position',position:7};
 assistant.fields=[{type:'content',field:'message_id',binding:{deferred:true,path:['message_id'],source_path:['id'],content_ref:{conversation_id:'original',source:{type:'canonical_message_block',locator:{type:'by_ledger_position',position:7},block_index:0,component:'message_metadata'},representation:'canonical_json_v1',version:1,content_revision:1,readable_end:9000}}}];
 expect(traceRequestAccepts(request,assistant)).toBe(true);
 request.request!.accepted_message_locator={type:'by_ledger_position',position:8};expect(traceRequestAccepts(request,assistant)).toBe(false);
 assistant.fields=[];expect(traceRequestAccepts(request,assistant)).toBe(false);
});
it('deferred whole detail is never automatically materialized',()=>{
 const page={cursor:'1',domain:'trace.records.1.detail',read_cut:cut('1'),total:'1',items:[{offset:'0',identity:'trace:1',value:{id:'trace:1'},content:[],collections:[],deferred:true}],collections:[]};
 expect(inlineTraceDetail(page,'trace:1')).toBeUndefined();
});
