import assert from 'node:assert/strict';
import { it } from 'node:test';
import { ServerCallbacks } from '../../protocol/app-server/callbacks.ts';
const thread = 'conv_01900000-0000-7000-8000-000000000002';
const request = (id: string, namespace = 'business') => ({jsonrpc:'2.0',id,method:'item/tool/call',params:{threadId:thread,turnId:'attempt-1',callId:'call-1',namespace,tool:'lookup',arguments:{key:'invoice'}}});
const resolved = (id: string) => ({jsonrpc:'2.0',method:'serverRequest/resolved',params:{requestId:id,threadId:thread}});
const flush = async () => { for(let i=0;i<8;i++)await Promise.resolve(); };
it('generic clients ignore unhandled tools and full namespaces select explicit handlers', async () => {
  const replies:unknown[]=[];const callbacks=new ServerCallbacks(async reply=>{replies.push(reply);});let handled=0;
  callbacks.installHandler('business','lookup',async call=>{handled++;assert.equal(call.namespace,'business');return {success:true,contentItems:[{type:'inputText',text:'done'}]};});
  callbacks.receive(request('other','inventory'));callbacks.receive(request('one'));callbacks.receive(request('one'));
  await flush();assert.equal(handled,1);assert.equal(replies.length,1);assert.equal((replies[0] as {id:string}).id,'one');
  callbacks.receive(resolved('one'));callbacks.receive(resolved('other'));callbacks.clear();
});
it('resolution before the handler microtask prevents an external start', async () => {
  const callbacks=new ServerCallbacks(async()=>assert.fail('no reply after resolution'));let handled=0;
  callbacks.installHandler('business','lookup',async()=>{handled++;return {success:true,contentItems:[]};});
  callbacks.receive(request('one'));callbacks.receive(resolved('one'));await flush();assert.equal(handled,0);
});
it('disconnect and replay fence a previous handler result by entry identity', async () => {
  const replies:unknown[]=[];const completions:Array<(value:{success:boolean;contentItems:[]})=>void>=[];const signals:AbortSignal[]=[];
  const callbacks=new ServerCallbacks(async value=>{replies.push(value);});
  callbacks.installHandler('business','lookup',async(_,signal)=>{signals.push(signal);return new Promise(resolve=>completions.push(resolve));});
  callbacks.receive(request('one'));await flush();callbacks.clear();assert.equal(signals[0]!.aborted,true);
  callbacks.receive(request('one'));await flush();completions[0]!({success:true,contentItems:[]});await flush();assert.equal(replies.length,0);
  completions[1]!({success:true,contentItems:[]});await flush();assert.equal(replies.length,1);
  callbacks.receive(resolved('one'));assert.equal(signals[1]!.aborted,true);
});
it('invalid envelopes and wrong-thread resolutions cannot clear a pending request', async () => {
  const callbacks=new ServerCallbacks(async()=>{});callbacks.receive(request('one'));
  assert.throws(()=>callbacks.receive({...resolved('one'),params:{requestId:'one',threadId:'conv_01900000-0000-7000-8000-000000000003'}}),/Mismatched/);
  assert.throws(()=>callbacks.receive({...request('bad'),id:Number.MAX_SAFE_INTEGER+1}),/Invalid/);
  assert.throws(()=>callbacks.receive({...request('bad'),params:[]}),/Invalid/);
  callbacks.receive(resolved('one'));
});
