import { afterEach, expect, it } from 'vitest';
import type { ConversationTurn, ConversationTurnPage, RuntimeClientTranscriptEntry } from '../../protocol/app-server/v42';
import { Server, snapshot } from './fixture';
import { HISTORY_LIMIT, HISTORY_MAX_BYTES, turnKey } from '../src/client/transcript';
const owner=(n:number)=>({conversation_id:'conversation-A',attempt_id:`attempt-${n}`,control_cursor:String(n),message_count:1,tool_call_count:0,outcome:'completed' as const});
const entry=(n:number):RuntimeClientTranscriptEntry=>({cursor:String(n),item:{type:'message',message:{role:'assistant',id:`m${n}`,content:[{type:'text',text:`answer-${n}`}]}},turn_process:owner(n)});
const turn=(n:number):ConversationTurn=>({id:{conversation_id:'conversation-A',attempt_id:`attempt-${n}`},ordinal:n,cursor:String(n),prompt:`prompt-${n}`,response:''});
const cut={conversation_id:'conversation-A',journal:'6063',transcript:'6063',mutation_revision: '0'};
const outline:ConversationTurnPage={cut,offset:0,total:6063,turns:Array.from({length:64},(_,i)=>turn(i+1))};
const cursors=()=>server.client.getSnapshot().views.A.history?.page.entries?.map(row=>Number(row.cursor));
const reads=()=>server.requests.filter(row=>row.request.method==='session/transcript');
let server:Server;
afterEach(()=>server?.client.disconnect());
async function ready(){
 server=new Server();server.snapshots.set('A',{...snapshot(),transcript:{entries:Array.from({length:64},(_,i)=>entry(6000+i)),next_cursor:'6000'}});await server.attached('A');
 server.handlers.set('session/turns',()=>({type:'conversation_turns',page:outline}));await server.client.readTurns('A');
 server.handlers.set('session/transcript',request=>{
  if(request.method!=='session/transcript')throw new Error('transcript read');
  const {at,limit}=request.params;
  const start=at.type==='turn'?Number(at.id.attempt_id.slice(8)):at.type==='older'?Math.max(1,Number(at.before)-limit):at.type==='newer'?Number(at.after)+1:6000;
  const end=at.type==='older'?Number(at.before):Math.min(start+limit,6064);
  return {type:'transcript_window',window:{cut,target:at.type==='turn'?at.id:null,target_cursor:at.type==='turn'?String(start):null,newer_cursor:end<6064?String(end-1):null,page:{entries:Array.from({length:end-start},(_,i)=>entry(start+i)),next_cursor:start>1?String(start):null}}};
 });
 server.held.add('session/transcript');
}
async function serve(count=1){server.reply(await server.waitFor('session/transcript',count));}

it('a loaded native anchor lands without a read',async()=>{await ready();expect(await server.client.navigateTurn('A',turn(6010))).toEqual(turn(6010));expect(reads()).toHaveLength(0);});
it('6063 Turns: distant jump is one read and 64 retained entries; live authority continues independently',async()=>{
 await ready();const work=server.client.navigateTurn('A',turn(1));
 expect(server.client.getSnapshot().views.A.turnNavigation?.pending).toBe(turnKey(turn(1).id));
 const request=await server.waitFor('session/transcript',1);
 expect(request.params).toMatchObject({at:{type:'turn',id:turn(1).id,cut},limit:64});
 server.reply(request);expect(await work).toEqual(turn(1));
 expect(cursors()).toEqual(Array.from({length:64},(_,i)=>i+1));expect(reads()).toHaveLength(1);
 const historical=server.client.getSnapshot().views.A.history;
 await server.update('A',{...snapshot(),transcript:{entries:[entry(6063),entry(6064)]}});
 expect(server.client.getSnapshot().views.A.history).toBe(historical);
 expect(server.client.getSnapshot().views.A.snapshot?.transcript.entries?.at(-1)?.cursor).toBe('6064');
 server.client.returnToLatest('A');expect(cursors()).toEqual([6063,6064]);expect(reads()).toHaveLength(1);
});
it('new navigation retires an older in-flight jump without waiting for it',async()=>{
 await ready();const old=server.client.navigateTurn('A',turn(500)),fresh=server.client.navigateTurn('A',turn(200));
 await serve(2);expect(await fresh).toEqual(turn(200));const installed=server.client.getSnapshot().views.A.history;
 await serve(1);expect(await old).toBe(false);expect(server.client.getSnapshot().views.A.history).toBe(installed);expect(reads()).toHaveLength(2);
});
it('a newer jump retires an ordinary older read; late completion cannot prepend into its window',async()=>{
 await ready();const older=server.client.loadEarlier('A');await server.client.loadEarlier('A');
 expect(reads()).toHaveLength(1);const jump=server.client.navigateTurn('A',turn(400));
 await serve(2);expect(await jump).toEqual(turn(400));const installed=server.client.getSnapshot().views.A.history;
 await serve(1);await older;expect(server.client.getSnapshot().views.A.history).toBe(installed);
});
it('repeated older gestures retain bounded entries and bytes, ordered without duplicates',async()=>{
 await ready();
 for(let n=1;n<=20;n++){const work=server.client.loadEarlier('A');await serve(n);await work;
 const entries=server.client.getSnapshot().views.A.history!.page.entries!;
 expect(entries.length).toBeLessThanOrEqual(HISTORY_LIMIT);expect(JSON.stringify(entries).length*2).toBeLessThanOrEqual(HISTORY_MAX_BYTES);
 expect(new Set(entries.map(row=>row.cursor)).size).toBe(entries.length);
 expect(cursors()).toEqual([...cursors()!].sort((a,b)=>a-b));}
 expect(reads()).toHaveLength(20);
});
for(const replacement of ['attachment','node','runtime','generation','resync'] as const)it(`${replacement} replacement fences navigation`,async()=>{
 await ready();const work=server.client.navigateTurn('A',turn(1));const request=await server.waitFor('session/transcript',1),socket=server.socket;
 if(replacement==='attachment'){await server.client.release('A');await server.client.attach('A');}
 if(replacement==='node'){await server.client.switchNode('A','another-node');await server.client.attach('A','another-node');}
 if(replacement==='runtime'||replacement==='generation')await server.connect();
 if(replacement==='resync'){socket.deliver({jsonrpc:'2.0',method:'session/resyncRequired',params:{target:server.target('A'),after_cursor:'0',earliest_serviceable:'1'}});await server.client.refresh('A');}
 const before=server.client.getSnapshot().views.A;socket.deliver(server.commit(request,socket));expect(await work).toBe(false);
 expect(server.client.getSnapshot().views.A.history).toBe(before.history);expect(server.client.getSnapshot().views.A.turnNavigation).toBe(before.turnNavigation);
});
it('failed jump preserves the last valid presentation and explicit retry recovers',async()=>{
 await ready();const before=server.client.getSnapshot().views.A.history,work=server.client.navigateTurn('A',turn(1));const request=await server.waitFor('session/transcript',1);
 server.socket.deliver({jsonrpc:'2.0',id:request.id,error:{code:-32000,message:'History mutation invalidated this cut'}});expect(await work).toBe(false);
 expect(server.client.getSnapshot().views.A.history).toBe(before);expect(server.client.getSnapshot().views.A.turnNavigation?.error).toContain('History mutation');
 const retry=server.client.navigateTurn('A',turn(200));await serve(2);expect(await retry).toEqual(turn(200));
});
it('native mutation retires a held read and its obsolete cut',async()=>{
 await ready();const work=server.client.navigateTurn('A',turn(1));const request=await server.waitFor('session/transcript',1);
 server.cursor++;server.socket.deliver({jsonrpc:'2.0',method:'session/event',params:{target:server.target('A'),cursor:String(server.cursor),event:{type:'pending_inbound_changed',pending:[]}}});
 const before=server.client.getSnapshot().views.A.history;server.reply(request);expect(await work).toBe(false);expect(server.client.getSnapshot().views.A.history).toBe(before);
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
it.each(['byte bound','entry bound','foreign cut'] as const)('%s rejects a malformed window without replacing presentation',async failure=>{
 await ready();const before=server.client.getSnapshot().views.A.history;
 const work=server.client.navigateTurn('A',turn(1)),request=await server.waitFor('session/transcript',1);
 const entries=failure==='entry bound'?Array.from({length:65},(_,i)=>entry(i+1)):[entry(1)];
 if(failure==='byte bound'&&entries[0].item.type==='message'&&entries[0].item.message.role==='assistant')entries[0].item.message.content=[{type:'text',text:'x'.repeat(HISTORY_MAX_BYTES)}];
 server.socket.success(request,{type:'transcript_window',window:{cut:failure==='foreign cut'?{...cut,mutation_revision:'1'}:cut,page:{entries},target:turn(1).id,target_cursor:'1'}});
 expect(await work).toBe(false);expect(reads()).toHaveLength(1);expect(server.client.getSnapshot().views.A.history).toBe(before);
 expect(server.client.getSnapshot().views.A.turnNavigation?.error).toBeTruthy();
});

it('later paging moves a finite cut and returning to latest releases it',async()=>{
 await ready();const jump=server.client.navigateTurn('A',turn(1));await serve(1);await jump;
 for(let n=2;n<=6;n++){const next=server.client.loadLater('A');await serve(n);await next;
  expect(cursors()!.length).toBeLessThanOrEqual(HISTORY_LIMIT);
  expect(reads().at(-1)!.request.params).toMatchObject({at:{type:'newer',cut},limit:64});}
 expect(reads()).toHaveLength(6);expect(cursors()![0]).toBe(129);expect(cursors()!.at(-1)).toBe(384);
 expect(server.client.getSnapshot().views.A.history?.page.next_cursor).toBe('129');
 server.client.returnToLatest('A');expect(reads()).toHaveLength(6);
 expect(server.client.getSnapshot().views.A.history?.window).toBeUndefined();
 expect(cursors()).toEqual(Array.from({length:64},(_,i)=>6000+i));
});

it('a failed replacement jump releases the obsolete page loading owner and allows recovery', async () => {
 await ready();const entries=cursors(),older=server.client.loadEarlier('A');
 const jump=server.client.navigateTurn('A',turn(400));const request=await server.waitFor('session/transcript',2);
 server.socket.deliver({jsonrpc:'2.0',id:request.id,error:{code:-32000,message:'read failed'}});expect(await jump).toBe(false);
 expect(cursors()).toEqual(entries);expect(server.client.getSnapshot().views.A.history?.loading).toBe(false);
 await serve(1);await older;expect(cursors()).toEqual(entries);
 const retry=server.client.loadEarlier('A');await serve(3);await retry;
 expect(reads()).toHaveLength(3);expect(cursors()![0]).toBe(5936);
});

it('an obsolete user gesture cannot install a native window or publish its failure', async () => {
 await ready();let current=true;
 const before=server.client.getSnapshot().views.A.history;
 const work=server.client.navigateTurn('A',turn(1),()=>current);
 const reply=await server.waitFor('session/transcript',1);
 current=false;server.reply(reply);expect(await work).toBe(false);
 expect(server.client.getSnapshot().views.A.history).toBe(before);
 const failed=server.client.navigateTurn('A',turn(2),()=>true);
 const error=await server.waitFor('session/transcript',2);
 server.client.invalidateReading('A');
 server.socket.deliver({jsonrpc:'2.0',id:error.id,error:{code:-32000,message:'obsolete failure'}});
 expect(await failed).toBe(false);expect(server.client.getSnapshot().views.A.history).toBe(before);
 expect(server.client.getSnapshot().views.A.turnNavigation?.error).toBeUndefined();
});

for(const source of ['gesture','refresh'] as const)it(`latest unloaded navigation survives an in-flight ${source} without an unbounded queue`,async()=>{
 await ready();server.held.add('session/turns');
 server.handlers.set('session/turns',request=>{if(request.method!=='session/turns')throw Error();const offset=request.params.offset??6016;return {type:'conversation_turns',page:{...outline,offset,turns:Array.from({length:Math.min(64,6063-offset)},(_,i)=>turn(offset+i+1))}};});
 const first=source==='gesture'?server.client.navigateTurn('A',100):server.client.refreshTurns('A');
 const old=await server.waitFor('session/turns',2);
 const replaced=server.client.navigateTurn('A',150),skipped=server.client.navigateTurn('A',200),latest=server.client.navigateTurn('A',300);
 expect(await replaced).toBe(false);expect(await skipped).toBe(false);
 expect(server.requests.filter(row=>row.request.method==='session/turns')).toHaveLength(2);
 const original=server.client.getSnapshot().views.A.turnOutline?.page;
 server.reply(old);await first;
 const fresh=await server.waitFor('session/turns',3);expect(fresh.params).toMatchObject({offset:256,limit:64});
 expect(server.client.getSnapshot().views.A.turnOutline?.page).toBe(original);
 server.reply(fresh);const window=await server.waitFor('session/transcript',1);expect(window.params).toMatchObject({at:{type:'turn',id:turn(300).id}});
 server.reply(window);expect(await latest).toEqual(turn(300));expect(reads()).toHaveLength(1);expect(cursors()![0]).toBe(300);
 if(source==='refresh'){const owed=await server.waitFor('session/turns',4);expect(owed.params).toMatchObject({offset:256});server.reply(owed);}
 expect(server.requests.filter(row=>row.request.method==='session/turns')).toHaveLength(source==='refresh'?4:3);
});
it('equivalent pending outline demands coalesce but only the last gesture lands',async()=>{
 await ready();server.held.add('session/turns');
 const a=server.client.navigateTurn('A',100),read=await server.waitFor('session/turns',2);
 const b=server.client.navigateTurn('A',110);
 server.socket.success(read,{type:'conversation_turns',page:{...outline,offset:64,turns:Array.from({length:64},(_,i)=>turn(i+65))}});
 expect(await a).toBe(false);await serve();expect(await b).toEqual(turn(110));
 expect(server.requests.filter(row=>row.request.method==='session/turns')).toHaveLength(2);expect(reads()).toHaveLength(1);
});
it.each(['attachment','disconnect'] as const)('%s retires both the active and deferred outline navigation',async reason=>{
 await ready();server.held.add('session/turns');
 const a=server.client.navigateTurn('A',100),old=await server.waitFor('session/turns',2),socket=server.socket;
 const b=server.client.navigateTurn('A',300);
 if(reason==='attachment'){await server.client.release('A');await server.client.attach('A');}else server.client.disconnect();
 expect(await a).toBe(false);expect(await b).toBe(false);
 const before=server.client.getSnapshot().views.A;
 socket.deliver(server.commit(old,socket));
 expect(server.client.getSnapshot().views.A.history).toBe(before.history);expect(reads()).toHaveLength(0);
 expect(server.requests.filter(row=>row.request.method==='session/turns')).toHaveLength(2);
});

it('retired user intent removes deferred outline demand without issuing its RPC',async()=>{
 await ready();server.held.add('session/turns');
 const refresh=server.client.refreshTurns('A'),old=await server.waitFor('session/turns',2);
 let current=true;const jump=server.client.navigateTurn('A',300,()=>current);
 current=false;server.client.invalidateReading('A');
 const before=server.client.getSnapshot().views.A.history;
 server.reply(old);await refresh;expect(await jump).toBe(false);
 expect(server.requests.filter(row=>row.request.method==='session/turns')).toHaveLength(2);
 expect(reads()).toHaveLength(0);expect(server.client.getSnapshot().views.A.history).toBe(before);
});
it('failed latest outline settles its pending mark without replacing history or navigating',async()=>{
 await ready();server.held.add('session/turns');
 const before=server.client.getSnapshot().views.A.history,work=server.client.navigateTurn('A',300);
 const read=await server.waitFor('session/turns',2);
 server.socket.deliver({jsonrpc:'2.0',id:read.id,error:{code:-32000,message:'outline unavailable'}});
 expect(await work).toBe(false);expect(server.client.getSnapshot().views.A.history).toBe(before);
 expect(server.client.getSnapshot().views.A.turnOutline?.error).toContain('outline unavailable');
 expect(server.client.getSnapshot().views.A.turnNavigation?.pending).toBeUndefined();expect(reads()).toHaveLength(0);
});

it.each([64,128,192])('ordinal %i stays fixed across growth into the next latest page',async ordinal=>{
 await ready();let total=ordinal;
 server.handlers.set('session/turns',request=>{
  if(request.method!=='session/turns')throw Error();
  const offset=request.params.offset??Math.floor((total-1)/64)*64;
  return {type:'conversation_turns',page:{...outline,cut:{...cut,journal:String(7000+total),transcript:String(7000+total)},total,offset,turns:Array.from({length:Math.min(64,total-offset)},(_,i)=>turn(offset+i+1))}};
 });
 await server.client.readTurns('A');server.held.add('session/turns');
 const navigation=server.client.navigateTurn('A',ordinal),held=await server.waitFor('session/turns',3);
 expect(held.params).toMatchObject({offset:ordinal-64,limit:64});
 total=ordinal+1;const resolvedCut={...cut,journal:String(7000+total),transcript:String(7000+total)};server.reply(held);
 const window=await server.waitFor('session/transcript',1);
 expect(window.params).toMatchObject({at:{type:'turn',id:turn(ordinal).id,cut:resolvedCut},limit:64});
 server.socket.success(window,{type:'transcript_window',window:{cut:resolvedCut,target:turn(ordinal).id,target_cursor:String(ordinal),page:{entries:[entry(ordinal)]}}});expect(await navigation).toEqual(turn(ordinal));expect(reads()).toHaveLength(1);
 expect(cursors()![0]).toBe(ordinal);expect(server.client.getSnapshot().views.A.turnOutline?.paging).toEqual({type:'latest'});
 const refresh=server.client.refreshTurns('A'),latest=await server.waitFor('session/turns',4);
 expect(latest.params).toMatchObject({offset:null});server.reply(latest);await refresh;
 expect(server.client.getSnapshot().views.A.turnOutline?.page?.turns.map(t=>t.ordinal)).toEqual([ordinal+1]);
});

it.each(['success','failure'] as const)('canceled pending navigation preserves active automatic %s and committed paging',async outcome=>{
 await ready();server.held.add('session/turns');
 const original=server.client.getSnapshot().views.A.turnOutline!;
 const refresh=server.client.refreshTurns('A'),held=await server.waitFor('session/turns',2);
 const navigation=server.client.navigateTurn('A',300);
 expect(server.client.getSnapshot().views.A.turnOutline?.paging).toEqual(original.paging);
 server.client.invalidateReading('A'); // The production viewport retirement boundary.
 const page={...outline,cut:{...cut,journal:'7000'}};
 if(outcome==='success')server.socket.success(held,{type:'conversation_turns',page});
 else server.socket.deliver({jsonrpc:'2.0',id:held.id,error:{code:-32000,message:'authoritative refresh failure'}});
 await refresh;expect(await navigation).toBe(false);
 const result=server.client.getSnapshot().views.A.turnOutline!;
 expect(result.paging).toEqual(original.paging);
 if(outcome==='success'){expect(result.page).toEqual(page);expect(result.error).toBeUndefined();}
 else {expect(result.page).toBe(original.page);expect(result.error).toContain('authoritative refresh failure');}
 expect(server.requests.filter(row=>row.request.method==='session/turns')).toHaveLength(2);expect(reads()).toHaveLength(0);
 const next=server.client.refreshTurns('A'),request=await server.waitFor('session/turns',3);
 expect(request.params).toMatchObject({offset:null});server.reply(request);await next;
});

it('coalesced automatic refresh survives canceled explicit reads and pending replacements',async()=>{
 await ready();server.held.add('session/turns');
 const a=server.client.navigateTurn('A',100),held=await server.waitFor('session/turns',2);
 await server.client.refreshTurns('A');await server.client.refreshTurns('A');
 const b=server.client.navigateTurn('A',200),c=server.client.navigateTurn('A',300);
 expect(await b).toBe(false);server.client.invalidateReading('A');
 server.socket.deliver({jsonrpc:'2.0',id:held.id,error:{code:-32000,message:'obsolete explicit failure'}});
 expect(await a).toBe(false);expect(await c).toBe(false);
 const refresh=await server.waitFor('session/turns',3);expect(refresh.params).toMatchObject({offset:null});
 expect(server.client.getSnapshot().views.A.turnOutline?.error).toBeUndefined();
 expect(server.client.getSnapshot().views.A.turnOutline?.paging).toEqual({type:'latest'});
 const done=new Promise<void>(resolve=>{const stop=server.client.subscribe(()=>{if(!server.client.getSnapshot().views.A.turnOutline?.loading){stop();resolve();}});});
 server.reply(refresh);await done;
 expect(server.requests.filter(row=>row.request.method==='session/turns')).toHaveLength(3);expect(reads()).toHaveLength(0);
});

it.each(['resync','mutation','attachment','disconnect'] as const)('%s retires active refresh, pending selection and queued refresh together',async reason=>{
 await ready();server.held.add('session/turns');
 const active=server.client.refreshTurns('A'),held=await server.waitFor('session/turns',2),socket=server.socket;
 const pending=server.client.navigateTurn('A',300);await server.client.refreshTurns('A');
 if(reason==='resync'){server.held.add('session/snapshot');socket.deliver({jsonrpc:'2.0',method:'session/resyncRequired',params:{target:server.target('A'),after_cursor:'0',earliest_serviceable:'1'}});}
 if(reason==='mutation'){server.cursor++;socket.deliver({jsonrpc:'2.0',method:'session/event',params:{target:server.target('A'),cursor:String(server.cursor),event:{type:'pending_inbound_changed',pending:[]}}});}
 if(reason==='attachment'){await server.client.release('A');await server.client.attach('A');}
 if(reason==='disconnect')server.client.disconnect();
 await active;expect(await pending).toBe(false);const before=server.client.getSnapshot().views.A;
 socket.deliver(server.commit(held,socket));
 expect(server.client.getSnapshot().views.A.turnOutline).toBe(before.turnOutline);expect(server.client.getSnapshot().views.A.history).toBe(before.history);
 expect(server.requests.filter(row=>row.request.method==='session/turns')).toHaveLength(2);expect(reads()).toHaveLength(0);
});

it('an automatic refresh queued during pending replacement survives and uses the committed explicit page',async()=>{
 await ready();server.held.add('session/turns');
 const a=server.client.navigateTurn('A',100),first=await server.waitFor('session/turns',2);
 const b=server.client.navigateTurn('A',200);await server.client.refreshTurns('A');
 const c=server.client.navigateTurn('A',300);expect(await b).toBe(false);
 server.reply(first);expect(await a).toBe(false);
 const selected=await server.waitFor('session/turns',3);expect(selected.params).toMatchObject({offset:256});
 server.socket.success(selected,{type:'conversation_turns',page:{...outline,offset:256,turns:Array.from({length:64},(_,i)=>turn(257+i))}});
 const owed=await server.waitFor('session/turns',4);expect(owed.params).toMatchObject({offset:256});
 await serve();expect(await c).toEqual(turn(300));expect(reads()).toHaveLength(1);
 server.reply(owed);expect(server.requests.filter(row=>row.request.method==='session/turns')).toHaveLength(4);
});

it('a genuinely missing ordinal reports unavailability without a transcript read',async()=>{
 await ready();server.held.add('session/turns');
 const work=server.client.navigateTurn('A',128),held=await server.waitFor('session/turns',2);
 server.socket.success(held,{type:'conversation_turns',page:{...outline,total:64,offset:0,turns:outline.turns}});
 expect(await work).toBe(false);expect(reads()).toHaveLength(0);
 expect(server.client.getSnapshot().views.A.turnNavigation?.error).toBe('This Turn is no longer in the native outline.');
});
