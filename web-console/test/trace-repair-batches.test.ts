import {afterEach,expect,it} from 'vitest';
import type {Request,TraceLifecycle,TraceRecord} from '../../protocol/app-server/v52';
import {Server,checkpoint,snapshot,traceReceipt} from './fixture';
import {requestDetail,traceRecord,traceTool} from './trace-fixture';

let server:Server;
afterEach(()=>server?.client.disconnect());
const cut=(revision='0')=>({...checkpoint(snapshot()).read_cut,revision,through:revision});
function lifecycle(record:TraceRecord):TraceLifecycle {
 return {id:record.id,location:record.location,fields:record.fields,collections:record.collections,state:record.state,timing:record.timing,
  message_id:record.message_id,attachments:record.attachments,truncated:record.truncated,
  request:record.request?{failure_kind:record.request.failure_kind,usage:record.request.usage,generation:record.request.generation}:null,
  tool:record.tool?{started:record.tool.started,outcome:record.tool.outcome,detail:record.tool.detail}:null};
}
function emit(cursor:string){
 const published=checkpoint(snapshot(),cursor);
 server.socket.deliver({jsonrpc:'2.0',method:'session/event',params:{target:server.target('A'),cursor,event:{type:'projection_transition',events:[{type:'trace_changed'}],metadata:{read_cut:published.read_cut,content:published.content,collections:published.collections}}}});
}
const traceRequests=()=>server.requests.filter(({request})=>request.method==='session/trace').map(({request})=>request as Extract<Request,{method:'session/trace'}>);

async function loadedWindow(){
 server=new Server();
 const records=Array.from({length:288},(_,index)=>traceRecord(index+1));
 records[0]=traceTool(1,{message_id:null});
 records[2]=traceTool(3,{state:'outcome_unknown'});records[2].tool!.outcome='outcome_unknown';
 server.snapshots.set('A',{...snapshot(),trace:{read_cut:cut(),records:records.slice(-32),next_cursor:'trace:257'}});
 server.handlers.set('session/trace',request=>{
  if(request.method!=='session/trace')throw new Error('Trace request');
  const end=request.params.before?records.findIndex(record=>record.position===request.params.before):records.length;
  const start=Math.max(0,end-request.params.limit),interests=request.params.records??[];
  return {type:'trace',refreshed:interests.length,page:{read_cut:cut(),records:records.slice(start,end),next_cursor:start?records[start].position:null,
   updates:interests.flatMap(position=>{const record=records.find(record=>record.position===position);return record?[lifecycle(record)]:[];})}};
 });
 await server.attached('A');
 for(let index=0;index<8;index++)await server.client.loadEarlierTrace('A');
 expect(server.client.getSnapshot().views.A.trace?.page.records).toHaveLength(288);
 return records;
}

it('bounded prefix replies cover the whole loaded window, late canonical bindings and held original detail',async()=>{
 const records=await loadedWindow(),start=traceRequests().length,examined:string[]=[];
 server.client.selectTrace('A','trace:2');server.held.add('session/traceDetail');
 const detail=server.client.loadTraceDetail('A','trace:2'),detailRequest=await server.waitFor('session/traceDetail',1);
 const before=server.client.getSnapshot().views.A.trace!;
 records[0]={...records[0],message_id:'late-canonical-result',fields:[{type:'value',field:'message_id',domain:'messages.1',item_offset:0,path:['message_id'],encoded_bytes:'23'}]};
 server.handlers.set('session/trace',request=>{
  if(request.method!=='session/trace')throw new Error('Trace request');
  const supplied=request.params.records??[],prefix=supplied.slice(0,64);examined.push(...prefix);
  return {type:'trace',refreshed:prefix.length,page:{read_cut:cut('1'),records:records.slice(-request.params.limit),next_cursor:'trace:257',
   updates:prefix.flatMap(position=>{const record=records.find(record=>record.position===position);return record?[lifecycle(record)]:[];})}};
 });
 emit('1');await server.client['traceReads'].get('A')!.work;
 const requests=traceRequests().slice(start),after=server.client.getSnapshot().views.A.trace!;
 expect(requests).toHaveLength(5);
 expect(requests.every(request=>request.params.records!.length<=128)).toBe(true);
 expect(examined).toHaveLength(288);expect(new Set(examined)).toEqual(new Set(records.map(record=>record.position)));
 expect(after.page.records[0]).toMatchObject({state:'completed',message_id:'late-canonical-result',fields:records[0].fields});
 expect(after.page.records[2]).toMatchObject({state:'outcome_unknown',tool:{outcome:'outcome_unknown'}});
 expect(after.readCuts?.['trace:1']).toEqual(cut('1'));
 expect(after.details['trace:2']).toBe(before.details['trace:2']);expect(after.epoch).toBe(before.epoch);
 server.socket.success(detailRequest,traceReceipt(requestDetail(2)));await detail;
 expect(server.client.getSnapshot().views.A.trace?.details['trace:2'].detail?.id).toBe('trace:2');
 expect(server.requests.filter(({request})=>request.method==='session/checkpoint')).toHaveLength(1);
 expect(server.requests.filter(({request})=>request.method==='session/subscribe')).toHaveLength(1);
});

it('a later prefix failure stays visible and a later successful repair clears it',async()=>{
 const records=await loadedWindow();let reads=0;
 server.handlers.set('session/trace',request=>{
  if(request.method!=='session/trace')throw new Error('Trace request');
  if(++reads===2)throw new Error('Use the explicitly held failure below');
  const prefix=(request.params.records??[]).slice(0,8);
  return {type:'trace',refreshed:prefix.length,page:{read_cut:cut('1'),records:records.slice(-request.params.limit),updates:prefix.flatMap(position=>records.filter(record=>record.position===position).map(lifecycle))}};
 });
 server.held.add('session/trace');const start=traceRequests().length;emit('1');
 const work=server.client['traceReads'].get('A')!.work.catch(()=>{}),first=await server.waitFor('session/trace',start+1);
 server.reply(first);const second=await server.waitFor('session/trace',start+2);
 server.socket.deliver({jsonrpc:'2.0',id:second.id,error:{code:-32000,message:'Later Trace prefix failed',data:{kind:'operation_failed'}}});await work;
 expect(server.client.getSnapshot().views.A.trace?.error).toContain('Later Trace prefix failed');
 server.handlers.set('session/trace',request=>{
  if(request.method!=='session/trace')throw new Error('Trace request');
  const prefix=request.params.records??[];
  return {type:'trace',refreshed:prefix.length,page:{read_cut:cut('2'),records:records.slice(-request.params.limit),updates:prefix.flatMap(position=>records.filter(record=>record.position===position).map(lifecycle))}};
 });
 server.held.delete('session/trace');emit('2');await server.client['traceReads'].get('A')!.work;
 expect(server.client.getSnapshot().views.A.trace?.error).toBeUndefined();
 expect(server.client.getSnapshot().views.A.trace?.page.records).toHaveLength(288);
});

it('zero prefix progress fails visibly without issuing an unbounded retry loop',async()=>{
 const records=await loadedWindow(),start=traceRequests().length;
 server.handlers.set('session/trace',()=>({type:'trace',refreshed:0,page:{read_cut:cut('1'),records:records.slice(-32)}}));
 emit('1');await server.client['traceReads'].get('A')!.work.catch(()=>{});
 expect(traceRequests().length-start).toBe(1);
 expect(server.client.getSnapshot().views.A.trace?.error).toContain('Trace repair returned invalid progress');
 expect(server.client.getSnapshot().views.A.trace?.page.records).toHaveLength(288);
});
