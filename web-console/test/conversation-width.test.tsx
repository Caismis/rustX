import { act, cleanup, fireEvent, render, screen } from '@testing-library/react';
import { afterEach, beforeEach, expect, it, vi } from 'vitest';
import { ConversationWidthControls } from '../src/app/agent/ConversationWidthControls';
import { WIDTH_PREFERENCE_KEY, displayedWidth } from '../src/app/agent/conversation-width';
let column=1200, resize:()=>void, frame:FrameRequestCallback|undefined;
const stored=new Map<string,string>(), writes=vi.fn((key:string,value:string)=>stored.set(key,value));
beforeEach(()=>{
 column=1200;stored.clear();writes.mockClear();frame=undefined;
 vi.stubGlobal('localStorage',{getItem:(key:string)=>stored.get(key)??null,setItem:writes});
 vi.stubGlobal('PointerEvent',class extends MouseEvent {pointerId:number;constructor(type:string,init:PointerEventInit){super(type,init);this.pointerId=init.pointerId??1;}});
 vi.stubGlobal('ResizeObserver',class{constructor(callback:()=>void){resize=callback;}observe(){}disconnect(){}});
 vi.stubGlobal('requestAnimationFrame',(callback:FrameRequestCallback)=>{frame=callback;return 1;});vi.stubGlobal('cancelAnimationFrame',()=>{frame=undefined;});
 vi.spyOn(HTMLElement.prototype,'getBoundingClientRect').mockImplementation(()=>({width:column,height:600,top:0,left:0,right:column,bottom:600,x:0,y:0,toJSON(){}}));
 const captures=new WeakMap<HTMLElement,number>();
 HTMLElement.prototype.setPointerCapture=function(id){captures.set(this,id);};HTMLElement.prototype.hasPointerCapture=function(id){return captures.get(this)===id;};HTMLElement.prototype.releasePointerCapture=function(){captures.delete(this);};
});
afterEach(()=>{cleanup();vi.unstubAllGlobals();vi.restoreAllMocks();});
const mount=()=>{const ui=render(<section><ConversationWidthControls active/></section>);return{...ui,body:ui.container.querySelector('section')!,handle:(side:'left'|'right')=>screen.getByRole('slider',{name:side==='left'?'Resize conversation from the left':'Resize conversation from the right'})};};
const flush=()=>act(()=>{const cb=frame;frame=undefined;cb?.(0);});
for(const side of ['left','right'] as const)it(`${side} pointer capture coalesces live updates and commits preference only at successful release`,()=>{
 const ui=mount(), handle=ui.handle(side), start=Number(handle.getAttribute('aria-valuenow')), x=side==='left'?80:120;
 fireEvent.pointerDown(handle,{pointerId:1,button:0,clientX:100});
 for(let n=0;n<10;n++)fireEvent.pointerMove(handle,{pointerId:1,clientX:x,clientY:200});
 expect(writes).not.toHaveBeenCalled();expect(ui.body.style.getPropertyValue('--dsh-chat-content-width')).toBe(`${start}px`);
 flush();expect(ui.body.style.getPropertyValue('--dsh-chat-content-width')).toBe(`${start+40}px`);
 fireEvent.pointerUp(handle,{pointerId:1,clientX:x});expect(writes).toHaveBeenCalledOnce();expect(stored.get(WIDTH_PREFERENCE_KEY)).toBe(String(start+40));
});
it('press without meaningful movement retains a wider saved preference after temporary clamp',()=>{
 stored.set(WIDTH_PREFERENCE_KEY,'1000');column=900;const ui=mount(),handle=ui.handle('right');
 fireEvent.pointerDown(handle,{pointerId:1,button:0,clientX:100});fireEvent.pointerUp(handle,{pointerId:1,clientX:101});
 expect(writes).not.toHaveBeenCalled();expect(stored.get(WIDTH_PREFERENCE_KEY)).toBe('1000');column=1400;act(()=>resize());expect(ui.body.style.getPropertyValue('--dsh-chat-content-width')).toBe('1000px');
});
for(const cancellation of ['pointerCancel','lostPointerCapture','Escape'] as const)it(`${cancellation} cancels a drag and restores saved preference`,()=>{
 stored.set(WIDTH_PREFERENCE_KEY,'800');const ui=mount(),handle=ui.handle('right');
 fireEvent.pointerDown(handle,{pointerId:1,button:0,clientX:100});fireEvent.pointerMove(handle,{pointerId:1,clientX:150});flush();expect(ui.body.style.getPropertyValue('--dsh-chat-content-width')).toBe('900px');
 if(cancellation==='Escape')fireEvent.keyDown(handle,{key:'Escape'});else fireEvent[cancellation](handle,{pointerId:1});
 expect(ui.body.style.getPropertyValue('--dsh-chat-content-width')).toBe('800px');expect(writes).not.toHaveBeenCalled();flush();expect(ui.body.style.getPropertyValue('--dsh-chat-content-width')).toBe('800px');
});
it('ResizeObserver sidebar/right-panel clamps and restores deliberate preference; narrow columns expose no handles or overflow',()=>{
 stored.set(WIDTH_PREFERENCE_KEY,'920');const ui=mount();
 for(const available of [980,700,380,1200]){column=available;act(()=>resize());const width=Number.parseFloat(ui.body.style.getPropertyValue('--dsh-chat-content-width'));expect(width).toBe(displayedWidth(column,920));expect(width).toBeLessThan(column);expect(screen.queryAllByRole('slider')).toHaveLength(column>=816?2:0);}
 expect(ui.body.style.getPropertyValue('--dsh-chat-content-width')).toBe('920px');expect(writes).not.toHaveBeenCalled();
});
for(const value of ['garbage','NaN','-1','100000','0',' 900 ','{"width":900}'])it(`corrupt storage ${value} uses the measured default`,()=>{stored.set(WIDTH_PREFERENCE_KEY,value);expect(mount().body.style.getPropertyValue('--dsh-chat-content-width')).toBe('840px');});
it('blocked storage reads/writes and absent storage leave Chat width usable',()=>{
 vi.stubGlobal('localStorage',{getItem(){throw new Error('blocked');},setItem(){throw new Error('quota');}});
 const ui=mount(),handle=ui.handle('right');fireEvent.keyDown(handle,{key:'ArrowRight'});expect(ui.body.style.getPropertyValue('--dsh-chat-content-width')).toBe('856px');
 column=700;act(()=>resize());column=1200;act(()=>resize());expect(ui.body.style.getPropertyValue('--dsh-chat-content-width')).toBe('856px');ui.unmount();vi.stubGlobal('localStorage',undefined);expect(()=>mount()).not.toThrow();
});
it('keyboard width adjustments have accessible current bounds and reduced motion needs no animation',()=>{
 vi.stubGlobal('matchMedia',()=>({matches:true}));const ui=mount(),handle=ui.handle('left');fireEvent.keyDown(handle,{key:'ArrowRight',shiftKey:true});expect(handle.getAttribute('aria-valuenow')).toBe('904');
 fireEvent.keyDown(handle,{key:'Home'});expect(handle.getAttribute('aria-valuenow')).toBe('640');fireEvent.keyDown(handle,{key:'End'});expect(handle.getAttribute('aria-valuenow')).toBe('1024');expect(writes).toHaveBeenCalledTimes(3);expect(frame).toBeUndefined();
});
