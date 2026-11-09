import assert from 'node:assert/strict';
import {test} from 'node:test';
import {summaryFor,threadFor} from '../../protocol/app-server/testing.ts';
import {AppServerClient} from '../src/app-server/client.ts';
import {AppServerHost} from '../src/app-server/host.ts';
import type {ThreadStatusEntry} from '../../protocol/app-server/v76.ts';
import {FakeTransport} from './support/app-server-peer.ts';
import {harness,SERVER_CAPABILITIES} from './support/app-server-harness.ts';
import {sessionView} from './support/fixtures.ts';
async function connected(){const transport=new FakeTransport();const work=AppServerClient.initialize({transport});const init=(await transport.log.awaitMethod('initialize'))[0]!;transport.respond(init.id,{type:'initialized',authority_id:'status-authority',protocol_version:76,capabilities:SERVER_CAPABILITIES});const client=await work;return {transport,client,host:new AppServerHost({client,ownership:'external'})};}
const id=sessionView().active_conversation_id;
const entry=(revision:string,status:ThreadStatusEntry['status'],runtimeIncarnation:string|null='1'):ThreadStatusEntry=>({threadId:id,revision,status,runtimeIncarnation});
const row=()=>summaryFor({id:sessionView().id,cwd:'/work',name:'A',ownership_generation:'1',active_node:sessionView().active_node,updated_at:'2026-09-14T00:00:00Z'},id);
const changed=(h:Awaited<ReturnType<typeof connected>>,value:ThreadStatusEntry)=>h.transport.deliver({jsonrpc:'2.0',method:'thread/status/changed',params:value});
const resync=(h:Awaited<ReturnType<typeof connected>>)=>h.transport.deliver({jsonrpc:'2.0',method:'thread/status/resyncRequired',params:{}});
const readResult=(value:ThreadStatusEntry)=>({type:'thread' as const,thread:{...threadFor(sessionView({active_conversation_id:id})),status:value.status,statusRevision:value.revision,runtimeIncarnation:value.runtimeIncarnation}});
test('a late failed Thread read preserves a recovery notification received during that read',async t=>{
 for(const evicted of [false,true]){
 const h=await connected();t.after(()=>h.client.close());
 const listing=h.host.listSessions();const request=(await h.transport.log.awaitMethod('session/list'))[0]!;
 h.transport.respond(request.id,{type:'sessions',sessions:[{...row(),thread_status:entry('5',{type:'idle'})}],next_offset:null});await listing;
 resync(h);const read=(await h.transport.log.awaitMethod('thread/read'))[0]!;
 changed(h,entry('20',{type:'active',activeFlags:[]}));
 if(evicted)for(let i=0;i<130;i++)changed(h,{...entry(String(30+i),{type:'idle'}),threadId:`conv_113cba28-d6b2-70f4-8f62-${String(i).padStart(12,'0')}`});
 const repaired=new Promise<void>(resolve=>{const stop=h.host.onThreadStatus(()=>{stop();resolve();});});
 h.transport.respondError(read.id,{code:-32000,message:'source unavailable'});await repaired;
 assert.equal(h.host.threadStatusLabel(id),'Running');assert.equal(h.host.threadStatus(id)?.revision,'20');
 changed(h,entry('19',{type:'idle'}));assert.equal(h.host.threadStatus(id)?.revision,'20');
 }
});
test('a newer snapshot inherits recovery evidence before an older nullable response arrives',async t=>{
 const h=await connected();t.after(()=>h.client.close());
 const first=h.host.listSessions();const initial=(await h.transport.log.awaitMethod('session/list'))[0]!;
 h.transport.respond(initial.id,{type:'sessions',sessions:[{...row(),thread_status:entry('5',{type:'idle'})}],next_offset:null});await first;
 const older=h.host.listSessions();const old=(await h.transport.log.awaitMethod('session/list',2))[1]!;
 changed(h,entry('20',{type:'active',activeFlags:[]}));
 const newer=h.host.listSessions();const fresh=(await h.transport.log.awaitMethod('session/list',3))[2]!;
 h.transport.respond(fresh.id,{type:'sessions',sessions:[{...row(),thread_status:entry('30',{type:'idle'})}],next_offset:null});await newer;
 assert.equal(h.host.threadStatus(id)?.revision,'30');
 h.transport.respond(old.id,{type:'sessions',sessions:[{...row(),thread_status:null}],next_offset:null});await older;
 assert.equal(h.host.threadStatus(id)?.revision,'30');assert.equal(h.host.threadStatusLabel(id),'Idle');
 assert.equal(h.transport.log.count('thread/read'),0);
});
test('all 32 cold catalog rows crossing a status gap retain sequential repair ownership',async t=>{
 const h=await connected();t.after(()=>h.client.close());
 const listing=h.host.listSessions();const request=(await h.transport.log.awaitMethod('session/list'))[0]!;
 resync(h);
 const rows=Array.from({length:32},(_,i)=>{
  const suffix=String(i).padStart(12,'0'),threadId=`conv_113cba28-d6b2-70f4-8f62-${suffix}`;
  return {...row(),id:`ses_113cba28-d6b2-70f4-8f62-${suffix}`,thread_id:threadId,thread_status:{...entry('0',{type:'notLoaded'},null),threadId}};
 });
 h.transport.respond(request.id,{type:'sessions',sessions:rows,next_offset:null});await listing;
 for(let i=0;i<rows.length;i++){
  const read=(await h.transport.log.awaitMethod('thread/read',i+1))[i]!;
  assert.equal(read.method,'thread/read');if(read.method!=='thread/read')throw Error('Thread read');
  assert.equal(read.params.threadId,rows[i]!.thread_id);
  const result=readResult({...entry('0',{type:'notLoaded'},null),threadId:read.params.threadId});result.thread.id=read.params.threadId;
  h.transport.respond(read.id,result);
 }
 const last=rows.at(-1)!.thread_id;
 if(!h.host.threadStatus(last))await new Promise<void>(resolve=>{const stop=h.host.onThreadStatus(()=>{if(h.host.threadStatus(last)){stop();resolve();}});});
 for(const value of rows)assert.equal(h.host.threadStatusLabel(value.thread_id),'Not loaded');
 assert.equal(h.transport.log.count('thread/read'),32);assert.equal(h.transport.log.count('thread/resume'),0);
});
test('status repair waits for unrelated metadata capacity and close wakes a parked repair',async t=>{
 for(const closeWhileWaiting of [false,true]){
  const h=await connected();t.after(()=>h.client.close());
  const busy=[h.client.call('thread/read',{threadId:id},'thread'),h.client.call('thread/read',{threadId:id},'thread')];
  const outcomes=Promise.allSettled(busy);
  const requests=await h.transport.log.awaitMethod('thread/read',2);
  const listing=h.host.listSessions();const request=(await h.transport.log.awaitMethod('session/list'))[0]!;
  resync(h);h.transport.respond(request.id,{type:'sessions',sessions:[{...row(),thread_status:entry('0',{type:'notLoaded'},null)}],next_offset:null});await listing;
  assert.equal(h.transport.log.count('thread/read'),2);
  if(closeWhileWaiting){h.client.close();await outcomes;assert.equal(h.host.threadStatusLabel(id),'Status unavailable');continue;}
  h.transport.respond(requests[0]!.id,readResult(entry('0',{type:'notLoaded'},null)));
  const repairedRead=(await h.transport.log.awaitMethod('thread/read',3))[2]!;
  const repaired=new Promise<void>(resolve=>{const stop=h.host.onThreadStatus(()=>{if(h.host.threadStatus(id)){stop();resolve();}});});
  h.transport.respond(repairedRead.id,readResult(entry('0',{type:'notLoaded'},null)));await repaired;
  h.transport.respond(requests[1]!.id,readResult(entry('0',{type:'notLoaded'},null)));await outcomes;
  assert.equal(h.host.threadStatusLabel(id),'Not loaded');assert.equal(h.transport.log.count('thread/read'),3);
 }
});
test('evicting the final queued interest while capacity is full never sends an undefined Thread',async t=>{
 const h=await connected();t.after(()=>h.client.close());
 const busy=[h.client.call('thread/read',{threadId:id},'thread'),h.client.call('thread/read',{threadId:id},'thread')];
 const outcomes=Promise.allSettled(busy);const reads=await h.transport.log.awaitMethod('thread/read',2);
 const first=h.host.listSessions();const original=(await h.transport.log.awaitMethod('session/list'))[0]!;
 h.transport.respond(original.id,{type:'sessions',sessions:[{...row(),thread_status:null}],next_offset:null});await first;
 for(let page=0;page<4;page++){
  const listing=h.host.listSessions(undefined,page*32);const request=(await h.transport.log.awaitMethod('session/list',page+2))[page+1]!;
  const rows=Array.from({length:32},(_,i)=>{
   const suffix=String(page*32+i).padStart(12,'0'),threadId=`conv_113cba28-d6b2-70f4-8f62-${suffix}`;
   return {...row(),id:`ses_113cba28-d6b2-70f4-8f62-${suffix}`,thread_id:threadId,thread_status:{...entry('0',{type:'notLoaded'},null),threadId}};
  });
  h.transport.respond(request.id,{type:'sessions',sessions:rows,next_offset:null});await listing;
 }
 h.transport.respond(reads[0]!.id,readResult(entry('0',{type:'notLoaded'},null)));await busy[0];
 await h.client.waitForMetadataCapacity();
 assert.equal(h.transport.log.count('thread/read'),2);
 h.transport.respond(reads[1]!.id,readResult(entry('0',{type:'notLoaded'},null)));await outcomes;
 assert.equal(h.transport.log.count('thread/read'),2);
});
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
 for(const recovered of [false,true])for(const evicted of [false,true]){
  const h=await connected();t.after(()=>h.client.close());
  const first=h.host.listSessions();const init=(await h.transport.log.awaitMethod('session/list'))[0]!;
  h.transport.respond(init.id,{type:'sessions',sessions:[{...row(),thread_status:entry('5',{type:'idle'})}],next_offset:null});await first;
  changed(h,entry('5',{type:'idle'}));
  const listing=h.host.listSessions();const request=(await h.transport.log.awaitMethod('session/list',2))[1]!;
  changed(h,recovered?entry('6',{type:'active',activeFlags:[]}):entry('5',{type:'idle'}));
  if(evicted)for(let i=0;i<130;i++)changed(h,{...entry(String(20+i),{type:'idle'}),threadId:`conv_113cba28-d6b2-70f4-8f62-${String(i).padStart(12,'0')}`});
  h.transport.respond(request.id,{type:'sessions',sessions:[{...row(),thread_status:null}],next_offset:null});await listing;
  assert.equal(h.host.threadStatusLabel(id),recovered?'Running':'Status unavailable');
  if(!recovered){const read=(await h.transport.log.awaitMethod('thread/read'))[0]!;h.transport.respondError(read.id,{code:-32000,message:'source unavailable'});}
  changed(h,entry('500',{type:'idle'}));assert.equal(h.host.threadStatusLabel(id),'Idle');
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

async function awaitStatus(h: Awaited<ReturnType<typeof harness>>, threadId: string): Promise<void> {
 if(h.host.threadStatus(threadId))return;
 await new Promise<void>(resolve=>{const stop=h.host.onThreadStatus(()=>{if(h.host.threadStatus(threadId)){stop();resolve();}});});
}
async function browseColdPages(h: Awaited<ReturnType<typeof connected>>, pages: number): Promise<void> {
 for(let page=0;page<pages;page++){
  const seen=h.transport.log.count('session/list');
  const listing=h.host.listSessions(undefined,page*32);
  const request=(await h.transport.log.awaitMethod('session/list',seen+1))[seen]!;
  const rows=Array.from({length:32},(_,i)=>{
   const suffix=String(page*32+i).padStart(12,'0'),threadId=`conv_113cba28-d6b2-70f4-8f62-${suffix}`;
   return {...row(),id:`ses_113cba28-d6b2-70f4-8f62-${suffix}`,thread_id:threadId,thread_status:{...entry(String(100+page*32+i),{type:'notLoaded'},null),threadId}};
  });
  h.transport.respond(request.id,{type:'sessions',sessions:rows,next_offset:null});await listing;
 }
}
test('catalog eviction preserves an attached Thread proof until detach or native retirement',{timeout:10000},async t=>{
 for(const ending of ['detach','retire'] as const){
  const h=await harness();t.after(()=>h.client.close());const threadId=h.target.conversation_id;
  await awaitStatus(h,threadId);assert.equal(h.host.threadStatusLabel(threadId),'Idle');
  await browseColdPages(h,5);
  assert.equal(h.host.attachment(h.target.session_id),h.session);assert.equal(h.session.released,false);
  assert.equal(h.host.threadStatusLabel(threadId),'Idle');assert.equal(h.transport.log.count('thread/read'),1);
  // Once the attachment ends, only the ordinary 128-entry catalog bound remains.
  if(ending==='detach'){
   const detaching=h.host.detach(h.target.session_id);
   const request=(await h.transport.log.awaitMethod('session/detach'))[0]!;
   h.transport.respond(request.id,{type:'detached'});await detaching;
  }else h.transport.deliver({jsonrpc:'2.0',method:'session/closed',params:{target:h.target}});
  assert.equal(h.host.attachment(h.target.session_id),undefined);
  assert.equal(h.host.threadStatus(threadId),undefined);
  assert.equal(h.host.threadStatusLabel('conv_113cba28-d6b2-70f4-8f62-000000000032'),'Not loaded');
  assert.equal(h.host.threadStatus('conv_113cba28-d6b2-70f4-8f62-000000000031'),undefined);
 }
});
test('catalog browsing preserves an attached Thread pending repair while metadata capacity is full',{timeout:10000},async t=>{
 const h=await harness();t.after(()=>h.client.close());const threadId=h.target.conversation_id;
 await awaitStatus(h,threadId);
 const busy=[h.client.call('thread/read',{threadId},'thread'),h.client.call('thread/read',{threadId},'thread')];
 const outcomes=Promise.allSettled(busy);const reads=await h.transport.log.awaitMethod('thread/read',3);
 const listing=h.host.listSessions();const request=(await h.transport.log.awaitMethod('session/list'))[0]!;
 h.transport.respond(request.id,{type:'sessions',sessions:[{...row(),id:h.target.session_id,thread_id:threadId,thread_status:null}],next_offset:null});await listing;
 await browseColdPages(h,5);assert.equal(h.transport.log.count('thread/read'),3);
 h.transport.respond(reads[1]!.id,readResult(entry('1',{type:'idle'})));await busy[0];
 const repair=(await h.transport.log.awaitMethod('thread/read',4))[3]!;
 assert.equal(repair.method,'thread/read');if(repair.method!=='thread/read')throw Error('Thread read');
 assert.equal(repair.params.threadId,threadId);
 const result=readResult({...entry('1000',{type:'idle'}),threadId});result.thread.id=threadId;
 h.transport.respond(repair.id,result);await awaitStatus(h,threadId);
 h.transport.respond(reads[2]!.id,readResult(entry('1',{type:'idle'})));await outcomes;
 assert.equal(h.host.attachment(h.target.session_id),h.session);assert.equal(h.session.released,false);
 assert.equal(h.host.threadStatusLabel(threadId),'Idle');assert.equal(h.host.threadStatus(threadId)?.revision,'1000');
 assert.equal(h.transport.log.count('thread/read'),4);
});
