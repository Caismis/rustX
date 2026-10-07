import { afterEach, expect, it } from 'vitest';
import type { ConversationTurn, ConversationTurnPage, RuntimeClientTranscriptEntry } from '../../protocol/app-server/v35';
import { Server, snapshot } from './fixture';
import { RpcFailure } from '../src/client/app-server';
import { turnKey } from '../src/client/transcript';
// Durable history 1..663: every cursor n opens native turn n.
const owner=(n:number)=>({conversation_id:'conversation-A',attempt_id:`attempt-${n}`,control_cursor:String(n),message_count:1,tool_call_count:0,outcome:'completed' as const});
const entry=(n:number):RuntimeClientTranscriptEntry=>({cursor:String(n),item:{type:'message',message:{role:'assistant',id:`m${n}`,content:[{type:'text',text:`answer-${n}`}]}},turn_process:owner(n)});
const turn=(n:number):ConversationTurn=>({id:{conversation_id:'conversation-A',attempt_id:`attempt-${n}`},ordinal:n,cursor:String(n),prompt:`prompt-${n}`,response:''});
const cut={conversation_id:'conversation-A',journal:'663',transcript:'663',mutation_revision: '0'};
const outline:ConversationTurnPage={cut,offset:0,total:663,turns:Array.from({length:64},(_,i)=>turn(i+1))};
const cursors=()=>server.client.getSnapshot().views.A.history?.page.entries?.map(row=>Number(row.cursor));
const reads=()=>server.requests.filter(row=>row.request.method==='session/transcript');
let server:Server;
afterEach(()=>server?.client.disconnect());
async function ready(){
 server=new Server();server.snapshots.set('A',{...snapshot(),transcript:{entries:Array.from({length:64},(_,i)=>entry(600+i)),next_cursor:'600'}});await server.attached('A');
 server.handlers.set('session/turns',()=>({type:'conversation_turns',page:outline}));await server.client.readTurns('A');
 // Native older paging: at most 64 entries strictly before the cursor.
 server.handlers.set('session/transcript',request=>{
  if(request.method!=='session/transcript'||request.params.at.type!=='older')throw new Error('older read');
  const before=Number(request.params.at.before),first=Math.max(1,before-request.params.limit);
  return {type:'transcript_window',window:{page:{entries:Array.from({length:before-first},(_,i)=>entry(first+i)),next_cursor:first>1?String(first):null}}};
 });
 server.held.add('session/transcript');
}
/** Reply to every held older read until the client stops asking. */
async function serve(from=1){
 for(let count=from;;count++){
  const pending=reads()[count-1];
  if(!pending){await new Promise(resolve=>setTimeout(resolve));if(!reads()[count-1])return count-1;}
  server.reply(await server.waitFor('session/transcript',count));
 }
}

it('a loaded native anchor lands at once without any read', async () => {
 await ready();
 expect(await server.client.navigateTurn('A',turn(610))).toBe(true);
 expect(reads()).toHaveLength(0);
 expect(server.client.getSnapshot().views.A.turnNavigation).toEqual({intent:expect.any(Number)});
});

it('source-message navigation reads its own lineage and stops at the exact message', async () => {
 await ready();
 expect(await server.client.navigateMessage('A','m610')).toBe(true);
 expect(reads()).toHaveLength(0);
 const work=server.client.navigateMessage('A','m530');
 await serve(); expect(await work).toBe(true);
 expect(reads()).toHaveLength(2);
 expect(cursors()?.[0]).toBe(472);
 expect(cursors()?.at(-1)).toBe(663);
});

it('an obsolete source-message jump stops paging after the in-flight read', async () => {
 await ready(); let current=true;
 const work=server.client.navigateMessage('A','m1',()=>current);
 const pending=await server.waitFor('session/transcript',1); current=false; server.reply(pending);
 expect(await work).toBe(false); expect(reads()).toHaveLength(1);
});

it('a distant turn pages older history through its location, publishing one contiguous window joined to the live tail', async () => {
 await ready();
 const work=server.client.navigateTurn('A',turn(1));
 expect(server.client.getSnapshot().views.A.turnNavigation?.pending).toBe(turnKey(turn(1).id));
 expect(server.client.getSnapshot().views.A.history?.loading).toBe(true);
 const first=await server.waitFor('session/transcript',1);
 expect(first.params).toMatchObject({limit:64});expect(first.params).toHaveProperty('at',{type:'older',before:'600'});
 // Pages publish together: the reader's window does not shift once per page.
 server.reply(first);await server.waitFor('session/transcript',2);
 expect(cursors()?.[0]).toBe(600);
 expect(await serve(2)).toBe(10);expect(await work).toBe(true);
 const view=server.client.getSnapshot().views.A;
 expect(cursors()).toEqual(Array.from({length:663},(_,i)=>i+1));
 expect(view.history?.page.next_cursor).toBeNull();expect(view.history?.loading).toBe(false);
 expect(view.turnNavigation?.pending).toBeUndefined();expect(view.turnNavigation?.error).toBeUndefined();
 // Live output still appends: nothing froze the window.
 await server.update('A',{...snapshot(),transcript:{entries:[entry(663),entry(664)]}});
 expect(cursors()?.slice(-2)).toEqual([663,664]);expect(cursors()?.[0]).toBe(1);
});

it('a newer jump retargets the shared read; only the newest navigation lands', async () => {
 await ready();
 const a=server.client.navigateTurn('A',turn(500)),b=server.client.navigateTurn('A',turn(200));
 await serve();
 expect(await a).toBe(false);expect(await b).toBe(true);
 expect(cursors()?.[0]).toBeLessThanOrEqual(200);
 expect(new Set(reads().map(row=>row.request.method==='session/transcript'&&row.request.params.at.type==='older'?row.request.params.at.before:'')).size).toBe(reads().length);
});

it('a jump during an ordinary older read joins and extends it', async () => {
 await ready();
 const older=server.client.loadEarlier('A');await server.waitFor('session/transcript',1);
 // A repeated plain gesture is a no-op while the read runs.
 await server.client.loadEarlier('A');
 const jump=server.client.navigateTurn('A',turn(400));
 await serve();await older;expect(await jump).toBe(true);
 expect(cursors()?.[0]).toBeLessThanOrEqual(400);expect(cursors()?.at(-1)).toBe(663);
});

it('an ordinary older page prepends and keeps native order', async () => {
 await ready();
 const older=server.client.loadEarlier('A');await serve();await older;
 expect(cursors()).toEqual(Array.from({length:128},(_,i)=>536+i));
 expect(server.client.getSnapshot().views.A.history?.page.next_cursor).toBe('536');
});

for(const replacement of ['attachment','node','runtime','generation','resync'] as const)it(`${replacement} replacement fences a paging navigation without changing the replacement view`,async()=>{
 await ready();const work=server.client.navigateTurn('A',turn(1));const request=await server.waitFor('session/transcript',1),socket=server.socket;
 if(replacement==='attachment'){await server.client.release('A');await server.client.attach('A');}
 if(replacement==='node'){await server.client.switchNode('A','another-node');await server.client.attach('A','another-node');}
 if(replacement==='runtime'||replacement==='generation'){await server.connect();}
 if(replacement==='resync'){socket.deliver({jsonrpc:'2.0',method:'session/resyncRequired',params:{target:server.target('A'),after_cursor:'0',earliest_serviceable:'1'}});await server.client.refresh('A');}
 const before=server.client.getSnapshot().views.A;
 socket.deliver(server.commit(request,socket));expect(await work).toBe(false);
 const after=server.client.getSnapshot().views.A;
 expect(after.history).toBe(before.history);expect(after.turnNavigation).toBe(before.turnNavigation);
 expect(reads()).toHaveLength(1);
});

it('a failing page keeps the pages already read, surfaces the error and leaves Load earlier usable', async () => {
 await ready();
 const work=server.client.navigateTurn('A',turn(1));
 server.reply(await server.waitFor('session/transcript',1));
 const failing=await server.waitFor('session/transcript',2);
 server.socket.deliver({jsonrpc:'2.0',id:failing.id,error:{code:-32000,message:'Reading fixture rejection'}});
 expect(await work).toBe(false);
 const view=server.client.getSnapshot().views.A;
 expect(cursors()?.[0]).toBe(536);expect(view.history?.loading).toBe(false);
 expect(view.history?.page.next_cursor).toBe('536');
 expect(view.history?.error).toContain('Reading fixture rejection');expect(view.turnNavigation?.error).toContain('Reading fixture rejection');
 server.client.invalidateReading('A');expect(server.client.getSnapshot().views.A.turnNavigation?.error).toBeUndefined();
 const retry=server.client.navigateTurn('A',turn(500));await serve(3);expect(await retry).toBe(true);
});

it('a page that does not move the history head cannot spin the jump', async () => {
 await ready();
 server.handlers.set('session/transcript',()=>({type:'transcript_window',window:{page:{entries:[],next_cursor:'600'}}}));
 const work=server.client.navigateTurn('A',turn(1));await serve();
 expect(await work).toBe(false);expect(reads()).toHaveLength(1);
 expect(server.client.getSnapshot().views.A.turnNavigation?.error).toContain('Invalid native history page');
});

it('a turn beyond the oldest native history fails visibly instead of landing elsewhere', async () => {
 await ready();
 const absent={...turn(1),cursor:'0'};
 const work=server.client.navigateTurn('A',absent);await serve();
 expect(await work).toBe(false);expect(cursors()?.[0]).toBe(1);
 expect(server.client.getSnapshot().views.A.turnNavigation?.error).toContain('outside the readable native history');
});

it('outline read survives a transcript page but a resync retires it synchronously', async () => {
 await ready();server.held.add('session/turns');const work=server.client.readTurns('A',64),req=await server.waitFor('session/turns',2);
 const older=server.client.loadEarlier('A');await serve();await older;
 server.socket.success(req,{type:'conversation_turns',page:{...outline,offset:64}});await work;
 expect(server.client.getSnapshot().views.A.turnOutline?.page?.offset).toBe(64);
 const pending=server.client.readTurns('A',128),old=await server.waitFor('session/turns',3);
 server.held.add('session/snapshot');server.socket.deliver({jsonrpc:'2.0',method:'session/resyncRequired',params:{target:server.target('A'),after_cursor:'0',earliest_serviceable:'1'}});
 expect(server.client.getSnapshot().views.A.turnOutline).toBeUndefined();
 server.socket.success(old,{type:'conversation_turns',page:{...outline,offset:128}});await pending;
 expect(server.client.getSnapshot().views.A.turnOutline).toBeUndefined();
});
it('resync retires a queued outline refresh; the old reply cannot retire new authority demand',async()=>{
 await ready();server.held.add('session/turns');
 const oldWork=server.client.readTurns('A'),old=await server.waitFor('session/turns',2);
 await server.client.refreshTurns('A');
 server.socket.deliver({jsonrpc:'2.0',method:'session/resyncRequired',params:{target:server.target('A'),after_cursor:'0',earliest_serviceable:'1'}});
 await server.client.refresh('A');
 const newWork=server.client.readTurns('A'),fresh=await server.waitFor('session/turns',3);
 await server.client.refreshTurns('A');
 server.socket.success(old,{type:'conversation_turns',page:{...outline,cut:{...cut,journal:'599'}}});await oldWork;
 expect(server.client.getSnapshot().views.A.turnOutline?.loading).toBe(true);
 server.socket.success(fresh,{type:'conversation_turns',page:outline});await newWork;
 const demanded=await server.waitFor('session/turns',4);
 await new Promise<void>(resolve=>{
  const unsubscribe=server.client.subscribe(()=>{if(server.client.getSnapshot().views.A.turnOutline?.page?.cut.journal==='664'){unsubscribe();resolve();}});
  server.socket.success(demanded,{type:'conversation_turns',page:{...outline,cut:{...cut,journal:'664'}}});
 });
 expect(server.client.getSnapshot().views.A.turnOutline?.page?.cut.journal).toBe('664');
 expect(server.requests.filter(row=>row.request.method==='session/turns')).toHaveLength(4);
});

it('an existing outline survives same-Attempt durable streaming progress and a jump keeps the streaming tail', async()=>{
 await ready();
 const running={attempt_id:'live-attempt',phase:{type:'running' as const},turn:1};
 const live=(n:number)=>({...entry(n),turn_process:{...owner(n),attempt_id:'live-attempt',control_cursor:'664',outcome:'running' as const}});
 server.snapshots.set('A',{...snapshot(),attempt:running,transcript:{entries:[entry(663),live(664)],next_cursor:'663'}});await server.client.refresh('A');
 const oldOutline=server.client.getSnapshot().views.A.turnOutline?.page;
 server.durableUpdate('A',{...snapshot(),attempt:running,transcript:{entries:[entry(663),live(664),live(665)]}});
 expect(server.client.getSnapshot().views.A.turnOutline?.page).toBe(oldOutline);
 const work=server.client.navigateTurn('A',turn(600));await serve();expect(await work).toBe(true);
 expect(cursors()?.slice(-3)).toEqual([663,664,665]);expect(cursors()?.[0]).toBeLessThanOrEqual(600);
 server.durableUpdate('A',{...snapshot(),attempt:running,transcript:{entries:[entry(663),live(664),live(665),live(666)]}});
 expect(cursors()?.at(-1)).toBe(666);
 expect(server.requests.filter(row=>row.request.method==='session/turns')).toHaveLength(1);
});

it('loaded semantic ownership without its exact control cursor still pages to that native location', async () => {
 await ready();
 const a=turn(90),clipped=owner(90);
 server.snapshots.set('A',{...snapshot(),transcript:{entries:[100,101].map(n=>({...entry(n),turn_process:clipped})),next_cursor:'100'}});await server.client.refresh('A');
 const locate=server.client.navigateTurn('A',a),request=await server.waitFor('session/transcript',1);
 expect(request.params).toMatchObject({limit:64});expect(request.params).toHaveProperty('at',{type:'older',before:'100'});
 server.reply(request);expect(await locate).toBe(true);
 expect(cursors()).toEqual([...Array.from({length:64},(_,i)=>36+i),100,101]);
 expect(reads()).toHaveLength(1);
});

it('a native rejection of the jump read is reported through the read, not swallowed', async () => {
 await ready();
 server.handlers.set('session/transcript',()=>{throw new RpcFailure({code:-32000,message:'Reading fixture rejection'});});
 const work=server.client.navigateTurn('A',turn(1));await serve();
 expect(await work).toBe(false);
 expect(server.client.getSnapshot().views.A.history?.error).toContain('Reading fixture rejection');
});
