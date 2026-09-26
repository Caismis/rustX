import { useTranslation } from '../../locale/react';
/* Copyright (c) 2026 DeepSeek. MIT. Adapted from StatsPills and ContextMeter; see PROVENANCE.md. */
import type { RuntimeClientSnapshot } from '../../../../protocol/app-server/v23';
import { useState } from 'react';
import { Modal } from '../../presentation/primitives/Modal';
import { Usage } from './TurnTail';
import css from './TurnTail.module.css';

export function ConversationStats({ snapshot }: { snapshot?: RuntimeClientSnapshot }) {
  const tx = useTranslation();
  const statistics = snapshot?.transcript.statistics;
  const [details, setDetails] = useState(false);
  if (!statistics) return null;
  const timing = statistics.timing;
  return <div className={css.stats} aria-label={tx('agent:conversation-stats.conversation-statistics')}>
    {statistics && <><button className={css.stat} type="button" aria-haspopup="dialog" aria-expanded={details} onClick={() => setDetails(true)}>{statistics.turns} {tx('agent:conversation-stats.turns')}{' '}{statistics.steps} {tx('agent:conversation-stats.steps')}{timing?.output_tokens_per_second != null && <> · {timing.output_tokens_per_second.toFixed(1)} {tx('agent:conversation-stats.tok-s')}</>}</button>
      {statistics.reported_usage && <Usage showCache usage={statistics.reported_usage} label={statistics.requests_with_usage === statistics.model_requests ? tx('agent:conversation-stats.conversation-usage') : tx('agent:conversation-stats.reported-usage-value-of-value-requests', { p0: statistics.requests_with_usage, p1: statistics.model_requests })}/>}</>}
    <Modal open={details} title={tx('agent:conversation-stats.conversation-statistics')} closeLabel={tx('agent:conversation-stats.close-statistics')} onClose={() => setDetails(false)}><dl className={css.metrics}>
      <dt>{tx('agent:conversation-stats.turns-2')}</dt><dd>{statistics.turns}</dd><dt>{tx('agent:conversation-stats.steps')}</dt><dd>{statistics.steps}</dd>
      <dt>{tx('agent:conversation-stats.model-requests')}</dt><dd>{statistics.model_requests}</dd><dt>{tx('agent:conversation-stats.usage-reports')}</dt><dd>{statistics.requests_with_usage}</dd>
      {timing?.generation_ms != null && <><dt>{tx('agent:conversation-stats.model-generation-work')}</dt><dd>{(timing.generation_ms / 1000).toFixed(2)} {tx('agent:conversation-stats.s')}</dd></>}
      {timing?.ttft_ms != null && <><dt>{tx('agent:conversation-stats.first-request-ttft')}</dt><dd>{(timing.ttft_ms / 1000).toFixed(2)} {tx('agent:conversation-stats.s')}</dd></>}
    </dl></Modal>
  </div>;
}
