import { useTranslation } from '../../locale/react';
/* Copyright (c) 2026 DeepSeek. MIT. Adapted TurnProcessNodeView; see PROVENANCE.md. */
import { IconChevronDownOutline14 } from '../primitives/icons';
import { useEffect, useState } from 'react';
import css from './TurnProcess.module.css';
export function TurnProcess({ id, open, toggle, tools, messages, running = false, start, end, outcome, durationMs }: { id: string; open: boolean; toggle?: () => void; tools: number; messages: number; running?: boolean; start?: string; end?: string; outcome?: string; durationMs?: number }) {
  const tx = useTranslation();
  const [now, setNow] = useState(Date.now);
  useEffect(() => {
    if (!running || !start) return;
    setNow(Date.now());
    const timer = setInterval(() => setNow(Date.now()), 1000);
    return () => clearInterval(timer);
  }, [running, start, id]);
  const seconds = durationMs !== undefined ? Math.max(1, Math.floor(durationMs / 1000)) : start ? Math.max(1, Math.floor(((end ? Date.parse(end) : now) - Date.parse(start)) / 1000)) : undefined;
  const duration = seconds === undefined ? undefined : seconds < 60 ? tx('agent:duration.seconds', { n: seconds }) : tx('agent:duration.minutes-seconds', { m: Math.floor(seconds / 60), s: seconds % 60 });
  const label = running ? duration ? tx('agent:copy.deep-diving-for-value', { p0: duration }) : tx('agent:turn-process.deep-diving')
    : outcome === 'cancelled' ? tx('agent:turn-process.stopped') : outcome && outcome !== 'completed' ? tx('agent:turn-process.failed')
      : duration ? tx('agent:copy.took-value', { p0: duration }) : tx('agent:turn-process.worked');
  return <><span className={css.announcement} role="status" aria-live="polite" aria-atomic="true">{running ? tx('agent:turn-process.deep-diving') : outcome === 'cancelled' ? tx('agent:turn-process.stopped') : outcome && outcome !== 'completed' ? tx('agent:turn-process.failed') : tx('agent:turn-process.worked')}</span><button type="button" className={css.root} data-open={open || undefined} data-turn-process={id} data-turn-process-tool-calls={tools} data-turn-process-messages={messages} disabled={!toggle} aria-expanded={toggle ? open : undefined} onClick={event => { event.currentTarget.focus(); toggle?.(); }}>
    <span className={css.label}>{label}</span>{toggle && <IconChevronDownOutline14 className={css.chevron}/>}
  </button></>;
}
