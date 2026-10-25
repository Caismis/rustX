/* Copyright (c) 2026 DeepSeek. MIT. Adapted ModelRetryItem; see PROVENANCE.md. */
import {attemptOwnerKey} from '../../../../protocol/app-server/attempt-identity';
import { useEffect, useMemo, useRef, useState, type ReactNode } from 'react';
import { traceFieldKey } from '../../../../protocol/app-server/trace-identity';
import { readTraceFieldWindow } from '../../../../protocol/app-server/trace-field';
import type { RuntimeClientSnapshot, RuntimeClientTranscriptEntry } from '../../../../protocol/app-server/v57';
import type { AppServerClient } from '../../client/app-server';
import type { ClientView, SessionView } from '../../client/contracts';
import { useClientSelector } from '../../client/selectors';
import { modelRetryPlacements, type ModelRetryNotice } from '../../bindings/model-retry';
import type { RequestFeedback } from './AgentTranscript';
import { useTranslation } from '../../locale/react';
import { TextShimmer } from '../../presentation/primitives/TextShimmer';
import { TraceFields } from '../trajectory/TraceFields';
import css from './ModelRetry.module.css';

interface RetrySelection {
  placements: ReturnType<typeof modelRetryPlacements>;
  attached: boolean;
  conversationId?: string;
  attempt: RuntimeClientSnapshot['attempt'] | undefined;
}
function failureSelection(view:SessionView|undefined,id:string|undefined) {
  const record=view?.trace?.page.records.find(row=>row.id===id);
  return {id:record?.id,field:record?.fields.find(field=>field.field==='failure'),
    cut:record?view?.trace?.readCuts?.[record.id]:undefined,reader:view?.trace?.fields,admission:view?.attachmentObservation};
}
function sameFailureSource(a:ReturnType<typeof failureSelection>,b:ReturnType<typeof failureSelection>) {
  return a.id===b.id&&a.reader===b.reader&&a.admission===b.admission&&!!a.cut===!!b.cut
    &&(a.field?traceFieldKey(a.field):undefined)===(b.field?traceFieldKey(b.field):undefined);
}
function sameRetries(a: RetrySelection, b: RetrySelection) {
  return a.conversationId===b.conversationId && a.attached === b.attached && a.attempt === b.attempt && a.placements.length === b.placements.length
    && a.placements.every((row, index) => {
      const other = b.placements[index];
      return row.beforeMessageId === other.beforeMessageId && row.notice.key === other.notice.key
        && row.notice.request === other.notice.request && row.notice.failure === other.notice.failure;
    });
}

export function useModelRetryFeedback(client: AppServerClient, sessionId: string, entries: readonly RuntimeClientTranscriptEntry[], streamingId?: string): RequestFeedback {
  // A retry can move from the no-publication seat to its streamed/canonical
  // message. The disclosure belongs to the retry chain, not that render seat.
  const [expanded, setExpanded] = useState<ReadonlySet<string>>(new Set());
  const messageIds = useMemo(() => {
    const ids = new Set(entries.flatMap(entry => entry.item.type === 'message' ? [entry.item.message.id]
      : entry.item.type === 'publication_audit' ? [entry.item.audit.message_id] : []));
    if (streamingId) ids.add(streamingId);
    return ids;
  }, [entries, streamingId]);
  const view = useClientSelector(client, state => {
    const value = state.views[sessionId];
    return { placements: modelRetryPlacements(value?.trace?.page.records ?? [], messageIds),
      conversationId:value?.snapshot?.conversation_id,attached: value?.attachment === 'attached', attempt: value?.snapshot?.attempt };
  }, sameRetries);
  return useMemo(() => {
    const messages = new Map<string, ReactNode[]>(), attempts = new Map<string, ReactNode[]>();
    for (const { notice, beforeMessageId } of view.placements) {
      const map = beforeMessageId ? messages : attempts;
      if(!view.conversationId)continue;
      const ownerKey=attemptOwnerKey({conversation_id:view.conversationId,...notice.owner});
      const key = beforeMessageId ?? ownerKey;
      const list = map.get(key) ?? [];
      list.push(<RetryNotice key={notice.key} notice={notice} open={expanded.has(notice.key)} onToggle={open => setExpanded(previous => {
        if (previous.has(notice.key) === open) return previous;
        const next = new Set(previous); if (open) next.add(notice.key); else next.delete(notice.key); return next;
      })} attached={view.attached} client={client} sessionId={sessionId}
        running={!!view.attempt && attemptOwnerKey({conversation_id:view.conversationId,...view.attempt})===ownerKey && view.attempt.phase.type === 'running'
          && notice.request.request?.assistant_message_id!=null && view.attempt.in_flight?.message_id === notice.request.request.assistant_message_id}/>);
      map.set(key, list);
    }
    return { messages, attempts };
  }, [client, sessionId, view.placements, view.attached, view.attempt, view.conversationId, expanded]);
}
function RetryNotice({ notice, open, onToggle, attached, running, client, sessionId }: {
  notice: ModelRetryNotice; open: boolean; onToggle: (open: boolean) => void; attached: boolean; running: boolean; client: AppServerClient; sessionId: string;
}) {
  const tx = useTranslation();
  const select=useMemo(()=>(state:ClientView)=>failureSelection(state.views[sessionId],notice.failure?.id),[sessionId,notice.failure?.id]);
  const source=useClientSelector(client,select,sameFailureSource);
  // Keep the original field's cut across later retry progress. A replacement
  // attachment or original source retires the read, rather than retagging it.
  const selected=useRef(source);selected.current=source;
  const generation=useRef(0);
  const [failure,setFailure]=useState<{source:typeof source;message?:string;loading?:boolean;error?:boolean}>();
  const loaded=failure?.source===source?failure:undefined;
  const owned=()=>selected.current===source&&client.isAttachmentObservationCurrent(sessionId,source.admission)
    &&!!source.field&&client.getSnapshot().views[sessionId]?.trace?.page.records.some(row=>row.id===source.id&&row.fields.some(field=>traceFieldKey(field)===traceFieldKey(source.field!)))===true;
  const large=source.field?.type==='content'||source.field?.type==='value'&&(!/^(0|[1-9][0-9]{0,19})$/.test(source.field.encoded_bytes)||BigInt(source.field.encoded_bytes)>65_536n);
  const read=async()=>{
    const field=source.field,token=++generation.current,current=()=>token===generation.current&&owned();
    if(!attached||!field||field.type!=='value'||large||!current())return;
    if(!source.cut||!source.reader){setFailure({source,error:true});return;}
    setFailure({source,loading:true});
    try {
      const chunk=await readTraceFieldWindow(field,source.cut,0,at=>source.reader!.value(source.cut!,field,at,current),current);
      if(chunk.next!=null||chunk.representation!=='canonical_json_v1')throw Error('Original failure is not a complete JSON field.');
      const value:unknown=JSON.parse(new TextDecoder('utf-8',{fatal:true,ignoreBOM:true}).decode(chunk.bytes));
      if(!value||typeof value!=='object'||!('message' in value)||typeof value.message!=='string'
        ||!('kind' in value)||value.kind!==notice.failure?.request?.failure_kind)throw Error('Original failure identity changed.');
      if(current())setFailure({source,message:value.message});
    }catch{if(current())setFailure({source,error:true});}
  };
  useEffect(()=>{
    if(open&&attached)void read();
    return ()=>{generation.current++;};
  },[open,attached,source]);
  // Trace may have been read before PublicationOpened. Exact current native
  // Assistant ownership answers that earlier incomplete read without a reread;
  // a durable terminal request can never become active again.
  const active = attached && running && ['running', 'incomplete'].includes(notice.request.state) && notice.retry > 0;
  const label = active ? tx('agent:retry.active') : notice.request.state === 'cancelled' ? tx('agent:retry.cancelled')
    : notice.request.request?.failure_kind === 'timeout' ? tx('agent:retry.timeout') : tx('agent:retry.started');
  return <details className={css.retryRow} open={open} data-model-retry data-active={active || undefined} onToggle={event => {
    onToggle(event.currentTarget.open);
  }}>
    <summary className={css.retrySummary}><span className={css.retryText} role="status">
      <TextShimmer active={active}>{label + (notice.retry > 0 ? tx('agent:retry.ordinal', { n: notice.retry }) : '')}</TextShimmer>
    </span></summary>
    <div className={css.retryDetails}><div><span className={css.retryDetailLabel}>{tx('agent:retry.failure')}</span>{loaded?.message ?? tx('agent:retry.timeout')}</div>
      {large&&source.field&&open&&<TraceFields fields={[source.field]} cut={source.cut} reader={source.reader} owned={owned}/>}
      {loaded?.loading && <div>{tx('agent:retry.loading')}</div>}
      {loaded?.error && <button type="button" disabled={!attached} onClick={()=>void read()}>{tx('agent:retry.read-again')}</button>}
    </div>
  </details>;
}
