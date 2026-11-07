import {RpcFailure} from '../src/client/errors';
import {afterEach,expect,it} from 'vitest';
import {Server,snapshot} from './fixture';
import type {ThreadStatusEntry} from '../../protocol/app-server/v74';
const servers:Server[]=[];
const server=()=>{const s=new Server();servers.push(s);return s;};
afterEach(()=>{for(const s of servers.splice(0))s.client.disconnect();});
const entry=(revision:string,status:ThreadStatusEntry['status'],runtimeIncarnation:string|null='1'):ThreadStatusEntry=>({threadId:'conversation-A',revision,status,runtimeIncarnation});
function changed(s:Server,value:ThreadStatusEntry){s.socket.deliver({jsonrpc:'2.0',method:'thread/status/changed',params:value});}
function published(s:Server,ready:()=>boolean):Promise<void>{if(ready())return Promise.resolve();return new Promise(resolve=>{const stop=s.client.subscribe(()=>{if(ready()){stop();resolve();}});});}
it('an early notification wins over the first catalog snapshot without an attachment',async()=>{
 const s=server();s.held.add('session/list');const connecting=s.connect();const request=await s.waitFor('session/list',1);
 const stale={...s.summary('A'),thread_status:entry('5',{type:'idle'})};
 changed(s,entry('6',{type:'active',activeFlags:['waitingOnApproval','waitingOnUserInput']}));
 s.socket.success(request,{type:'sessions',sessions:[stale],next_offset:null});await connecting;
 expect(s.client.getSnapshot().sessions[0].thread_status).toEqual(entry('6',{type:'active',activeFlags:['waitingOnApproval','waitingOnUserInput']}));
 expect(s.requests.some(row=>row.request.method==='thread/resume')).toBe(false);
});
it('retirement and new incarnations cannot be overwritten by delayed old notifications',async()=>{
 const s=server();await s.connect();
 changed(s,entry('6',{type:'active',activeFlags:[]}));changed(s,entry('8',{type:'notLoaded'},null));changed(s,entry('7',{type:'active',activeFlags:[]}));
 expect(s.client.getSnapshot().sessions.find(row=>row.id==='A')?.thread_status?.status.type).toBe('notLoaded');
 changed(s,entry('9',{type:'active',activeFlags:[]},'2'));changed(s,entry('8',{type:'idle'}));
 expect(s.client.getSnapshot().sessions.find(row=>row.id==='A')?.thread_status?.runtimeIncarnation).toBe('2');
});
it('a second overflow during repair requires a second authoritative snapshot',async()=>{
 const s=server();await s.connect();s.held.add('session/list');
 const resync=()=>s.socket.deliver({jsonrpc:'2.0',method:'thread/status/resyncRequired',params:{}});
 resync();const first=await s.waitFor('session/list',2);resync();
 s.socket.success(first,{type:'sessions',sessions:[{...s.summary('A'),thread_status:entry('5',{type:'idle'})}],next_offset:null});
 const second=await s.waitFor('session/list',3);
 expect(s.client.getSnapshot().threadStatusSynchronized).toBe(false);
 s.socket.success(second,{type:'sessions',sessions:[{...s.summary('A'),thread_status:entry('10',{type:'active',activeFlags:[]})}],next_offset:null});
 await published(s,()=>s.client.getSnapshot().threadStatusSynchronized===true);
 expect(s.client.getSnapshot().sessions[0].thread_status?.status.type).toBe('active');
});
it('reconnect establishes fresh status availability from its own catalog snapshot',async()=>{
 const s=server();await s.connect();await s.client.disconnect();expect(s.client.getSnapshot().threadStatusSynchronized).toBe(false);
 await s.connect();expect(s.client.getSnapshot().threadStatusSynchronized).toBe(true);
 expect(s.client.getSnapshot().sessions[0].thread_status).toBeDefined();
});
it('notification buffer eviction repairs a newly introduced stale catalog row with a pure read',async()=>{
 const s=server();s.held.add('session/list');const connecting=s.connect();const request=await s.waitFor('session/list',1);
 changed(s,entry('5',{type:'active',activeFlags:[]}));
 for(let i=0;i<130;i++)changed(s,{...entry(String(6+i),{type:'idle'}),threadId:`other-${i}`});
 s.handlers.set('session/list',()=>({type:'sessions',sessions:[{...s.summary('A'),thread_status:entry('200',{type:'active',activeFlags:[]})}],next_offset:null}));
 s.held.delete('session/list');
 s.socket.success(request,{type:'sessions',sessions:[{...s.summary('A'),thread_status:entry('1',{type:'idle'})}],next_offset:null});await connecting;
 await s.waitFor('session/list',2);
 await published(s,()=>s.client.getSnapshot().sessions[0]?.thread_status?.revision==='200');
 expect(s.requests.some(row=>row.request.method==='thread/resume')).toBe(false);
});

it('a gap during the first list keeps status unavailable until the later authoritative page',async()=>{
 const s=server();s.held.add('session/list');const connecting=s.connect();const old=await s.waitFor('session/list',1);
 s.socket.deliver({jsonrpc:'2.0',method:'thread/status/resyncRequired',params:{}});const repair=await s.waitFor('session/list',2);
 s.socket.success(old,{type:'sessions',sessions:[{...s.summary('A'),thread_status:entry('5',{type:'idle'})}],next_offset:null});await connecting;
 expect(s.client.getSnapshot().threadStatusSynchronized).toBe(false);
 s.socket.success(repair,{type:'sessions',sessions:[{...s.summary('A'),thread_status:entry('10',{type:'active',activeFlags:[]})}],next_offset:null});
 await published(s,()=>s.client.getSnapshot().threadStatusSynchronized===true);
 expect(s.client.getSnapshot().sessions[0].thread_status?.status.type).toBe('active');
});
it('one unavailable Thread cannot poison repair of other rows and its recovery notice restores its row',async()=>{
 const s=server();await s.connect();
 s.handlers.set('thread/read',request=>{
  if(request.method!=='thread/read')throw Error('Thread read');
  if(request.params.threadId==='conversation-A')throw new RpcFailure({code:-32000,message:'busy source'});
  return {type:'thread',thread:{id:request.params.threadId,sessionId:'B',nodeId:'node-B',name:null,archived:false,dynamicTools:[],status:{type:'active',activeFlags:[]},statusRevision:'20',runtimeIncarnation:'1'}};
 });
 s.socket.deliver({jsonrpc:'2.0',method:'thread/status/resyncRequired',params:{}});
 await published(s,()=>s.client.getSnapshot().threadStatusSynchronized===true);
 expect(s.client.getSnapshot().sessions.find(row=>row.id==='A')?.thread_status).toBeUndefined();
 expect(s.client.getSnapshot().sessions.find(row=>row.id==='B')?.thread_status?.status.type).toBe('active');
 changed(s,entry('21',{type:'idle'}));
 expect(s.client.getSnapshot().sessions.find(row=>row.id==='A')?.thread_status?.status.type).toBe('idle');
 expect(s.requests.some(row=>row.request.method==='thread/resume')).toBe(false);
});

it('metadata without a status proof remains readable and accepts the exact recovery notification',async()=>{
 const s=server();s.handlers.set('session/list',()=>({type:'sessions',sessions:[{...s.summary('A'),thread_status:null}],next_offset:null}));
 await s.connect();expect(s.client.getSnapshot().sessions[0].thread_id).toBe('conversation-A');
 expect(s.client.getSnapshot().sessions[0].thread_status).toBeUndefined();
 changed(s,entry('21',{type:'idle'}));expect(s.client.getSnapshot().sessions[0].thread_status?.status.type).toBe('idle');
 expect(s.requests.some(row=>row.request.method==='thread/resume')).toBe(false);
});
it('an unknown exact summary crossing a repaired gap requires a fresh native status read',async()=>{
 const s=server();await s.connect();s.snapshots.set('C',snapshot('C'));s.client.restoreViews(['C']);
 s.held.add('session/summary');const reading=s.client.readSessionSummary('C');const request=await s.waitFor('session/summary',1);
 s.handlers.set('session/list',()=>({type:'sessions',sessions:[s.summary('A'),s.summary('B')],next_offset:null}));
 s.handlers.set('thread/read',request=>{
  if(request.method!=='thread/read')throw Error('Thread read');
  const id=request.params.threadId.replace('conversation-','');
  return {type:'thread',thread:{id:request.params.threadId,sessionId:id,nodeId:`node-${id}`,name:null,archived:false,dynamicTools:[],status:{type:'active',activeFlags:[]},statusRevision:'20',runtimeIncarnation:'1'}};
 });
 s.socket.deliver({jsonrpc:'2.0',method:'thread/status/resyncRequired',params:{}});
 await published(s,()=>s.client.getSnapshot().threadStatusSynchronized===true);
 s.socket.success(request,{type:'session_summary',summary:{...s.summary('C'),thread_status:{...entry('5',{type:'idle'}),threadId:'conversation-C'}}});await reading;
 await published(s,()=>s.client.getSnapshot().views.C?.summary?.thread_status?.revision==='20');
 expect(s.client.getSnapshot().views.C.summary?.thread_status?.status.type).toBe('active');
 expect(s.requests.some(row=>row.request.method==='thread/resume')).toBe(false);
});
it('a nullable proof revokes older status while preserving a recovery notice received during the read',async()=>{
 for(const recovered of [false,true]){
  const s=server();await s.connect();changed(s,entry('5',{type:'idle'}));s.held.add('session/list');
  const listing=s.client.listSessions();const request=await s.waitFor('session/list',2);
  changed(s,entry(recovered?'6':'5',{type:recovered?'active':'idle',...(recovered?{activeFlags:[]}: {})} as ThreadStatusEntry['status']));
  s.socket.success(request,{type:'sessions',sessions:[{...s.summary('A'),thread_status:null}],next_offset:null});await listing;
  expect(s.client.getSnapshot().sessions[0].thread_status?.status.type).toBe(recovered?'active':undefined);
  changed(s,entry('7',{type:'idle'}));expect(s.client.getSnapshot().sessions[0].thread_status?.revision).toBe('7');
 }
});
it('a revoked snapshot revision rejects delayed notifications until a newer recovery proof',async()=>{
 const s=server();s.handlers.set('session/list',()=>({type:'sessions',sessions:[{...s.summary('A'),thread_status:entry('10',{type:'active',activeFlags:[]})}],next_offset:null}));await s.connect();
 s.handlers.set('session/list',()=>({type:'sessions',sessions:[{...s.summary('A'),thread_status:null}],next_offset:null}));await s.client.listSessions();
 changed(s,entry('9',{type:'idle'}));expect(s.client.getSnapshot().sessions[0].thread_status).toBeUndefined();
 changed(s,entry('11',{type:'idle'}));expect(s.client.getSnapshot().sessions[0].thread_status?.revision).toBe('11');
});
it('revocation keeps the visible revision fence after the bounded notification cache evicts it',async()=>{
 const s=server();s.handlers.set('session/list',()=>({type:'sessions',sessions:[{...s.summary('A'),thread_status:entry('10',{type:'active',activeFlags:[]})}],next_offset:null}));await s.connect();
 for(let i=0;i<130;i++)changed(s,{...entry(String(20+i),{type:'idle'}),threadId:`other-${i}`});
 s.handlers.set('session/list',()=>({type:'sessions',sessions:[{...s.summary('A'),thread_status:null}],next_offset:null}));await s.client.listSessions();
 changed(s,entry('10',{type:'active',activeFlags:[]}));expect(s.client.getSnapshot().sessions[0].thread_status).toBeUndefined();
 changed(s,entry('500',{type:'idle'}));expect(s.client.getSnapshot().sessions[0].thread_status?.revision).toBe('500');
});
