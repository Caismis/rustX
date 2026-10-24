import {expect,it} from 'vitest';
import type {TraceFieldSource} from '../../protocol/app-server/v57';
import {readTraceFieldWindow} from '../../protocol/app-server/trace-field';
import {Server,checkpoint,snapshot} from './fixture';
import {traceRecord} from './trace-fixture';

it('old checkpoint then newer standalone Trace reads its selected original field at the returned cut',async()=>{
 const server=new Server(),old=checkpoint(snapshot(), '1').read_cut,newer={...old,revision:'5',through:'5'};
 const field:Extract<TraceFieldSource,{type:'value'}>={type:'value',field:'model',domain:'trace.records.5.detail.request.model',item_offset:0,path:[],encoded_bytes:'8'};
 const record=traceRecord(5);record.request!.model=null;record.request!.model_source=field;
 server.handlers.set('session/trace',()=>({type:'trace',page:{read_cut:newer,records:[record],next_cursor:null}}));
 server.handlers.set('conversation/checkpointValueRead',request=>{
  if(request.method!=='conversation/checkpointValueRead')throw Error('method');
  expect(request.params.cut).toEqual(newer);expect(request.params.cut).not.toEqual(old);expect(request.params.domain).toBe(field.domain);
  return {type:'checkpoint_value_chunk',chunk:{cut:newer,domain:field.domain,item_offset:0,path:[],offset:0,readable_end:8,next_offset:null,representation:'utf8_text',bytes_base64:btoa('original')}};
 });
 await server.attached('A');
 try{
  await server.client.returnToLatestTrace('A');
  const cache=server.client.getSnapshot().views.A.trace!,cut=cache.readCuts![record.id];
  const result=await readTraceFieldWindow(field,cut,0,at=>cache.fields!.value(cut,field,at,()=>true),()=>true);
  expect(new TextDecoder().decode(result.bytes)).toBe('original');expect(server.requests.filter(row=>row.request.method==='conversation/checkpointValueRead')).toHaveLength(1);
  await expect(cache.fields!.value({...cut,conversation_id:'foreign'},field,0,()=>true)).rejects.toThrow('source was replaced');
  expect(server.requests.filter(row=>row.request.method==='conversation/checkpointValueRead')).toHaveLength(1);
 }finally{server.client.disconnect();}
});
