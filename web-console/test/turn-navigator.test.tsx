import { act, cleanup, fireEvent, render, within } from '@testing-library/react';
import { afterEach, expect, it, vi } from 'vitest';
import { turnRail, turnRailRange } from '../src/app/agent/turn-rail-items';
import { TurnNavigator } from '../src/app/agent/TurnNavigator';
import { Server } from './fixture';
import { installTurnNavigatorObserver } from './turn-navigator-fixture';
import type { ConversationTurn, RuntimeClientSnapshot } from '../../protocol/app-server/v37';
let server: Server;
afterEach(() => { cleanup(); server?.client.disconnect(); vi.unstubAllGlobals(); });
const turnsRequests = () => server.requests.filter(row => row.request.method === 'session/turns');
const native = (n: number, prompt = `Prompt ${n}`): ConversationTurn => ({ id: { conversation_id: 'conversation-A', attempt_id: `native-${n}` }, ordinal: n, cursor: String(n), prompt, response: `Response ${n}` });
const scroller = (ui: ReturnType<typeof render>) => ui.getByRole('navigation', { name: 'Turn navigation' }).firstElementChild as HTMLElement;
/** Scroll the rail itself, as a reader's wheel does; the virtual range follows. */
async function scrollRail(ui: ReturnType<typeof render>, top: number) {
  const element = scroller(ui);
  await act(async () => { element.scrollTop = top; element.dispatchEvent(new Event('scroll')); });
}

function navigationIntent() {
  server.handlers.set('session/transcript', request => {
    if (request.method !== 'session/transcript' || request.params.at.type !== 'turn') throw Error('expected direct Turn window');
    const at = request.params.at;
    const turn = server.client.getSnapshot().views.A.turnOutline!.page!.turns.find(turn => turn.id.attempt_id === at.id.attempt_id)!;
    return {type:'transcript_window',window:{cut:at.cut,target:at.id,target_cursor:turn.cursor,page:{entries:[{cursor:turn.cursor!,item:{type:'message',message:{role:'assistant',id:'read',content:[{type:'text',text:'window'}]}},turn_process:{...at.id,control_cursor:turn.cursor!,message_count:1,tool_call_count:0,outcome:'completed'}}]}}};
  });
  return vi.fn((selection: ConversationTurn | number) => { if (typeof selection === 'number') void server.client.navigateTurn('A', selection); });
}

async function thousandTurns(total = 1000) {
  // 600px shows about 60 marks: the loaded final page and the unloaded turns above it.
  installTurnNavigatorObserver(600);
  server = new Server(); await server.attached('A');
  server.handlers.set('session/turns', request => {
    if (request.method !== 'session/turns') throw new Error('outline method');
    const offset = request.params.offset ?? Math.floor((total - 1) / 64) * 64;
    return { type: 'conversation_turns', page: { cut: { conversation_id: 'conversation-A', journal: '1', transcript: '1000', mutation_revision: '0' }, offset, total,
      turns: Array.from({ length: Math.min(64, total - offset) }, (_, i) => native(offset + i + 1, offset + i + 1 === 999 ? '' : undefined)) } };
  });
  const navigate = navigationIntent();
  let ui: ReturnType<typeof render>;
  await act(async () => { ui = render(<TurnNavigator client={server.client} sessionId="A" active={`turn:["conversation-A","native-${total}"]`} onNavigate={navigate}/>); });
  return { ui: ui!, navigate };
}

it('every native turn has a fixed-pitch mark; only the visible range mounts and the active turn is placed in view', async () => {
  const { ui } = await thousandTurns();
  const nav = ui.getByRole('navigation', { name: 'Turn navigation' });
  expect(within(nav).getAllByRole('button').length).toBeLessThan(70);
  expect(ui.getByRole('button', { name: 'Jump to turn 1000' }).getAttribute('aria-current')).toBe('true');
  expect(ui.queryByRole('button', { name: /turn 1$/ })).toBeNull();
  // Turns outside the one native page are known by ordinal only and load first.
  expect(ui.getByRole('button', { name: 'Load and jump to turn 960' })).toBeTruthy();
  expect(turnsRequests()).toHaveLength(1);
});

it('previews show the native prompt and final response, fall back to the turn number, and focus never navigates alone', async () => {
  const { ui, navigate } = await thousandTurns();
  const mark = ui.getByRole('button', { name: 'Jump to turn 1000' });
  act(() => mark.focus());
  const tooltip = ui.getByRole('tooltip');
  expect(tooltip.textContent).toBe('Prompt 1000Response 1000');
  expect(mark.getAttribute('aria-describedby')).toBe(tooltip.id);
  fireEvent.pointerMove(ui.getByRole('button', { name: 'Jump to turn 999' }));
  expect(ui.getByRole('tooltip').textContent).toBe('Turn 999Response 999');
  fireEvent.pointerMove(ui.getByRole('button', { name: 'Load and jump to turn 960' }));
  expect(ui.getByRole('tooltip').textContent).toBe('Turn 960');
  fireEvent.pointerLeave(ui.getByRole('navigation'));
  expect(ui.queryByRole('tooltip')).toBeNull();
  expect(navigate).not.toHaveBeenCalled();
  fireEvent.click(mark);
  expect(navigate).toHaveBeenCalledExactlyOnceWith(native(1000));
});

it('an unloaded mark reads its native page before navigating; the newest page restores latest intent', async () => {
  const { ui, navigate } = await thousandTurns();
  server.held.add('session/turns');
  fireEvent.click(ui.getByRole('button', { name: 'Load and jump to turn 960' }));
  const read = await server.waitFor('session/turns', 2);
  expect(read.params).toMatchObject({ offset: 896 });
  expect(ui.getByRole('button', { name: 'Load and jump to turn 960' }).getAttribute('aria-busy')).toBe('true');
  expect(navigate).toHaveBeenCalledExactlyOnceWith(960);
  expect(server.requests.filter(row=>row.request.method==='session/transcript')).toHaveLength(0);
  await act(async () => server.reply(read));
  expect(server.client.getSnapshot().views.A.history?.window?.target).toEqual(native(960).id);
  expect(server.client.getSnapshot().views.A.turnOutline?.paging).toEqual({ type: 'page', offset: 896 });
  expect(ui.getByRole('button', { name: 'Jump to turn 960' }).hasAttribute('aria-busy')).toBe(false);
  fireEvent.click(ui.getByRole('button', { name: 'Load and jump to turn 1000' }));
  const latest = await server.waitFor('session/turns', 3);
  expect(latest.params).toMatchObject({ offset: null });
  await act(async () => server.reply(latest));
  expect(navigate).toHaveBeenLastCalledWith(1000);
  expect(server.client.getSnapshot().views.A.history?.window?.target).toEqual(native(1000).id);
  expect(server.client.getSnapshot().views.A.turnOutline?.paging).toEqual({ type: 'latest' });
});

it('fewer than two native turns render no rail', async () => {
  installTurnNavigatorObserver();
  server = new Server(); await server.attached('A');
  server.handlers.set('session/turns', () => ({ type: 'conversation_turns', page: { cut: { conversation_id: 'conversation-A', journal: '1', transcript: '1', mutation_revision: '0' }, offset: 0, total: 1, turns: [native(1)] } }));
  const ui = await act(async () => render(<TurnNavigator client={server.client} sessionId="A" onNavigate={() => {}}/>));
  expect(ui.queryByRole('navigation')).toBeNull();
});

it('current native identity precedes location; the first durable process location enables navigation before settlement', async()=>{
 installTurnNavigatorObserver();
 server=new Server();await server.attached('A');
 const id={conversation_id:'conversation-A',attempt_id:'live'};
 const cut={conversation_id:'conversation-A',journal:'10',transcript:'0',mutation_revision:'0'};
 let cursor:string|null=null;
 server.handlers.set('session/turns',()=>({type:'conversation_turns',page:{cut:{...cut,transcript:cursor??'0'},offset:0,total:2,turns:[native(1),{id,ordinal:2,cursor,prompt:'Live prompt',response:''}]}}));
 const s=server.snapshots.get('A')!;
 server.snapshots.set('A',{...s,attempt:{attempt_id:'live',phase:{type:'running'},turn:1}});await server.client.refresh('A');
 const navigate=vi.fn();let ui:ReturnType<typeof render>;
 await act(async()=>{ui=render(<TurnNavigator client={server.client} sessionId="A" onNavigate={navigate}/>);});
 const mark=()=>ui!.container.querySelector<HTMLButtonElement>('[data-turn-id=\'["conversation-A","live"]\']')!;
 expect(mark().getAttribute('aria-current')).toBe('true');expect(mark().getAttribute('aria-disabled')).toBe('true');
 fireEvent.click(mark());expect(navigate).not.toHaveBeenCalled();
 cursor='1';const owner={...id,control_cursor:'1',message_count:1,tool_call_count:0,outcome:'running' as const};
 await act(async()=>{server.durableUpdate('A',{...s,attempt:{attempt_id:'live',phase:{type:'running'},turn:1},transcript:{entries:[{cursor:'1',item:{type:'message',message:{role:'assistant',id:'first',content:[{type:'text',text:'first durable'}]}},turn_process:owner}]}});});
 expect(mark().hasAttribute('aria-disabled')).toBe(false);expect(mark().getAttribute('aria-current')).toBe('true');
 expect(server.client.getSnapshot().views.A.snapshot?.attempt?.phase.type).toBe('running');
 expect(turnsRequests()).toHaveLength(2);
 await act(async()=>{server.durableUpdate('A',{...server.snapshots.get('A')!,attempt:{attempt_id:'live',phase:{type:'running'},turn:1,in_flight:{message_id:'more',blocks:[{type:'text',block_index:0,text:'same attempt text delta'}]}}});});
 expect(turnsRequests()).toHaveLength(2);
 fireEvent.click(mark());expect(navigate).toHaveBeenCalledWith(expect.objectContaining({id,cursor:'1'}));
 ui!.rerender(<TurnNavigator client={server.client} sessionId="A" active={'turn:["conversation-A","history"]'} onNavigate={navigate}/>);
 expect(mark().hasAttribute('aria-current')).toBe(false);
 ui!.rerender(<TurnNavigator client={server.client} sessionId="A" active={'turn:["conversation-A","live"]'} onNavigate={navigate}/>);
 expect(mark().getAttribute('aria-current')).toBe('true');
});

it('a loaded rail mark outside the transcript window reads its native window while the independent live tail keeps appending', async()=>{
 installTurnNavigatorObserver();
 server=new Server();await server.attached('A');
 const old={id:{conversation_id:'conversation-A',attempt_id:'old'},ordinal:1,cursor:'1',prompt:'first',response:'answer'};
 const live={id:{conversation_id:'conversation-A',attempt_id:'live'},ordinal:2,cursor:'600',prompt:'current',response:''};
 const cut={conversation_id:'conversation-A',journal:'600',transcript:'600',mutation_revision:'0'};
 const owner={...live.id,control_cursor:'600',message_count:1,tool_call_count:0,outcome:'running' as const};
 const row=(cursor:string)=>({cursor,item:{type:'message' as const,message:{role:'assistant' as const,id:`m${cursor}`,content:[{type:'text' as const,text:`output ${cursor}`}]}},turn_process:owner});
 const s={...server.snapshots.get('A')!,attempt:{attempt_id:'live',phase:{type:'running' as const},turn:1},transcript:{entries:[row('600')],next_cursor:'600'}};
 server.snapshots.set('A',s);await server.client.refresh('A');
 server.handlers.set('session/turns',()=>({type:'conversation_turns',page:{cut,offset:0,total:2,turns:[old,live]}}));
 let work:Promise<false | ConversationTurn>|undefined,ui:ReturnType<typeof render>;
 await act(async()=>{ui=render(<TurnNavigator client={server.client} sessionId="A" onNavigate={turn=>{work=server.client.navigateTurn('A',turn);}}/>);});
 const mark=ui!.getByRole('button',{name:'Jump to turn 1'});
 await act(async()=>{server.durableUpdate('A',{...s,transcript:{entries:[row('600'),row('601')]}});});
 expect(mark.hasAttribute('aria-disabled')).toBe(false);expect(server.client.getSnapshot().views.A.snapshot?.attempt?.phase.type).toBe('running');
 expect(turnsRequests()).toHaveLength(1);
 server.held.add('session/transcript');
 fireEvent.click(mark);const request=await server.waitFor('session/transcript',1);
 expect(request.params).toMatchObject({limit:64});expect(request.params).toHaveProperty('at',{type:'turn',id:old.id,cut});
 expect(mark.getAttribute('aria-busy')).toBe('true');
 await act(async()=>{server.socket.success(request,{type:'transcript_window',window: { cut, target:old.id, target_cursor:'1', page:{entries:[{...row('1'),turn_process:{...old.id,control_cursor:'1',message_count:1,tool_call_count:0,outcome:'completed'}}]}}});expect(await work).toEqual(old);});
 expect(mark.hasAttribute('aria-busy')).toBe(false);
 expect(server.client.getSnapshot().views.A.history?.page.entries?.map(row=>row.cursor)).toEqual(['1']);
 // The historical window and current tail have separate read authority.
 await act(async()=>{server.durableUpdate('A',{...s,transcript:{entries:[row('600'),row('601'),row('602')]}});});
 expect(server.client.getSnapshot().views.A.history?.page.entries?.map(row=>row.cursor)).toEqual(['1']);
 expect(server.client.getSnapshot().views.A.snapshot?.transcript.entries?.map(row=>row.cursor)).toEqual(['600','601','602']);
 // Active reading belongs to ChatViewport, separately tested in scroll.test.tsx.
 ui!.rerender(<TurnNavigator client={server.client} sessionId="A" active={'turn:["conversation-A","old"]'} onNavigate={()=>{}}/>);
 expect(mark.getAttribute('aria-current')).toBe('true');
 expect(turnsRequests()).toHaveLength(1);
});

for(const initial of [64,128])it(`latest outline follows ${initial} → ${initial+1} through start, location and settlement`,async()=>{
 const fixture=await pagingRail(initial);const n=initial+1;
 await fixture.start(n);
 let current=fixture.mark(n);expect(current).toBeTruthy();expect(current!.getAttribute('aria-disabled')).toBe('true');
 await fixture.locate(n);current=fixture.mark(n);expect(current!.hasAttribute('aria-disabled')).toBe(false);expect(current!.dataset.turnOrdinal).toBe(String(n));
 await fixture.settle(n);
 expect(fixture.mark(n)!.hasAttribute('aria-disabled')).toBe(false);expect(fixture.mark(n)!.dataset.turnOrdinal).toBe(String(n));
 expect(server.client.getSnapshot().views.A.turnOutline?.paging).toEqual({type:'latest'});
 expect(server.client.getSnapshot().views.A.turnOutline?.page?.offset).toBe(initial);
 expect(fixture.offsets()).toEqual([null,null,null,null]);
});

it('an explicit historical page survives live growth; an unloaded newest mark restores native latest intent',async()=>{
 const fixture=await pagingRail(128);
 // Following the latest output places the newest marks; the reader scrolls up.
 expect(fixture.ui.queryByRole('button',{name:'Load and jump to turn 1'})).toBeNull();
 await scrollRail(fixture.ui,0);
 await act(async()=>{fireEvent.click(fixture.ui.getByRole('button',{name:'Load and jump to turn 1'}));});
 expect(server.client.getSnapshot().views.A.turnOutline?.paging).toEqual({type:'page',offset:0});
 expect(fixture.navigate).toHaveBeenCalledExactlyOnceWith(1);
 expect(server.client.getSnapshot().views.A.history?.window?.target?.attempt_id).toBe('a1');
 await fixture.start(129);await fixture.locate(129);await fixture.settle(129);
 const outline=server.client.getSnapshot().views.A.turnOutline!;
 expect(outline.paging).toEqual({type:'page',offset:0});expect(outline.page?.turns.map(turn=>turn.ordinal)).toEqual(Array.from({length:64},(_,i)=>i+1));
 expect(fixture.mark(129)).toBeNull();expect(fixture.offsets()).toEqual([null,0,0,0,0]);
 await scrollRail(fixture.ui,1000);
 await act(async()=>{fireEvent.click(fixture.ui.getByRole('button',{name:'Load and jump to turn 129'}));});
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
 expect(fixture.mark(65)!.dataset.turnOrdinal).toBe('65');expect(fixture.mark(65)!.hasAttribute('aria-disabled')).toBe(false);
 expect(server.client.getSnapshot().views.A.turnOutline?.paging).toEqual({type:'latest'});
 expect(fixture.offsets()).toEqual([null,null,null]);
});

async function pagingRail(initial:number){
 installTurnNavigatorObserver();
 server=new Server();await server.attached('A');let total=initial,location:number|undefined;
 server.handlers.set('session/turns',request=>{
  if(request.method!=='session/turns')throw new Error('outline method');
  const offset=request.params.offset??Math.floor((total-1)/64)*64;
  return {type:'conversation_turns',page:{cut:{conversation_id:'conversation-A',journal:String(total),transcript:String(location??initial),mutation_revision:'0'},offset,total,
   turns:Array.from({length:Math.min(64,total-offset)},(_,i)=>{const n=offset+i+1;return {id:{conversation_id:'conversation-A',attempt_id:`a${n}`},ordinal:n,cursor:n>initial && n!==location?null:String(n),prompt:`turn ${n}`,response:''};})}};
 });
 const navigate=navigationIntent();
 let ui:ReturnType<typeof render>;await act(async()=>{ui=render(<TurnNavigator client={server.client} sessionId="A" onNavigate={navigate}/>);});
 const update=async(n:number,settled=false,phaseChange=false)=>{const s=server.snapshots.get('A')!;const next:RuntimeClientSnapshot={...s,attempt:{attempt_id:`a${n}`,turn:1,phase:settled?{type:'settled',outcome:{type:'completed',finish_reason:{type:'stop'}}}:{type:'running'}},transcript:{entries:location?[{cursor:String(location),item:{type:'message',message:{role:'assistant',id:`m${location}`,content:[{type:'text',text:'native output'}]}},turn_process:{conversation_id:'conversation-A',attempt_id:`a${n}`,control_cursor:String(location),message_count:1,tool_call_count:0,outcome:settled?'completed' as const:'running' as const}}]:[]}};await act(async()=>{server.snapshots.set('A',next);if(phaseChange)await server.client.refresh('A');else server.durableUpdate('A',next);});};
 return {ui:ui!,navigate,mark:(n:number)=>ui!.container.querySelector<HTMLButtonElement>(`[data-turn-id='["conversation-A","a${n}"]']`),
  offsets:()=>server.requests.filter(row=>row.request.method==='session/turns').map(row=>row.request.method==='session/turns'?row.request.params.offset:undefined),
  start:async(n:number)=>{total=n;location=undefined;await update(n,false,true);},locate:async(n:number)=>{location=n;await update(n);},settle:async(n:number)=>{await update(n,true,true);}};
}

it('observed unknown detached ownership suppresses the unrelated live fallback', async () => {
 installTurnNavigatorObserver();
 server=new Server();await server.attached('A');
 const id={conversation_id:'conversation-A',attempt_id:'live'},cut={conversation_id:'conversation-A',journal:'1',transcript:'1',mutation_revision:'0'};
 const s=server.snapshots.get('A')!;
 server.snapshots.set('A',{...s,attempt:{attempt_id:'live',phase:{type:'running'},turn:1}});await server.client.refresh('A');
 const turn={id,ordinal:2,cursor:'1',prompt:'',response:''};
 server.handlers.set('session/turns',()=>({type:'conversation_turns',page:{cut,offset:0,total:2,turns:[native(1),turn]}}));
 const ui=await act(async()=>render(<TurnNavigator client={server.client} sessionId="A" active={null} onNavigate={()=>{}}/>));
 expect(ui.container.querySelectorAll('[aria-current]')).toHaveLength(0);
 ui.rerender(<TurnNavigator client={server.client} sessionId="A" onNavigate={()=>{}}/>);
 expect(ui.container.querySelectorAll('[aria-current]')).toHaveLength(1);
});

it('a million native ordinals retain only one page and arithmetic accessors; allocation follows the visible range', () => {
 const total=1_000_000,offset=499_968;
 const page={cut:{conversation_id:'conversation-A',journal:'1',transcript:String(total),mutation_revision:'0'},offset,total,turns:Array.from({length:64},(_,i)=>native(offset+i+1))};
 let accesses=0;page.turns=new Proxy(page.turns,{get(target,key,receiver){if(typeof key==='string' && /^\d+$/.test(key))accesses++;return Reflect.get(target,key,receiver);}});
 const model=turnRail(page);
 expect(Object.keys(model)).toEqual(['count','item','indexOfKey','indexOfTurn']);
 expect(model.count).toBe(total);expect(accesses).toBe(0);
 for(const ordinal of [1,64,65,499_968,499_969,500_032,500_033,total]) {
  const item=model.item(ordinal-1)!;expect(item.key).toBe(`ordinal:${ordinal}`);expect(item.ordinal).toBe(ordinal);
  expect(item.turn).toEqual(ordinal>offset && ordinal<=offset+64?native(ordinal):undefined);
  expect(model.indexOfKey(item.key)).toBe(ordinal-1);
 }
 for(const top of [0,5_000_000,9_999_700]) {
  const range=turnRailRange(total,top,300,42);expect(range.length).toBeLessThanOrEqual(42);
  const marks=range.map(index=>model.item(index)!);expect(marks).toHaveLength(range.length);
 }
 expect(accesses).toBeLessThan(150);expect(page.turns).toHaveLength(64);
});
it('a million-Turn rail mounts bounded marks and navigates earliest, middle and latest native ordinals',async()=>{
 const {ui,navigate}=await thousandTurns(1_000_000);
 expect(ui.getByRole('button',{name:'Jump to turn 1000000'})).toBeTruthy();
 for(const ordinal of [1,500_001]) {
  await scrollRail(ui,(ordinal-1)*10);
  expect(within(ui.getByRole('navigation')).getAllByRole('button').length).toBeLessThan(72);
  await act(async()=>{fireEvent.click(ui.getByRole('button',{name:`Load and jump to turn ${ordinal}`}));});
  expect(navigate).toHaveBeenLastCalledWith(ordinal);
  expect(server.client.getSnapshot().views.A.history?.window?.target).toEqual(native(ordinal).id);
 }
 await scrollRail(ui,9_999_400);
 await act(async()=>{fireEvent.click(ui.getByRole('button',{name:'Load and jump to turn 1000000'}));});
 expect(navigate).toHaveBeenLastCalledWith(1_000_000);
 expect(server.client.getSnapshot().views.A.history?.window?.target).toEqual(native(1_000_000).id);
 expect(turnsRequests()).toHaveLength(4);
 expect(server.requests.filter(row=>row.request.method==='session/transcript')).toHaveLength(3);
});
it('overlapping unloaded clicks retain only the newest page demand and navigation',async()=>{
 const {ui}=await thousandTurns();server.held.add('session/turns');
 fireEvent.click(ui.getByRole('button',{name:'Load and jump to turn 960'}));const old=await server.waitFor('session/turns',2);
 await scrollRail(ui,0);fireEvent.click(ui.getByRole('button',{name:'Load and jump to turn 1'}));
 expect(turnsRequests()).toHaveLength(2);
 await act(async()=>server.reply(old));const next=await server.waitFor('session/turns',3);expect(next.params).toMatchObject({offset:0});
 expect(server.requests.filter(row=>row.request.method==='session/transcript')).toHaveLength(0);
 await act(async()=>server.reply(next));
 const windows=server.requests.filter(row=>row.request.method==='session/transcript');expect(windows).toHaveLength(1);
 expect(windows[0].request.params).toMatchObject({at:{type:'turn',id:native(1).id}});
 expect(server.client.getSnapshot().views.A.history?.window?.target).toEqual(native(1).id);
});

it('native invalidation removes rail geometry; explicit recovery places the current native Turn again',async()=>{
 const {ui}=await thousandTurns();
 expect(scroller(ui).scrollTop).toBeGreaterThan(9000);
 await act(async()=>{server.cursor++;server.socket.deliver({jsonrpc:'2.0',method:'session/event',params:{target:server.target('A'),cursor:String(server.cursor),event:{type:'pending_inbound_changed',pending:[]}}});});
 expect(ui.queryByRole('navigation')).toBeNull();
 const recovery=ui.getByRole('alert').querySelector('button')!;
 await act(async()=>fireEvent.click(recovery));
 expect(scroller(ui).scrollTop).toBeGreaterThan(9000);
 expect(ui.getByRole('button',{name:'Jump to turn 1000'}).getAttribute('aria-current')).toBe('true');
});
