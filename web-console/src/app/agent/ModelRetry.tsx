/* Copyright (c) 2026 DeepSeek. MIT. Adapted ModelRetryItem; see PROVENANCE.md. */
import { useEffect, useMemo, useState, type ReactNode } from 'react';
import type { RuntimeClientSnapshot, RuntimeClientTranscriptEntry } from '../../../../protocol/app-server/v42';
import type { AppServerClient } from '../../client/app-server';
import { useClientSelector } from '../../client/selectors';
import { modelRetryPlacements, type ModelRetryNotice } from '../../bindings/model-retry';
import type { RequestFeedback } from './AgentTranscript';
import { useTranslation } from '../../locale/react';
import { TextShimmer } from '../../presentation/primitives/TextShimmer';
import css from './ModelRetry.module.css';

interface RetrySelection {
  placements: ReturnType<typeof modelRetryPlacements>;
  attached: boolean;
  attempt: RuntimeClientSnapshot['attempt'] | undefined;
}
function sameRetries(a: RetrySelection, b: RetrySelection) {
  return a.attached === b.attached && a.attempt === b.attempt && a.placements.length === b.placements.length
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
      attached: value?.attachment === 'attached', attempt: value?.snapshot?.attempt };
  }, sameRetries);
  return useMemo(() => {
    const messages = new Map<string, ReactNode[]>(), attempts = new Map<string, ReactNode[]>();
    for (const { notice, beforeMessageId } of view.placements) {
      const map = beforeMessageId ? messages : attempts;
      const key = beforeMessageId ?? notice.attemptId;
      const list = map.get(key) ?? [];
      list.push(<RetryNotice key={notice.key} notice={notice} open={expanded.has(notice.key)} onToggle={open => setExpanded(previous => {
        if (previous.has(notice.key) === open) return previous;
        const next = new Set(previous); if (open) next.add(notice.key); else next.delete(notice.key); return next;
      })} attached={view.attached} client={client} sessionId={sessionId}
        running={view.attempt?.attempt_id === notice.attemptId && view.attempt.phase.type === 'running'
          && view.attempt.in_flight?.message_id === notice.request.request?.assistant_message_id}/>);
      map.set(key, list);
    }
    return { messages, attempts };
  }, [client, sessionId, view.placements, view.attached, view.attempt, expanded]);
}
function RetryNotice({ notice, open, onToggle, attached, running, client, sessionId }: {
  notice: ModelRetryNotice; open: boolean; onToggle: (open: boolean) => void; attached: boolean; running: boolean; client: AppServerClient; sessionId: string;
}) {
  const tx = useTranslation();
  const detail = useClientSelector(client, state => notice.failure ? state.views[sessionId]?.trace?.details[notice.failure.id] : undefined);
  // Trace may have been read before PublicationOpened. Exact current native
  // Assistant ownership answers that earlier incomplete read without a reread;
  // a durable terminal request can never become active again.
  const active = attached && running && ['running', 'incomplete'].includes(notice.request.state) && notice.retry > 0;
  const label = active ? tx('agent:retry.active') : notice.request.state === 'cancelled' ? tx('agent:retry.cancelled')
    : notice.request.request?.failure_kind === 'timeout' ? tx('agent:retry.timeout') : tx('agent:retry.started');
  const read = () => { if (attached && notice.failure?.has_detail) void client.loadTraceDetail(sessionId, notice.failure.id); };
  useEffect(() => { if (open && attached && notice.failure?.has_detail) void client.loadTraceDetail(sessionId, notice.failure.id); }, [open, attached, client, sessionId, notice.failure?.id, notice.failure?.has_detail]);
  const message = detail?.detail?.request?.failure?.message;
  return <details className={css.retryRow} open={open} data-model-retry data-active={active || undefined} onToggle={event => {
    onToggle(event.currentTarget.open);
  }}>
    <summary className={css.retrySummary}><span className={css.retryText} role="status">
      <TextShimmer active={active}>{label + (notice.retry > 0 ? tx('agent:retry.ordinal', { n: notice.retry }) : '')}</TextShimmer>
    </span></summary>
    <div className={css.retryDetails}><div><span className={css.retryDetailLabel}>{tx('agent:retry.failure')}</span>{message?.text ?? tx('agent:retry.timeout')}</div>
      {message?.truncated && <div>{tx('agent:retry.truncated')}</div>}
      {detail?.loading && <div>{tx('agent:retry.loading')}</div>}
      {detail?.error && <button type="button" disabled={!attached} onClick={read}>{tx('agent:retry.read-again')}</button>}
    </div>
  </details>;
}
