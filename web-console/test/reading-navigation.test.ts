import { afterEach, expect, it } from 'vitest';
import type { ConversationTurn, ConversationTurnPage, ConversationWindow, RuntimeClientTranscriptEntry } from '../../protocol/app-server/v32';
import { Server, snapshot } from './fixture';
import { HISTORY_LIMIT, HISTORY_MAX_BYTES, turnKey } from '../src/client/transcript';
const entry=(n:number):RuntimeClientTranscriptEntry=>({cursor:String(n),item:{type:'message',message:{role:'assistant',id:`m${n}`,content:[{type:'text',text:`answer-${n}`}]}}});
const turn=(n:number):ConversationTurn=>({id:{conversation_id:'conversation-A',attempt_id:`attempt-${n}`},ordinal:n,cursor:String(n),preview:`preview-${n}`});
const cut={conversation_id:'conversation-A',journal:'600',transcript:'600',mutation_revision: '0'};
const outline:ConversationTurnPage={cut,offset:0,total:600,turns:Array.from({length:64},(_,i)=>turn(i+1))};
const window=(target:ConversationTurn):ConversationWindow=>({cut,page:{entries:Array.from({length:64},(_,i)=>({...entry(Number(target.cursor)+i),...(i===0?{turn_process:{...target.id,control_cursor:target.cursor!,message_count:1,tool_call_count:0,outcome:'completed' as const}}:{})})),next_cursor:target.cursor},newer_cursor:String(Number(target.cursor)+63),target:target.id,target_cursor:target.cursor});
let server:Server;
afterEach(()=>server?.client.disconnect());
async function ready(){server=new Server();server.snapshots.set('A',{...snapshot(),transcript:{entries:Array.from({length:64},(_,i)=>entry(600+i)),next_cursor:'600'}});await server.attached('A');server.handlers.set('session/turns',()=>({type:'conversation_turns',page:outline}));await server.client.readTurns('A');server.held.add('session/transcript');}
for(const order of [[1,0],[0,1]])it(`only newest navigation installs and requests scroll for reply order ${order}`,async()=>{
 await ready();const commits:string[]=[];
 const a=server.client.navigateTurn('A',turn(1)).then(ok=>{if(ok)commits.push('a');});
 const b=server.client.navigateTurn('A',turn(2)).then(ok=>{if(ok)commits.push('b');});
 const requests=[await server.waitFor('session/transcript',1),await server.waitFor('session/transcript',2)];
 for(const index of order){server.socket.success(requests[index],{type:'transcript_window',window:window(turn(index+1))});await(index===0?a:b);}
 expect(commits).toEqual(['b']);const view=server.client.getSnapshot().views.A;
 expect(view.history?.page.entries?.[0].cursor).toBe('2');expect(view.turnNavigation?.active).toBe(turnKey(turn(2).id));expect(view.turnNavigation?.pending).toBeUndefined();
});
it('a >512-entry distant turn uses one direct read with bounded transcript and outline caches',async()=>{
 await ready();const work=server.client.navigateTurn('A',turn(1));const request=await server.waitFor('session/transcript',1);
 expect(request.params).toMatchObject({at:{type:'turn',id:turn(1).id,cut},limit:64});
 server.socket.success(request,{type:'transcript_window',window:window(turn(1))});expect(await work).toBe(true);
 const view=server.client.getSnapshot().views.A;
 expect(view.history?.page.entries?.[0].item).toEqual(entry(1).item);
 expect(view.history?.page.entries?.length).toBeLessThanOrEqual(HISTORY_LIMIT);expect(JSON.stringify(view.history).length*2).toBeLessThan(HISTORY_MAX_BYTES);
 expect(view.turnOutline?.page?.turns).toHaveLength(64);expect(server.requests.filter(row=>row.request.method==='session/transcript')).toHaveLength(1);
 await server.update('A',{...snapshot(),transcript:{entries:[entry(900)]}});
 expect(server.client.getSnapshot().views.A.history?.page.entries?.[0].cursor).toBe('1');
 server.client.latestTranscript('A');expect(server.client.getSnapshot().views.A.history?.page.entries?.[0].cursor).toBe('900');
 await server.update('A',{...snapshot(),transcript:{entries:[entry(900),entry(901)]}});
 expect(server.client.getSnapshot().views.A.history?.page.entries?.at(-1)?.cursor).toBe('901');
});
for(const replacement of ['session','attachment','node','runtime','generation','resync','user'] as const)it(`${replacement} replacement fences a gated navigation without changing the replacement view`,async()=>{
 await ready();const work=server.client.navigateTurn('A',turn(1));const request=await server.waitFor('session/transcript',1),socket=server.socket;
 if(replacement==='session'){server.client.navigation.invalidate();await server.client.attach('B');}
 if(replacement==='attachment'){await server.client.release('A');await server.client.attach('A');}
 if(replacement==='node'){await server.client.switchNode('A','another-node');await server.client.attach('A','another-node');}
 if(replacement==='runtime'||replacement==='generation'){await server.connect();}
 if(replacement==='resync'){socket.deliver({jsonrpc:'2.0',method:'session/resyncRequired',params:{target:server.target('A'),after_cursor:'0',earliest_serviceable:'1'}});await server.client.refresh('A');}
 if(replacement==='user')server.client.userScrolled('A');
 const before=server.client.getSnapshot().views.A;
 socket.success(request,{type:'transcript_window',window:window(turn(1))});expect(await work).toBe(false);
 const after=server.client.getSnapshot().views.A;
 expect(after.history).toBe(before.history);expect(after.turnNavigation).toBe(before.turnNavigation);
});
it('outstanding navigation reads are bounded at two and newer intent retires both',async()=>{
 await ready();const a=server.client.navigateTurn('A',turn(1)),b=server.client.navigateTurn('A',turn(2));
 const requests=[await server.waitFor('session/transcript',1),await server.waitFor('session/transcript',2)];
 expect(await server.client.navigateTurn('A',turn(3))).toBe(false);
 for(const[index,request]of requests.entries())server.socket.success(request,{type:'transcript_window',window:window(turn(index+1))});
 expect(await a).toBe(false);expect(await b).toBe(false);expect(server.requests.filter(row=>row.request.method==='session/transcript')).toHaveLength(2);
});

it('a loaded exact native anchor at the same cut commits without a second read', async () => {
 await ready(); const work = server.client.navigateTurn('A', turn(1)); const request = await server.waitFor('session/transcript', 1);
 const targetWindow = window(turn(1)); targetWindow.page.entries![0] = {cursor:'1',item:{type:'attempt_terminal',turn:{...turn(1).id,control_cursor:'1',message_count:0,tool_call_count:0,outcome:'timed_out'}}};
 server.socket.success(request,{type:'transcript_window',window:targetWindow}); expect(await work).toBe(true);
 expect(await server.client.navigateTurn('A',turn(1))).toBe(true);
 expect(server.requests.filter(row=>row.request.method==='session/transcript')).toHaveLength(1);
});
for (const mismatch of ['journal', 'mutation_revision', 'target'] as const) it(`a mismatched native ${mismatch} cannot install or leave pending navigation stuck`, async () => {
 await ready(); const before=server.client.getSnapshot().views.A.history;
 const work=server.client.navigateTurn('A',turn(1)), request=await server.waitFor('session/transcript',1);
 const invalid=window(turn(1));
 if(mismatch==='target')invalid.target=turn(2).id;
 else invalid.cut={...cut,[mismatch]:String(Number(cut[mismatch])+1)};
 server.socket.success(request,{type:'transcript_window',window:invalid});
 expect(await work).toBe(false); expect(server.client.getSnapshot().views.A.history).toBe(before);
 expect(server.client.getSnapshot().views.A.turnNavigation?.pending).toBeUndefined();
 expect(server.client.getSnapshot().views.A.turnNavigation?.error).toContain('Invalid native turn location');
});
it('outline read survives a window switch but a resync retires it synchronously', async () => {
 await ready();server.held.add('session/turns');const work=server.client.readTurns('A',64),req=await server.waitFor('session/turns',2);
 server.client.latestTranscript('A');server.socket.success(req,{type:'conversation_turns',page:{...outline,offset:64}});await work;
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
  const unsubscribe=server.client.subscribe(()=>{if(server.client.getSnapshot().views.A.turnOutline?.page?.cut.journal==='601'){unsubscribe();resolve();}});
  server.socket.success(demanded,{type:'conversation_turns',page:{...outline,cut:{...cut,journal:'601'}}});
 });
 expect(server.client.getSnapshot().views.A.turnOutline?.page?.cut.journal).toBe('601');
 expect(server.requests.filter(row=>row.request.method==='session/turns')).toHaveLength(4);
});
it('ordinary older paging is superseded by a newer navigation intent',async()=>{
 await ready();
 const older=server.client.loadEarlier('A'),a=await server.waitFor('session/transcript',1);
 const navigation=server.client.navigateTurn('A',turn(1)),b=await server.waitFor('session/transcript',2);
 server.socket.success(b,{type:'transcript_window',window:window(turn(1))});expect(await navigation).toBe(true);
 server.socket.success(a,{type:'transcript_window',window:{...window(turn(500)),target:null,target_cursor:null}});await older;
 expect(server.client.getSnapshot().views.A.history?.page.entries?.[0].cursor).toBe('1');
});

it('an ordinary prepend read survives user detachment without claiming a replacement navigation', async () => {
 await ready();
 const work=server.client.loadEarlier('A'), request=await server.waitFor('session/transcript',1);
 server.client.userScrolled('A');
 server.socket.success(request,{type:'transcript_window',window:{...window(turn(536)),target:null,target_cursor:null}});
 await work;
 expect(server.client.getSnapshot().views.A.history?.page.entries?.[0].cursor).toBe('536');
 expect(server.client.getSnapshot().views.A.turnNavigation?.active).toBeUndefined();
});

it('an existing outline survives same-Attempt durable streaming progress and installs only its original cut', async()=>{
 await ready();
 const running={attempt_id:'live-attempt',phase:{type:'running' as const},turn:1};
 server.snapshots.set('A',{...snapshot(),attempt:running,transcript:{entries:[entry(700)]}});await server.client.refresh('A');
 const oldOutline=server.client.getSnapshot().views.A.turnOutline?.page;
 server.durableUpdate('A',{...snapshot(),attempt:running,transcript:{entries:[entry(700),entry(701)]}});
 expect(server.client.getSnapshot().views.A.turnOutline?.page).toBe(oldOutline);
 const work=server.client.navigateTurn('A',turn(1)),request=await server.waitFor('session/transcript',1);
 expect(request.params).toMatchObject({at:{type:'turn',id:turn(1).id,cut}});
 server.socket.success(request,{type:'transcript_window',window:window(turn(1))});expect(await work).toBe(true);
 const history=server.client.getSnapshot().views.A.history!;
 expect(history.window?.cut).toEqual(cut);expect(history.mode).toBe('historical');
 expect(history.page.entries?.map(row=>row.cursor)).toEqual(Array.from({length:64},(_,i)=>String(i+1)));
 expect(server.requests.filter(row=>row.request.method==='session/turns')).toHaveLength(1);
 server.client.latestTranscript('A');expect(server.client.getSnapshot().views.A.history?.page.entries?.map(row=>row.cursor)).toEqual(['700','701']);
});

it('loaded semantic ownership without its exact control cursor still requires a direct native turn read', async () => {
 await ready();
 const seed=server.client.navigateTurn('A',turn(120)),first=await server.waitFor('session/transcript',1);
 server.socket.success(first,{type:'transcript_window',window:window(turn(120))});expect(await seed).toBe(true);
 const older=server.client.loadEarlier('A'),read=await server.waitFor('session/transcript',2);
 const a=turn(90),owner={...a.id,control_cursor:'90',message_count:2,tool_call_count:0,outcome:'completed' as const};
 server.socket.success(read,{type:'transcript_window',window:{cut,page:{entries:[100,101].map(n=>({...entry(n),turn_process:owner})),next_cursor:'100'},newer_cursor:'101',target:null,target_cursor:null}});
 await older;
 expect(server.client.getSnapshot().views.A.history?.page.entries?.every(row=>row.turn_process?.attempt_id===a.id.attempt_id)).toBe(true);
 const locate=server.client.navigateTurn('A',a),request=await server.waitFor('session/transcript',3);
 expect(request.params).toMatchObject({at:{type:'turn',id:a.id,cut},limit:64});
 expect(server.client.getSnapshot().views.A.history?.page.entries?.map(row=>row.cursor)).toEqual(['100','101']);
 server.socket.success(request,{type:'transcript_window',window:window(a)});expect(await locate).toBe(true);
 expect(server.client.getSnapshot().views.A.history?.page.entries?.[0].cursor).toBe('90');
 expect(server.requests.filter(row=>row.request.method==='session/transcript')).toHaveLength(3);
});
