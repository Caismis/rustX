import { useEffect, useLayoutEffect, useRef, useState } from 'react';
import { JournalSourceProvider } from '../agent/ObservationSource';
import type { AppServerClient } from '../../client/app-server';
import type { OperationAdmission } from '../../client/contracts';
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
  const [contentEpoch, setContentEpoch] = useState(0);
  const read = useRef<AbortController | undefined>(undefined);
  useEffect(() => {
    const subscription = settings.subscribe(() => { read.current?.abort(); read.current = undefined; setLoading(false); setContentEpoch(epoch => epoch + 1); });
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
        <JournalSourceProvider source={{client,sessionId:route.sessionId,nodeId:route.nodeId,historyCut:history.page.source_cut??undefined,cut:()=>undefined,epoch:contentEpoch,owned:()=>current()&&!settings.getSnapshot().context.page&&!settings.getSnapshot().matches('resolvingOwner')}} content={history.page.content} transcript={history.page}><AgentTranscript snapshot={{ conversation_id: route.conversationId, messages: [], attempt: null, statuses: [], transcript: history.page }} history={{ ...history, loading }} historicalDisabled sourceDisabled
          loadEarlier={!loading && history.page.next_cursor ? () => void page(true) : undefined} loadLater={history.window?.newer_cursor ? () => page(false) : undefined}/></JournalSourceProvider>
      </ChatViewport>
    </div></div>
  </section>;
}
