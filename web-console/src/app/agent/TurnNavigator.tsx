/* Copyright (c) 2026 DeepSeek. MIT. Rail interaction patterns adapted; see PROVENANCE.md. */
import { useEffect, useId, useRef, useState } from 'react';
import type { ConversationTurn } from '../../../../protocol/app-server/v34';
import type { AppServerClient } from '../../client/app-server';
import { shallowEqual, useClientSelector } from '../../client/selectors';
import { turnKey } from '../../client/transcript';
import { useTranslation } from '../../locale/react';
import css from './TurnNavigator.module.css';
import { currentTurnLocation, turnRailItems } from './turn-rail-items';

/** One bounded native page with preview navigation and adjacent-page controls. */
export function TurnNavigator({ client, sessionId, onNavigate, active }: { client: AppServerClient; sessionId: string; onNavigate: (turn: ConversationTurn) => void; active?: string | null }) {
  const tx=useTranslation(), previewId=useId(), [preview,setPreview]=useState<string>();
  const drag = useRef<{ pointer: number; start: number; moved: boolean; key?: string }>(undefined);
  const suppressClick = useRef(false);
  const [previewTop, setPreviewTop] = useState(20);
  const showPreview = (key: string, button: HTMLButtonElement) => {
    const rail = button.closest('[data-turn-navigator]')!.getBoundingClientRect();
    const mark = button.getBoundingClientRect();
    setPreviewTop(Math.max(0, Math.min(mark.top - rail.top + mark.height / 2 - 50, rail.height - 120)));
    setPreview(key);
  };
  const view=useClientSelector(client,state=>{
    const view=state.views[sessionId];
    return {target:view?.target,attachment:view?.attachment,outline:view?.turnOutline,navigation:view?.turnNavigation,
      attempt:view?.snapshot?.attempt?.attempt_id,phase:view?.snapshot?.attempt?.phase.type,
      conversation:view?.snapshot?.conversation_id,running:!!view?.snapshot?.attempt && view.snapshot.attempt.phase.type!=='settled',
      location:currentTurnLocation(view?.snapshot),generation:state.generation};
  },shallowEqual);
  useEffect(()=>{
    if(view.attachment==='attached')void client.refreshTurns(sessionId);
  },[client,sessionId,view.target,view.attachment,view.attempt,view.phase,view.generation]);
  const currentId=view.running && view.conversation && view.attempt ? {conversation_id:view.conversation,attempt_id:view.attempt} : undefined;
  const page=view.outline?.page, turns=turnRailItems(page,currentId,view.location), selected=turns.find(turn=>turnKey(turn.id)===preview);
  const missingLocation=!!page && !!view.location && BigInt(page.cut.transcript)<BigInt(view.location);
  useEffect(()=>{if(missingLocation && !view.outline?.loading && !view.outline?.error && view.attachment==='attached')void client.refreshTurns(sessionId);},[client,sessionId,missingLocation,view.outline?.loading,view.outline?.error,view.attachment,page?.offset]);
  const error=view.navigation?.error ?? view.outline?.error;
  if(page?.total===0 && !currentId && !view.outline?.loading && !error)return null;
  return <aside className={css.root} aria-label={tx('agent:reading.turn-navigation')} data-turn-navigator>
    <button type="button" aria-label={tx('agent:reading.older-turns')} disabled={!page || page.offset===0 || view.outline?.loading} onClick={()=>void client.readTurns(sessionId,Math.max(0,page!.offset-64))}>↑</button>
    <div className={css.marks} aria-busy={view.outline?.loading || undefined} onPointerLeave={event=>{if(!drag.current)setPreview(event.currentTarget.querySelector<HTMLElement>(':focus')?.dataset.turnId);}}
      onPointerDown={event=>{if(event.button!==0)return; suppressClick.current=false; drag.current={pointer:event.pointerId,start:event.clientY,moved:false};}}
      onPointerMove={event=>{
        const gesture=drag.current;
        if(!gesture || gesture.pointer!==event.pointerId)return;
        if(!gesture.moved && Math.abs(event.clientY-gesture.start)<4)return;
        gesture.moved=true;
        event.currentTarget.setPointerCapture(event.pointerId);
        const rect=event.currentTarget.getBoundingClientRect();
        if(event.clientY<rect.top+16)event.currentTarget.scrollTop-=10;
        if(event.clientY>rect.bottom-16)event.currentTarget.scrollTop+=10;
        const buttons=[...event.currentTarget.querySelectorAll<HTMLButtonElement>('button:not(:disabled)')];
        const nearest=buttons.reduce<HTMLButtonElement|undefined>((best,button)=>{
          const distance=(el:HTMLButtonElement)=>Math.abs(el.getBoundingClientRect().top+el.offsetHeight/2-event.clientY);
          return !best || distance(button)<distance(best)?button:best;
        },undefined);
        if(nearest){gesture.key=nearest.dataset.turnId;showPreview(gesture.key!,nearest);}
      }}
      onPointerUp={event=>{
        const gesture=drag.current;drag.current=undefined;
        if(event.currentTarget.hasPointerCapture(event.pointerId))event.currentTarget.releasePointerCapture(event.pointerId);
        if(gesture?.moved){suppressClick.current=true;const turn=turns.find(turn=>turnKey(turn.id)===gesture.key);if(turn)onNavigate(turn);setPreview(undefined);}
      }}
      onPointerCancel={()=>{drag.current=undefined;setPreview(undefined);}}
      onClickCapture={event=>{if(suppressClick.current){event.preventDefault();event.stopPropagation();suppressClick.current=false;}}}>
      {turns.map(turn=>{
        const key=turnKey(turn.id), current=active !== undefined ? active===`turn:${key}` : view.navigation?.active===key || !view.navigation?.active && turnKey(currentId ?? {conversation_id:'',attempt_id:''})===key;
        return <button key={key} type="button" className={css.mark} data-turn-id={key} data-turn-ordinal={turn.ordinal}
          aria-label={turn.ordinal ? tx('agent:reading.jump-turn',{n:turn.ordinal}) : tx('agent:reading.current-turn')} aria-current={current?'true':undefined}
          aria-busy={view.navigation?.pending===key || undefined} aria-describedby={selected===turn?previewId:undefined}
          disabled={turn.cursor==null || view.attachment!=='attached'} onClick={()=>onNavigate(turn)} onPointerEnter={event=>showPreview(key,event.currentTarget)} onFocus={event=>showPreview(key,event.currentTarget)} onBlur={()=>setPreview(undefined)}
          onKeyDown={event=>{
            if(!['ArrowUp','ArrowDown','Home','End'].includes(event.key))return;
            event.preventDefault(); const buttons=[...event.currentTarget.parentElement!.querySelectorAll<HTMLButtonElement>('button:not(:disabled)')];
            const index=buttons.indexOf(event.currentTarget), next=event.key==='Home'?0:event.key==='End'?buttons.length-1:index+(event.key==='ArrowDown'?1:-1);
            buttons[Math.max(0,Math.min(buttons.length-1,next))]?.focus();
          }}><span/></button>;
      })}
    </div>
    <button type="button" aria-label={tx('agent:reading.newer-turns')} disabled={!page || view.outline?.paging.type==='latest' && page.offset+page.turns.length>=page.total || view.outline?.loading} onClick={()=>void (page!.offset+64>=Math.floor((page!.total-1)/64)*64 ? client.readTurns(sessionId) : client.readTurns(sessionId,page!.offset+64))}>↓</button>
    {selected && <div id={previewId} role="tooltip" className={css.preview} style={{ top: previewTop }}><strong>{selected.prompt || (selected.ordinal ? tx('agent:reading.turn', { n: selected.ordinal }) : tx('agent:reading.current-turn'))}</strong>{selected.response && <p>{selected.response}</p>}</div>}
    {view.outline?.loading && <span className={css.feedback} role="status">{tx('agent:reading.loading-turns')}</span>}
    {error && <div className={css.feedback} role="alert"><p>{error}</p><button type="button" onClick={()=>{client.invalidateReading(sessionId);void client.refreshTurns(sessionId);}}>{tx('agent:reading.reload-turns')}</button></div>}
  </aside>;
}
