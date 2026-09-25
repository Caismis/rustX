/* Copyright (c) 2026 DeepSeek. MIT. Adapted TurnProcessNodeView; see PROVENANCE.md. */
import { IconChevronDownOutline14 } from '../primitives/icons';
import { useEffect, useState } from 'react';
import css from './TurnProcess.module.css';
export function TurnProcess({ id, open, toggle, tools, messages, running = false, start, end, outcome, durationMs }: { id: string; open: boolean; toggle?: () => void; tools: number; messages: number; running?: boolean; start?: string; end?: string; outcome?: string; durationMs?: number }) {
  const [now, setNow] = useState(Date.now);
  useEffect(() => {
    if (!running || !start) return;
    setNow(Date.now());
    const timer = setInterval(() => setNow(Date.now()), 1000);
    return () => clearInterval(timer);
  }, [running, start, id]);
  const seconds = durationMs !== undefined ? Math.max(1, Math.floor(durationMs / 1000)) : start ? Math.max(1, Math.floor(((end ? Date.parse(end) : now) - Date.parse(start)) / 1000)) : undefined;
  const duration = seconds === undefined ? undefined : seconds < 60 ? `${seconds}s` : `${Math.floor(seconds / 60)}m ${seconds % 60}s`;
  const label = running ? duration ? `Deep diving for ${duration}` : 'Deep diving…'
    : outcome === 'cancelled' ? 'Stopped' : outcome && outcome !== 'completed' ? 'Failed'
      : duration ? `Took ${duration}` : 'Worked';
  return <><span className={css.announcement} role="status" aria-live="polite" aria-atomic="true">{running ? 'Deep diving…' : outcome === 'cancelled' ? 'Stopped' : outcome && outcome !== 'completed' ? 'Failed' : 'Worked'}</span><button type="button" className={css.root} data-open={open || undefined} data-turn-process={id} data-turn-process-tool-calls={tools} data-turn-process-messages={messages} disabled={!toggle} aria-expanded={toggle ? open : undefined} onClick={event => { event.currentTarget.focus(); toggle?.(); }}>
    <span className={css.label}>{label}</span>{toggle && <IconChevronDownOutline14 className={css.chevron}/>}
  </button></>;
}
