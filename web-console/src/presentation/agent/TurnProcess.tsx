import { useTranslation } from '../../locale/react';
/* Copyright (c) 2026 DeepSeek. MIT. Adapted TurnProcessNodeView; see PROVENANCE.md. */
import { IconChevronDownOutline14 } from '../primitives/icons';
import css from './TurnProcess.module.css';
export function TurnProcess({ id, open, toggle, tools, messages, start, end, outcome, durationMs }: { id: string; open: boolean; toggle?: () => void; tools: number; messages: number; start?: string; end?: string; outcome?: string; durationMs?: number }) {
  const tx = useTranslation();
  // Running work has one status at the tail. The duration header appears only
  // after native settlement, without a second clock above streaming content.
  if (outcome === 'running') return null;
  const seconds = outcome === 'incomplete_at_cut' ? undefined : durationMs !== undefined ? Math.max(1, Math.floor(durationMs / 1000)) : start && end ? Math.max(1, Math.floor((Date.parse(end) - Date.parse(start)) / 1000)) : undefined;
  const duration = seconds === undefined ? undefined : seconds < 60 ? tx('agent:duration.seconds', { n: seconds }) : tx('agent:duration.minutes-seconds', { m: Math.floor(seconds / 60), s: seconds % 60 });
  const label = outcome === 'incomplete_at_cut' ? tx('agent:turn-process.incomplete') : outcome === 'cancelled' ? tx('agent:turn-process.stopped') : outcome && outcome !== 'completed' ? tx('agent:turn-process.failed')
      : duration ? tx('agent:copy.took-value', { p0: duration }) : tx('agent:turn-process.worked');
  return <><span className={css.announcement} role="status" aria-live="polite" aria-atomic="true">{outcome === 'incomplete_at_cut' ? tx('agent:turn-process.incomplete') : outcome === 'cancelled' ? tx('agent:turn-process.stopped') : outcome && outcome !== 'completed' ? tx('agent:turn-process.failed') : tx('agent:turn-process.worked')}</span><button type="button" className={css.root} data-open={open || undefined} data-turn-process={id} data-turn-process-tool-calls={tools} data-turn-process-messages={messages} disabled={!toggle} aria-expanded={toggle ? open : undefined} onClick={event => { event.currentTarget.focus(); toggle?.(); }}>
    <span className={css.label}>{label.split(/(\d+)/).map((part, index) => /^\d+$/.test(part) ? <span key={index} className={css.durationNumber}>{part}</span> : part)}</span>{toggle && <IconChevronDownOutline14 className={css.chevron}/>}
  </button></>;
}
