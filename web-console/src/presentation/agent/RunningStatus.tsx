/* Copyright (c) 2026 DeepSeek. MIT. Adapted from Harness; see PROVENANCE.md. */
import { memo, useEffect, useState } from 'react';
import { useTranslation } from '../../locale/react';
import { TextShimmer } from '../primitives/TextShimmer';
import { RunningWhaleTail } from './RunningWhaleTail';
import css from './Chat.module.css';

/** Isolate clock ticks from the transcript; announce activity, not every second. */
export const RunningStatus = memo(function RunningStatus({ startTime }: { startTime?: number }) {
  const tx = useTranslation();
  const [now, setNow] = useState(Date.now);
  useEffect(() => {
    if (startTime === undefined) return;
    setNow(Date.now());
    const timer = setInterval(() => setNow(Date.now()), 1000);
    return () => clearInterval(timer);
  }, [startTime]);
  const seconds = startTime === undefined ? undefined : Math.max(1, Math.floor((now - startTime) / 1000));
  const duration = seconds === undefined ? undefined : seconds < 60
    ? tx('agent:duration.seconds', { n: seconds })
    : tx('agent:duration.minutes-seconds', { m: Math.floor(seconds / 60), s: seconds % 60 });
  const label = duration === undefined ? tx('agent:turn-process.deep-diving') : tx('agent:copy.deep-diving-for-value', { p0: duration });
  return <div className={css.running} data-chat-running>
    <span className={css.runningAnnouncement} role="status" aria-live="polite" aria-atomic="true">{tx('agent:turn-process.deep-diving')}</span>
    <span className={css.runningDivider} aria-hidden="true" />
    <span className={css.runningContent}><RunningWhaleTail /><TextShimmer active className={css.runningText}>{label}</TextShimmer></span>
  </div>;
});
