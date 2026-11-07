import assert from 'node:assert/strict';
import {test} from 'node:test';
import {summaryFor,threadFor} from '../../protocol/app-server/testing.ts';
import {AppServerClient} from '../src/app-server/client.ts';
import {AppServerHost} from '../src/app-server/host.ts';
import type {ThreadStatusEntry} from '../../protocol/app-server/v74.ts';
import {FakeTransport} from './support/app-server-peer.ts';
import {SERVER_CAPABILITIES} from './support/app-server-harness.ts';
import {sessionView} from './support/fixtures.ts';
async function connected(){const transport=new FakeTransport();const work=AppServerClient.initialize({transport});const init=(await transport.log.awaitMethod('initialize'))[0]!;transport.respond(init.id,{type:'initialized',authority_id:'status-authority',protocol_version:74,capabilities:SERVER_CAPABILITIES});const client=await work;return {transport,client,host:new AppServerHost({client,ownership:'external'})};}
const id=sessionView().active_conversation_id;
const entry=(revision:string,status:ThreadStatusEntry['status'],runtimeIncarnation:string|null='1'):ThreadStatusEntry=>({threadId:id,revision,status,runtimeIncarnation});
const row=()=>summaryFor({id:sessionView().id,cwd:'/work',name:'A',ownership_generation:'1',active_node:sessionView().active_node,updated_at:'2026-09-14T00:00:00Z'},id);
const changed=(h:Awaited<ReturnType<typeof connected>>,value:ThreadStatusEntry)=>h.transport.deliver({jsonrpc:'2.0',method:'thread/status/changed',params:value});
const resync=(h:Awaited<ReturnType<typeof connected>>)=>h.transport.deliver({jsonrpc:'2.0',method:'thread/status/resyncRequired',params:{}});
const readResult=(value:ThreadStatusEntry)=>({type:'thread' as const,thread:{...threadFor(sessionView({active_conversation_id:id})),status:value.status,statusRevision:value.revision,runtimeIncarnation:value.runtimeIncarnation}});
test('early status notification wins over a first list snapshot without attaching',async t=>{
 const h=await connected();t.after(()=>h.client.close());const listing=h.host.listSessions();const request=(await h.transport.log.awaitMethod('session/list'))[0]!;
 changed(h,entry('6',{type:'active',activeFlags:['waitingOnApproval','waitingOnUserInput']}));
 h.transport.respond(request.id,{type:'sessions',sessions:[{...row(),thread_status:entry('5',{type:'idle'})}],next_offset:null});await listing;
 assert.equal(h.host.threadStatusLabel(id),'Waiting for approval and user input');assert.equal(h.transport.log.count('thread/resume'),0);
 changed(h,entry('8',{type:'notLoaded'},null));changed(h,entry('7',{type:'active',activeFlags:[]}));assert.equal(h.host.threadStatusLabel(id),'Not loaded');
 changed(h,entry('9',{type:'active',activeFlags:[]},'2'));changed(h,entry('8',{type:'idle'}));assert.equal(h.host.threadStatus(id)?.runtimeIncarnation,'2');
});
test('second overflow while an exact read is held requires a later read before presenting status',async t=>{
 const h=await connected();t.after(()=>h.client.close());const listing=h.host.listSessions();const request=(await h.transport.log.awaitMethod('session/list'))[0]!;
 h.transport.respond(request.id,{type:'sessions',sessions:[{...row(),thread_status:entry('5',{type:'idle'})}],next_offset:null});await listing;
 resync(h);const first=(await h.transport.log.awaitMethod('thread/read'))[0]!;resync(h);
 h.transport.respond(first.id,readResult(entry('5',{type:'idle'})));const second=(await h.transport.log.awaitMethod('thread/read',2))[1]!;
 assert.equal(h.host.threadStatusLabel(id),'Status unavailable');
 const repaired=new Promise<void>(resolve=>{const stop=h.host.onThreadStatus(()=>{if(h.host.threadStatus(id)?.revision==='10'){stop();resolve();}});});
 h.transport.respond(second.id,readResult(entry('10',{type:'active',activeFlags:[]})));await repaired;assert.equal(h.host.threadStatusLabel(id),'Running');
 h.client.close();assert.equal(h.host.threadStatusLabel(id),'Status unavailable');
});
test('later catalog pages keep the exact status of earlier rows',async t=>{
 const h=await connected();t.after(()=>h.client.close());
 for(let page=0;page<2;page++){const listing=h.host.listSessions(undefined,page);const request=(await h.transport.log.awaitMethod('session/list',page+1))[page]!;const summary={...row(),id:page?'ses_113cba28-d6b2-70f4-8f62-8f8007e37ace':sessionView().id,thread_id:page?'conv_113cba28-d6b2-70f4-8f62-8f8007e37ace':id,thread_status:page?{...entry('6',{type:'systemError'}),threadId:'conv_113cba28-d6b2-70f4-8f62-8f8007e37ace'}:entry('5',{type:'idle'})};h.transport.respond(request.id,{type:'sessions',sessions:[summary],next_offset:page?null:1});await listing;}
 assert.equal(h.host.threadStatusLabel(id),'Idle');assert.equal(h.host.threadStatusLabel('conv_113cba28-d6b2-70f4-8f62-8f8007e37ace'),'System error');
});

test('an unknown Thread snapshot crossing a server gap requires a fresh exact read',async t=>{
 const h=await connected();t.after(()=>h.client.close());const listing=h.host.listSessions();const request=(await h.transport.log.awaitMethod('session/list'))[0]!;
 resync(h);
 h.transport.respond(request.id,{type:'sessions',sessions:[{...row(),thread_status:entry('5',{type:'idle'})}],next_offset:null});await listing;
 assert.equal(h.host.threadStatusLabel(id),'Status unavailable');
 const read=(await h.transport.log.awaitMethod('thread/read'))[0]!;
 const repaired=new Promise<void>(resolve=>{const stop=h.host.onThreadStatus(()=>{if(h.host.threadStatus(id)?.revision==='6'){stop();resolve();}});});
 h.transport.respond(read.id,readResult(entry('6',{type:'active',activeFlags:[]})));await repaired;
 assert.equal(h.host.threadStatusLabel(id),'Running');assert.equal(h.transport.log.count('thread/resume'),0);
});

test('one unavailable Thread does not poison other repairs and its later notice restores availability',async t=>{
 const h=await connected();t.after(()=>h.client.close());const other='conv_113cba28-d6b2-70f4-8f62-8f8007e37ace';
 const listing=h.host.listSessions();const request=(await h.transport.log.awaitMethod('session/list'))[0]!;
 h.transport.respond(request.id,{type:'sessions',sessions:[{...row(),thread_status:entry('5',{type:'idle'})},{...row(),id:'ses_113cba28-d6b2-70f4-8f62-8f8007e37ace',thread_id:other,thread_status:{...entry('5',{type:'idle'}),threadId:other}}],next_offset:null});await listing;
 resync(h);const first=(await h.transport.log.awaitMethod('thread/read'))[0]!;h.transport.respondError(first.id,{code:-32000,message:'source unavailable'});
 const second=(await h.transport.log.awaitMethod('thread/read',2))[1]!;
 const repaired=new Promise<void>(resolve=>{const stop=h.host.onThreadStatus(()=>{if(h.host.threadStatus(other)?.revision==='20'){stop();resolve();}});});
 const result=readResult({...entry('20',{type:'active',activeFlags:[]}),threadId:other});result.thread.id=other;
 h.transport.respond(second.id,result);await repaired;
 assert.equal(h.host.threadStatusLabel(id),'Status unavailable');assert.equal(h.host.threadStatusLabel(other),'Running');
 changed(h,entry('21',{type:'idle'}));assert.equal(h.host.threadStatusLabel(id),'Idle');assert.equal(h.transport.log.count('thread/resume'),0);
});

test('a nullable metadata proof preserves identity for a later exact recovery notice',async t=>{
 const h=await connected();t.after(()=>h.client.close());const listing=h.host.listSessions();const request=(await h.transport.log.awaitMethod('session/list'))[0]!;
 h.transport.respond(request.id,{type:'sessions',sessions:[{...row(),thread_status:null}],next_offset:null});const page=await listing;
 assert.equal(page.sessions[0]?.thread_id,id);assert.equal(h.host.threadStatusLabel(id),'Status unavailable');
 const read=(await h.transport.log.awaitMethod('thread/read'))[0]!;h.transport.respondError(read.id,{code:-32000,message:'source unavailable'});
 changed(h,entry('21',{type:'idle'}));assert.equal(h.host.threadStatusLabel(id),'Idle');assert.equal(h.transport.log.count('thread/resume'),0);
});
test('a nullable proof revokes pre-read status but preserves a later recovery revision',async t=>{
 for(const recovered of [false,true]){
  const h=await connected();t.after(()=>h.client.close());
  const first=h.host.listSessions();const init=(await h.transport.log.awaitMethod('session/list'))[0]!;
  h.transport.respond(init.id,{type:'sessions',sessions:[{...row(),thread_status:entry('5',{type:'idle'})}],next_offset:null});await first;
  changed(h,entry('5',{type:'idle'}));
  const listing=h.host.listSessions();const request=(await h.transport.log.awaitMethod('session/list',2))[1]!;
  changed(h,recovered?entry('6',{type:'active',activeFlags:[]}):entry('5',{type:'idle'}));
  h.transport.respond(request.id,{type:'sessions',sessions:[{...row(),thread_status:null}],next_offset:null});await listing;
  assert.equal(h.host.threadStatusLabel(id),recovered?'Running':'Status unavailable');
  if(!recovered){const read=(await h.transport.log.awaitMethod('thread/read'))[0]!;h.transport.respondError(read.id,{code:-32000,message:'source unavailable'});}
  changed(h,entry('7',{type:'idle'}));assert.equal(h.host.threadStatusLabel(id),'Idle');
 }
});
test('revoking a snapshot retains its revision fence against delayed notifications',async t=>{
 const h=await connected();t.after(()=>h.client.close());
 const first=h.host.listSessions();const init=(await h.transport.log.awaitMethod('session/list'))[0]!;
 h.transport.respond(init.id,{type:'sessions',sessions:[{...row(),thread_status:entry('10',{type:'active',activeFlags:[]})}],next_offset:null});await first;
 const listing=h.host.listSessions();const request=(await h.transport.log.awaitMethod('session/list',2))[1]!;
 h.transport.respond(request.id,{type:'sessions',sessions:[{...row(),thread_status:null}],next_offset:null});await listing;
 const read=(await h.transport.log.awaitMethod('thread/read'))[0]!;h.transport.respondError(read.id,{code:-32000,message:'source unavailable'});
 changed(h,entry('9',{type:'idle'}));assert.equal(h.host.threadStatusLabel(id),'Status unavailable');
 changed(h,entry('11',{type:'idle'}));assert.equal(h.host.threadStatusLabel(id),'Idle');
});
test('a revoked UI proof keeps its revision fence after notification-cache eviction',async t=>{
 const h=await connected();t.after(()=>h.client.close());
 const first=h.host.listSessions();const init=(await h.transport.log.awaitMethod('session/list'))[0]!;
 h.transport.respond(init.id,{type:'sessions',sessions:[{...row(),thread_status:entry('10',{type:'active',activeFlags:[]})}],next_offset:null});await first;
 for(let i=0;i<130;i++)changed(h,{...entry(String(20+i),{type:'idle'}),threadId:`conv_113cba28-d6b2-70f4-8f62-${String(i).padStart(12,'0')}`});
 const listing=h.host.listSessions();const request=(await h.transport.log.awaitMethod('session/list',2))[1]!;
 h.transport.respond(request.id,{type:'sessions',sessions:[{...row(),thread_status:null}],next_offset:null});await listing;
 const read=(await h.transport.log.awaitMethod('thread/read'))[0]!;h.transport.respondError(read.id,{code:-32000,message:'source unavailable'});
 changed(h,entry('10',{type:'active',activeFlags:[]}));assert.equal(h.host.threadStatusLabel(id),'Status unavailable');
 changed(h,entry('500',{type:'idle'}));assert.equal(h.host.threadStatusLabel(id),'Idle');
});
