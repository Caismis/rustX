import { act, cleanup, fireEvent, render } from '@testing-library/react';
import { afterEach, expect, it, vi } from 'vitest';
import { TurnNavigator } from '../src/app/agent/TurnNavigator';
import { Server } from './fixture';
import type { RuntimeClientSnapshot } from '../../protocol/app-server/v32';
let server: Server;
afterEach(() => { cleanup(); server?.client.disconnect(); });
it('bounded native marks retain focus preview when the pointer leaves and keyboard focus never navigates alone', async () => {
  server = new Server(); await server.attached('A');
  server.handlers.set('session/turns', () => ({type:'conversation_turns',page:{cut:{conversation_id:'conversation-A',journal:'1',transcript:'1000',mutation_revision: '0'},offset:0,total:1000,
    turns:Array.from({length:64},(_,i)=>({id:{conversation_id:'conversation-A',attempt_id:`native-${i}`},ordinal:i+1,cursor:String(i+1),preview:`Bounded native preview ${i}`}))}}));
  const navigate=vi.fn();let ui: ReturnType<typeof render>;
  await act(async()=>{ui=render(<TurnNavigator client={server.client} sessionId="A" onNavigate={navigate}/>);});
  const marks=ui!.container.querySelectorAll<HTMLButtonElement>('[data-turn-id]');expect(marks.length).toBe(64);
  act(()=>marks[0].focus());expect(ui!.getByRole('tooltip').textContent).toContain('Bounded native preview 0');
  fireEvent.pointerLeave(marks[0].parentElement!);expect(ui!.getByRole('tooltip').textContent).toContain('Bounded native preview 0');
  fireEvent.keyDown(marks[0],{key:'ArrowDown'});expect(document.activeElement).toBe(marks[1]);expect(navigate).not.toHaveBeenCalled();
  fireEvent.click(marks[1]);expect(navigate).toHaveBeenCalledWith(expect.objectContaining({id:{conversation_id:'conversation-A',attempt_id:'native-1'}}));
  expect(server.requests.filter(row=>row.request.method==='session/turns')).toHaveLength(1);
});

it('current native identity precedes location; the first durable process location enables navigation before settlement', async()=>{
 server=new Server();await server.attached('A');
 const id={conversation_id:'conversation-A',attempt_id:'live'};
 const cut={conversation_id:'conversation-A',journal:'10',transcript:'0',mutation_revision:'0'};
 let cursor:string|null=null;
 server.handlers.set('session/turns',()=>({type:'conversation_turns',page:{cut:{...cut,transcript:cursor??'0'},offset:0,total:1,turns:[{id,ordinal:1,cursor,preview:''}]}}));
 const s=server.snapshots.get('A')!;
 server.snapshots.set('A',{...s,attempt:{attempt_id:'live',phase:{type:'running'},turn:1}});await server.client.refresh('A');
 const navigate=vi.fn();let ui:ReturnType<typeof render>;
 await act(async()=>{ui=render(<TurnNavigator client={server.client} sessionId="A" onNavigate={navigate}/>);});
 const mark=()=>ui!.container.querySelector<HTMLButtonElement>('[data-turn-id]')!;
 expect(mark().getAttribute('aria-current')).toBe('true');expect(mark().disabled).toBe(true);
 cursor='1';const owner={...id,control_cursor:'1',message_count:1,tool_call_count:0,outcome:'running' as const};
 await act(async()=>{server.durableUpdate('A',{...s,attempt:{attempt_id:'live',phase:{type:'running'},turn:1},transcript:{entries:[{cursor:'1',item:{type:'message',message:{role:'assistant',id:'first',content:[{type:'text',text:'first durable'}]}},turn_process:owner}]}});});
 expect(mark().disabled).toBe(false);expect(mark().getAttribute('aria-current')).toBe('true');
 expect(server.client.getSnapshot().views.A.snapshot?.attempt?.phase.type).toBe('running');
 expect(server.requests.filter(row=>row.request.method==='session/turns')).toHaveLength(2);
 await act(async()=>{server.durableUpdate('A',{...server.snapshots.get('A')!,attempt:{attempt_id:'live',phase:{type:'running'},turn:1,in_flight:{message_id:'more',blocks:[{type:'text',block_index:0,text:'same attempt text delta'}]}}});});
 expect(server.requests.filter(row=>row.request.method==='session/turns')).toHaveLength(2);
 fireEvent.click(mark());expect(navigate).toHaveBeenCalledWith(expect.objectContaining({id,cursor:'1'}));
 ui!.rerender(<TurnNavigator client={server.client} sessionId="A" active={'turn:["conversation-A","history"]'} onNavigate={navigate}/>);
 expect(mark().hasAttribute('aria-current')).toBe(false);
 ui!.rerender(<TurnNavigator client={server.client} sessionId="A" active={'turn:["conversation-A","live"]'} onNavigate={navigate}/>);
 expect(mark().getAttribute('aria-current')).toBe('true');
});

it('a loaded historical rail mark remains usable after same-Attempt append progress without refreshing its cut', async()=>{
 server=new Server();await server.attached('A');
 const old={id:{conversation_id:'conversation-A',attempt_id:'old'},ordinal:1,cursor:'1',preview:'historical'};
 const live={id:{conversation_id:'conversation-A',attempt_id:'live'},ordinal:2,cursor:'600',preview:'current'};
 const cut={conversation_id:'conversation-A',journal:'600',transcript:'600',mutation_revision:'0'};
 const owner={...live.id,control_cursor:'600',message_count:1,tool_call_count:0,outcome:'running' as const};
 const row=(cursor:string)=>({cursor,item:{type:'message' as const,message:{role:'assistant' as const,id:`m${cursor}`,content:[{type:'text' as const,text:`output ${cursor}`}]}},turn_process:owner});
 const s={...server.snapshots.get('A')!,attempt:{attempt_id:'live',phase:{type:'running' as const},turn:1},transcript:{entries:[row('600')]}};
 server.snapshots.set('A',s);await server.client.refresh('A');
 server.handlers.set('session/turns',()=>({type:'conversation_turns',page:{cut,offset:0,total:2,turns:[old,live]}}));
 let work:Promise<boolean>|undefined,ui:ReturnType<typeof render>;
 await act(async()=>{ui=render(<TurnNavigator client={server.client} sessionId="A" onNavigate={turn=>{work=server.client.navigateTurn('A',turn);}}/>);});
 const mark=ui!.getByRole('button',{name:'Jump to turn 1'}) as HTMLButtonElement;
 await act(async()=>{server.durableUpdate('A',{...s,transcript:{entries:[row('600'),row('601')]}});});
 expect(mark.disabled).toBe(false);expect(server.client.getSnapshot().views.A.snapshot?.attempt?.phase.type).toBe('running');
 expect(server.requests.filter(row=>row.request.method==='session/turns')).toHaveLength(1);
 server.held.add('session/transcript');
 fireEvent.click(mark);const request=await server.waitFor('session/transcript',1);
 expect(request.params).toMatchObject({at:{type:'turn',id:old.id,cut}});
 await act(async()=>{server.socket.success(request,{type:'transcript_window',window:{cut,page:{entries:[{...row('1'),turn_process:{...old.id,control_cursor:'1',message_count:1,tool_call_count:0,outcome:'completed'}}]},newer_cursor:'1',target:old.id,target_cursor:'1'}});expect(await work).toBe(true);});
 const installed=server.client.getSnapshot().views.A.history!;
 expect(installed.mode).toBe('historical');expect(installed.window?.cut).toEqual(cut);
 expect(installed.page.entries?.map(row=>row.cursor)).toEqual(['1']);expect(mark.getAttribute('aria-current')).toBe('true');
 act(()=>server.client.latestTranscript('A'));
 expect(server.client.getSnapshot().views.A.history?.page.entries?.map(row=>row.cursor)).toEqual(['600','601']);
 // Follow/current publication belongs to ChatViewport, separately tested in scroll.test.tsx.
 ui!.rerender(<TurnNavigator client={server.client} sessionId="A" active={'turn:["conversation-A","live"]'} onNavigate={()=>{}}/>);
 expect(ui!.getByRole('button',{name:'Jump to turn 2'}).getAttribute('aria-current')).toBe('true');
 expect(server.requests.filter(row=>row.request.method==='session/turns')).toHaveLength(1);
});

for(const initial of [64,128])it(`latest outline follows ${initial} → ${initial+1} through start, location and settlement`,async()=>{
 const fixture=await pagingRail(initial);const n=initial+1;
 await fixture.start(n);
 let current=fixture.mark(n);expect(current).toBeTruthy();expect(current!.disabled).toBe(true);
 await fixture.locate(n);current=fixture.mark(n);expect(current!.disabled).toBe(false);expect(current!.dataset.turnOrdinal).toBe(String(n));
 await fixture.settle(n);
 expect(fixture.mark(n)!.disabled).toBe(false);expect(fixture.mark(n)!.dataset.turnOrdinal).toBe(String(n));
 expect(server.client.getSnapshot().views.A.turnOutline?.paging).toEqual({type:'latest'});
 expect(server.client.getSnapshot().views.A.turnOutline?.page?.offset).toBe(initial);
 expect(fixture.offsets()).toEqual([null,null,null,null]);
});

it('explicit historical page survives live growth; reaching newest page restores native latest intent',async()=>{
 const fixture=await pagingRail(128);
 await act(async()=>{fireEvent.click(fixture.ui.getByRole('button',{name:'Earlier turns'}));});
 expect(server.client.getSnapshot().views.A.turnOutline?.paging).toEqual({type:'page',offset:0});
 await fixture.start(129);await fixture.locate(129);await fixture.settle(129);
 const outline=server.client.getSnapshot().views.A.turnOutline!;
 expect(outline.paging).toEqual({type:'page',offset:0});expect(outline.page?.turns.map(turn=>turn.ordinal)).toEqual(Array.from({length:64},(_,i)=>i+1));
 expect(fixture.mark(129)).toBeNull();expect(fixture.offsets()).toEqual([null,0,0,0,0]);
 await act(async()=>{fireEvent.click(fixture.ui.getByRole('button',{name:'Later turns'}));});
 expect(server.client.getSnapshot().views.A.turnOutline?.paging).toEqual({type:'page',offset:64});
 await act(async()=>{fireEvent.click(fixture.ui.getByRole('button',{name:'Later turns'}));});
 expect(server.client.getSnapshot().views.A.turnOutline?.paging).toEqual({type:'latest'});expect(fixture.mark(129)!.dataset.turnOrdinal).toBe('129');
 await fixture.start(130);await fixture.locate(130);await fixture.settle(130);
 expect(fixture.mark(130)!.dataset.turnOrdinal).toBe('130');expect(fixture.offsets().slice(-4)).toEqual([null,null,null,null]);
});

it('settlement during a gated outline reply preserves one latest refresh demand',async()=>{
 const fixture=await pagingRail(64);server.held.add('session/turns');
 await fixture.start(65);const started=await server.waitFor('session/turns',2);const captured=server.commit(started);
 await fixture.locate(65);await fixture.settle(65);
 expect(fixture.offsets()).toEqual([null,null]);
 await act(async()=>{server.socket.deliver(captured);});
 const after=await server.waitFor('session/turns',3);expect(after.params).toMatchObject({offset:null});
 await act(async()=>server.reply(after));
 expect(fixture.mark(65)!.dataset.turnOrdinal).toBe('65');expect(fixture.mark(65)!.disabled).toBe(false);
 expect(server.client.getSnapshot().views.A.turnOutline?.paging).toEqual({type:'latest'});
 expect(fixture.offsets()).toEqual([null,null,null]);
});

async function pagingRail(initial:number){
 server=new Server();await server.attached('A');let total=initial,location:number|undefined;
 server.handlers.set('session/turns',request=>{
  if(request.method!=='session/turns')throw new Error('outline method');
  const offset=request.params.offset??Math.floor((total-1)/64)*64;
  return {type:'conversation_turns',page:{cut:{conversation_id:'conversation-A',journal:String(total),transcript:String(location??initial),mutation_revision:'0'},offset,total,
   turns:Array.from({length:Math.min(64,total-offset)},(_,i)=>{const n=offset+i+1;return {id:{conversation_id:'conversation-A',attempt_id:`a${n}`},ordinal:n,cursor:n>initial && n!==location?null:String(n),preview:`turn ${n}`};})}};
 });
 let ui:ReturnType<typeof render>;await act(async()=>{ui=render(<TurnNavigator client={server.client} sessionId="A" onNavigate={()=>{}}/>);});
 const update=async(n:number,settled=false,phaseChange=false)=>{const s=server.snapshots.get('A')!;const next:RuntimeClientSnapshot={...s,attempt:{attempt_id:`a${n}`,turn:1,phase:settled?{type:'settled',outcome:{type:'completed',finish_reason:{type:'stop'}}}:{type:'running'}},transcript:{entries:location?[{cursor:String(location),item:{type:'message',message:{role:'assistant',id:`m${location}`,content:[{type:'text',text:'native output'}]}},turn_process:{conversation_id:'conversation-A',attempt_id:`a${n}`,control_cursor:String(location),message_count:1,tool_call_count:0,outcome:settled?'completed' as const:'running' as const}}]:[]}};await act(async()=>{server.snapshots.set('A',next);if(phaseChange)await server.client.refresh('A');else server.durableUpdate('A',next);});};
 return {ui:ui!,mark:(n:number)=>ui!.container.querySelector<HTMLButtonElement>(`[data-turn-id='["conversation-A","a${n}"]']`),
  offsets:()=>server.requests.filter(row=>row.request.method==='session/turns').map(row=>row.request.method==='session/turns'?row.request.params.offset:undefined),
  start:async(n:number)=>{total=n;location=undefined;await update(n,false,true);},locate:async(n:number)=>{location=n;await update(n);},settle:async(n:number)=>{await update(n,true,true);}};
}

it('observed unknown detached ownership suppresses both stale navigation and unrelated live fallback', async () => {
 server=new Server();await server.attached('A');
 const id={conversation_id:'conversation-A',attempt_id:'live'},cut={conversation_id:'conversation-A',journal:'1',transcript:'1',mutation_revision:'0'};
 const s=server.snapshots.get('A')!;
 server.snapshots.set('A',{...s,attempt:{attempt_id:'live',phase:{type:'running'},turn:1}});await server.client.refresh('A');
 const turn={id,ordinal:1,cursor:'1',preview:''};
 server.handlers.set('session/turns',()=>({type:'conversation_turns',page:{cut,offset:0,total:1,turns:[turn]}}));
 const ui=await act(async()=>render(<TurnNavigator client={server.client} sessionId="A" active={null} onNavigate={()=>{}}/>));
 expect(ui.container.querySelectorAll('[aria-current]')).toHaveLength(0);
 server.held.add('session/transcript');
 const work=server.client.navigateTurn('A',turn),read=await server.waitFor('session/transcript',1);
 await act(async()=>{server.socket.success(read,{type:'transcript_window',window:{cut,page:{entries:[]},target:id,target_cursor:'1',newer_cursor:null}});expect(await work).toBe(true);});
 expect(server.client.getSnapshot().views.A.turnNavigation?.active).toBe('["conversation-A","live"]');
 expect(ui.container.querySelectorAll('[aria-current]')).toHaveLength(0);
 ui.rerender(<TurnNavigator client={server.client} sessionId="A" onNavigate={()=>{}}/>);
 expect(ui.container.querySelectorAll('[aria-current]')).toHaveLength(1);
});
