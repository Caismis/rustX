/* Copyright (c) 2026 DeepSeek. MIT. Rail interaction patterns adapted; see PROVENANCE.md. */
import { useEffect, useId, useState } from 'react';
import type { ConversationTurn } from '../../../../protocol/app-server/v35';
import type { AppServerClient } from '../../client/app-server';
import { shallowEqual, useClientSelector } from '../../client/selectors';
import { turnKey } from '../../client/transcript';
import { useTranslation } from '../../locale/react';
import css from './TurnNavigator.module.css';
import { currentTurnLocation, turnRailItems } from './turn-rail-items';

/** One bounded native page, with direct ordinal access to any distant page. */
export function TurnNavigator({ client, sessionId, onNavigate, active }: { client: AppServerClient; sessionId: string; onNavigate: (turn: ConversationTurn) => void; active?: string | null }) {
  const tx=useTranslation(), previewId=useId(), [preview,setPreview]=useState<string>(), [ordinal,setOrdinal]=useState('');
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
    <div className={css.marks} aria-busy={view.outline?.loading || undefined} onPointerLeave={event=>setPreview(event.currentTarget.querySelector<HTMLElement>(':focus')?.dataset.turnId)}>
      {turns.map(turn=>{
        const key=turnKey(turn.id), current=active !== undefined ? active===`turn:${key}` : view.navigation?.active===key || !view.navigation?.active && turnKey(currentId ?? {conversation_id:'',attempt_id:''})===key;
        return <button key={key} type="button" className={css.mark} data-turn-id={key} data-turn-ordinal={turn.ordinal}
          aria-label={turn.ordinal ? tx('agent:reading.jump-turn',{n:turn.ordinal}) : tx('agent:reading.current-turn')} aria-current={current?'true':undefined}
          aria-busy={view.navigation?.pending===key || undefined} aria-describedby={selected===turn?previewId:undefined}
          disabled={turn.cursor==null || view.attachment!=='attached'} onClick={()=>onNavigate(turn)} onPointerEnter={()=>setPreview(key)} onFocus={()=>setPreview(key)} onBlur={()=>setPreview(undefined)}
          onKeyDown={event=>{
            if(!['ArrowUp','ArrowDown','Home','End'].includes(event.key))return;
            event.preventDefault(); const buttons=[...event.currentTarget.parentElement!.querySelectorAll<HTMLButtonElement>('button:not(:disabled)')];
            const index=buttons.indexOf(event.currentTarget), next=event.key==='Home'?0:event.key==='End'?buttons.length-1:index+(event.key==='ArrowDown'?1:-1);
            buttons[Math.max(0,Math.min(buttons.length-1,next))]?.focus();
          }}><span/></button>;
      })}
    </div>
    <button type="button" aria-label={tx('agent:reading.newer-turns')} disabled={!page || view.outline?.paging.type==='latest' && page.offset+page.turns.length>=page.total || view.outline?.loading} onClick={()=>void (page!.offset+64>=Math.floor((page!.total-1)/64)*64 ? client.readTurns(sessionId) : client.readTurns(sessionId,page!.offset+64))}>↓</button>
    <form className={css.index} onSubmit={event=>{event.preventDefault();const value=Number(ordinal);if(page && Number.isInteger(value) && value>=1 && value<=page.total)void client.readTurns(sessionId,Math.floor((value-1)/64)*64);}}>
      <input type="number" min={1} max={page?.total ?? 1} value={ordinal} onChange={event=>setOrdinal(event.target.value)} aria-label={tx('agent:reading.turn-number')}/>
      <button type="submit" disabled={!page || view.outline?.loading} aria-label={tx('agent:reading.show-turn-page')}>↵</button>
    </form>
    {selected && <div id={previewId} role="tooltip" className={css.preview}><strong>{selected.ordinal ? tx('agent:reading.jump-turn',{n:selected.ordinal}) : tx('agent:reading.current-turn')}</strong><p>{selected.preview || tx('agent:reading.no-preview')}</p></div>}
    {view.outline?.loading && <span className={css.feedback} role="status">{tx('agent:reading.loading-turns')}</span>}
    {error && <div className={css.feedback} role="alert"><p>{error}</p><button type="button" onClick={()=>{client.invalidateReading(sessionId);void client.refreshTurns(sessionId);}}>{tx('agent:reading.reload-turns')}</button></div>}
  </aside>;
}
