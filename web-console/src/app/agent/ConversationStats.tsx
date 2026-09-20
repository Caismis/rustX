/* Copyright (c) 2026 DeepSeek. MIT. Adapted from StatsPills and ContextMeter; see PROVENANCE.md. */
import type { RuntimeClientSnapshot } from '../../../../protocol/app-server/v13';
import { Tooltip } from '../../presentation/primitives/Tooltip';
import { Usage } from './ResponseTail';
import css from './ResponseTail.module.css';

export function ConversationStats({ snapshot }: { snapshot?: RuntimeClientSnapshot }) {
  const statistics = snapshot?.transcript.statistics;
  const context = snapshot?.context?.last_request_occupancy;
  if (!statistics && !context) return null;
  const percent = context ? Math.round(100 * context.input_tokens / context.context_window_tokens) : undefined;
  return <div className={css.stats} aria-label="Conversation statistics">
    {statistics && <><span>{statistics.completed_responses} {statistics.completed_responses === '1' ? 'response' : 'responses'} · {statistics.model_requests} {statistics.model_requests === '1' ? 'request' : 'requests'}</span>
      {statistics.requests_with_usage !== statistics.model_requests && <span>{statistics.requests_with_usage}/{statistics.model_requests} usage reports</span>}
      {statistics.reported_usage && <Usage showCache usage={statistics.reported_usage} label={statistics.requests_with_usage === statistics.model_requests ? 'Conversation usage' : `Reported usage (${statistics.requests_with_usage} of ${statistics.model_requests} requests)`}/>}</>}
    {context && <Tooltip label={`${context.input_tokens.toLocaleString()} / ${context.context_window_tokens.toLocaleString()} tokens · ${context.model} · Last provider-measured request; excludes unsent input`}><span tabIndex={0} className={css.stat} aria-label={`Last request context ${percent}%`}><svg width="14" height="14" viewBox="0 0 14 14" aria-hidden><circle cx="7" cy="7" r="5.5" fill="none" stroke="currentColor" opacity=".2" strokeWidth="2"/><circle cx="7" cy="7" r="5.5" fill="none" stroke="currentColor" strokeWidth="2" strokeDasharray={`${Math.min(100, percent!) * .3456} 34.56`} transform="rotate(-90 7 7)"/></svg>Last request context {percent}%</span></Tooltip>}
  </div>;
}
