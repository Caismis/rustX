import { useCallback, useEffect, useLayoutEffect, useMemo, useRef, useState } from 'react';
import { DeferredContent } from '../agent/DeferredContent';
import { CollectionPages, type CollectionRead } from '../agent/CollectionPages';
import type { ContentRef } from '../../../../protocol/app-server/v50';
import type { AppServerClient, OperationAdmission } from '../../client/app-server';
import { extendTranscriptWindow, installTranscriptWindow, sameReadCut } from '../../client/transcript';
import { ChatViewport } from '../../presentation/layout/ChatViewport';
import { AgentTranscript } from '../agent/AgentTranscript';
import { Button } from '../../presentation/primitives/Button';
import { useTranslation } from '../../locale/react';
import type { HistoryRoute } from './machine';
import type { SettingsNavigationActor } from '../settings/machines/navigation';
import css from '../../presentation/agent/Conversation.module.css';

/** Immutable native read identity, never an executable SessionView. */
export function HistorySurface({ client, route, current, back, settings }: { client: AppServerClient; route: HistoryRoute; current: () => boolean; back: () => void; settings: SettingsNavigationActor }) {
  const tx = useTranslation(), viewport = useRef<ChatViewport>(null);
  const [history, setHistory] = useState(() => installTranscriptWindow(route.window));
  const [error, setError] = useState(''), [loading, setLoading] = useState(false);
  const [readEpoch, setReadEpoch] = useState(0);
  const contentOwner = useMemo(() => Symbol('historical content view'), [route, readEpoch]);
  const owner = useRef<symbol | undefined>(contentOwner); owner.current = contentOwner;
  useEffect(() => () => { if(owner.current===contentOwner)owner.current=undefined; }, [contentOwner]);
  const ownsContent = useCallback(() => owner.current === contentOwner && current(), [contentOwner, current]);
  const contentRead = useCallback(async (content_ref:ContentRef, offset:number, readCurrent:()=>boolean) => {
    const cut=history.page.source_cut;
    if(!cut || !ownsContent())throw new Error('Historical content source is unavailable.');
    return (await client.request({method:'session/historyContentRead',params:{session_id:route.sessionId,node_id:route.nodeId,cut,content_ref,offset,max_bytes:65_536}},'content_chunk',undefined,()=>ownsContent()&&readCurrent())).chunk;
  }, [client, route, history.page.source_cut, ownsContent]);
  const read = useRef<AbortController | undefined>(undefined);
  const collectionRead=useMemo<CollectionRead>(()=>({identity:JSON.stringify([route.sessionId,route.nodeId,history.page.source_cut]),
    page:async(domain,offset,_previous,readCurrent)=>{const cut=history.page.source_cut;if(!cut)throw new Error('Historical collection cut is unavailable.');return (await client.request({method:'session/historyPage',params:{session_id:route.sessionId,node_id:route.nodeId,cut,domain,offset,limit:32}},'checkpoint_page',undefined,()=>ownsContent()&&readCurrent())).page;},
    value:async(page,item,offset,readCurrent)=>{const cut=history.page.source_cut;if(!cut||JSON.stringify(page.read_cut.source_cut)!==JSON.stringify(cut))throw new Error('Historical collection cut changed.');return (await client.request({method:'session/historyValueRead',params:{session_id:route.sessionId,node_id:route.nodeId,cut,domain:page.domain,item_offset:Number(item.offset),path:[],offset,max_bytes:65_536}},'checkpoint_value_chunk',undefined,()=>ownsContent()&&readCurrent())).chunk;}
  }),[client,route,history.page.source_cut,ownsContent]);
  useEffect(() => {
    const subscription = settings.subscribe(() => { read.current?.abort(); read.current = undefined; setLoading(false); setReadEpoch(epoch=>epoch+1); });
    return () => { subscription.unsubscribe(); read.current?.abort(); };
  }, [settings]);
  useLayoutEffect(() => { if(route.messageId!=null)viewport.current?.beginNavigation().commit(`message:${route.messageId}`, current); }, [route, current]);
  const page = async (older: boolean) => {
    if (loading || !current()) return;
    const cursor = older ? history.page.next_cursor : history.window?.newer_cursor;
    if (!cursor) return;
    const controller = new AbortController(); read.current?.abort(); read.current = controller;
    const valid = () => current() && read.current === controller && !controller.signal.aborted;
    let allowed: false | OperationAdmission = false;
    setLoading(true); setError('');
    try {
      allowed = await client.admitSessionOperation(route.sessionId, valid);
      if (!allowed || !valid()) return;
      const result = await client.request({ method: 'session/history', params: { session_id: route.sessionId, node_id: route.nodeId, at: older ? { type: 'older', before: cursor, cut: route.window.cut } : { type: 'newer', after: cursor, cut: route.window.cut }, limit: 32 } }, 'session_history', undefined, allowed);
      if (!valid() || !allowed.current()) return;
      if (result.conversation_id !== route.conversationId || !sameReadCut(result.window.cut, route.window.cut)) throw new Error('Historical read identity changed.');
      setHistory(previous => extendTranscriptWindow(result.window, previous, older));
    } catch (cause) { if (valid() && (!allowed || allowed.current())) setError(String(cause)); }
    finally { if (valid()) { read.current = undefined; setLoading(false); } }
  };
  return <section className={css.root} data-history-session={route.sessionId} data-history-node={route.nodeId} data-history-message={route.messageId}>
    <header><h1 id="session-title">{tx('agent:history.title')}</h1><p>{tx('agent:history.read-only')}</p><Button onClick={back}>{tx('agent:history.return')}</Button></header>
    <div className={css.body}><div className={css.scrollBody} data-conversation-scroll>
      <ChatViewport ref={viewport} historical chromeVisible={false}>
        {error && <p role="alert">{error}</p>}
        <AgentTranscript snapshot={{ conversation_id: route.conversationId, messages: [], attempt: null, statuses: [], transcript: history.page }} history={history} historicalDisabled sourceDisabled
          loadEarlier={!loading && history.page.next_cursor ? () => void page(true) : undefined} loadLater={!loading && history.window?.newer_cursor ? () => void page(false) : undefined}/>
        <CollectionPages read={collectionRead} collections={history.page.collections ?? []} owned={ownsContent}/>
        <DeferredContent bindings={history.page.content ?? []} read={contentRead} owned={ownsContent}/>
      </ChatViewport>
    </div></div>
  </section>;
}
