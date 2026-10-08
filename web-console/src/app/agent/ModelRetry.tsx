/* Copyright (c) 2026 DeepSeek. MIT. Adapted ModelRetryItem; see PROVENANCE.md. */
import { useEffect, useState } from 'react';
import type { AppServerClient } from '../../client/app-server';
import { useClientSelector } from '../../client/selectors';
import { modelRetryNotices, type ModelRetryNotice } from '../../bindings/model-retry';
import { useTranslation } from '../../locale/react';
import { TextShimmer } from '../../presentation/primitives/TextShimmer';
import css from './ModelRetry.module.css';

export function ModelRetries({ client, sessionId, attemptId }: { client: AppServerClient; sessionId: string; attemptId: string }) {
  const view = useClientSelector(client, state => state.views[sessionId]);
  if (!view?.trace) return null;
  const attached = view.attachment === 'attached';
  return modelRetryNotices(view.trace.page.records, attemptId).map(notice => <RetryNotice key={notice.key}
    notice={notice} attached={attached} client={client} sessionId={sessionId}
    running={view.snapshot?.attempt?.attempt_id === attemptId && view.snapshot.attempt.phase.type === 'running'
      && view.snapshot.attempt.in_flight?.message_id === notice.request.request?.assistant_message_id}/>);
}
function RetryNotice({ notice, attached, running, client, sessionId }: {
  notice: ModelRetryNotice; attached: boolean; running: boolean; client: AppServerClient; sessionId: string;
}) {
  const tx = useTranslation(), [open, setOpen] = useState(false);
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
    const expanded = event.currentTarget.open; setOpen(expanded);
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
