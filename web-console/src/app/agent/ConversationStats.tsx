/* Copyright (c) 2026 DeepSeek. MIT. Adapted from StatsPills and ContextMeter; see PROVENANCE.md. */
import type { RuntimeClientSnapshot } from '../../../../protocol/app-server/v25';
import { useState } from 'react';
import { Modal } from '../../presentation/primitives/Modal';
import { Usage } from './TurnTail';
import css from './TurnTail.module.css';

export function ConversationStats({ snapshot }: { snapshot?: RuntimeClientSnapshot }) {
  const statistics = snapshot?.transcript.statistics;
  const [details, setDetails] = useState(false);
  if (!statistics) return null;
  const timing = statistics.timing;
  return <div className={css.stats} aria-label="Conversation statistics">
    {statistics && <><button className={css.stat} type="button" aria-haspopup="dialog" aria-expanded={details} onClick={() => setDetails(true)}>{statistics.turns} Turns · {statistics.steps} Steps{timing?.output_tokens_per_second != null && <> · {timing.output_tokens_per_second.toFixed(1)} tok/s</>}</button>
      {statistics.reported_usage && <Usage showCache usage={statistics.reported_usage} label={statistics.requests_with_usage === statistics.model_requests ? 'Conversation usage' : `Reported usage (${statistics.requests_with_usage} of ${statistics.model_requests} requests)`}/>}</>}
    <Modal open={details} title="Conversation statistics" closeLabel="Close statistics" onClose={() => setDetails(false)}><dl className={css.metrics}>
      <dt>Turns</dt><dd>{statistics.turns}</dd><dt>Steps</dt><dd>{statistics.steps}</dd>
      <dt>Model requests</dt><dd>{statistics.model_requests}</dd><dt>Usage reports</dt><dd>{statistics.requests_with_usage}</dd>
      {timing?.generation_ms != null && <><dt>Model generation work</dt><dd>{(timing.generation_ms / 1000).toFixed(2)} s</dd></>}
      {timing?.ttft_ms != null && <><dt>First request TTFT</dt><dd>{(timing.ttft_ms / 1000).toFixed(2)} s</dd></>}
    </dl></Modal>
  </div>;
}
