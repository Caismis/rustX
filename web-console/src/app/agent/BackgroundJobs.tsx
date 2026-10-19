/* Copyright (c) 2026 DeepSeek. MIT. JobListAction grouping and header surface adapted to native Jobs; see PROVENANCE.md. */
import { useEffect, useId, useLayoutEffect, useRef, useState, type KeyboardEvent } from 'react';
import { createPortal } from 'react-dom';
import { autoUpdate, flip, offset, shift, size, useFloating } from '@floating-ui/react-dom';
import type { RuntimeClientJob, TraceRecord } from '../../../../protocol/app-server/v50';
import type { AppServerClient } from '../../client/app-server';
import { useClientSelector } from '../../client/selectors';
import { useTranslation } from '../../locale/react';
import { StateDot } from '../../presentation/primitives/StateDot';
import { IconChevronDownOutline14 } from '../../presentation/primitives/icons';
import { JobCard } from '../components/ActivityCards';
import { jobActive, jobDot } from './job-state';
import { jobStateLabel } from '../../bindings/status-labels';
import { formatDuration } from './token-format';
import css from './BackgroundJobs.module.css';

const NO_JOBS: RuntimeClientJob[] = [];
const FOCUSABLE = 'button:not(:disabled), summary, a[href], [tabindex="0"]';

/** Subscribe outside the transcript so reading and scrolling never hide Jobs. */
export function BackgroundJobs({ client, sessionId }: { client: AppServerClient; sessionId: string }) {
  const jobs = useClientSelector(client, state => state.views[sessionId]?.snapshot?.jobs) ?? NO_JOBS;
  const conversation = useClientSelector(client, state => state.views[sessionId]?.target?.conversation_id);
  const records = useClientSelector(client, state => state.views[sessionId]?.snapshot?.trace.records);
  return <BackgroundJobMenu key={`${sessionId}:${conversation}`} jobs={jobs} records={records} client={client} sessionId={sessionId}/>;
}

/** Clearing acknowledges only the ended identities visible at that gesture. */
export function BackgroundJobMenu({ jobs, records, client, sessionId }: { jobs: RuntimeClientJob[]; records?: readonly TraceRecord[]; client?: AppServerClient; sessionId?: string }) {
  const tx = useTranslation(), id = useId();
  const [open, setOpen] = useState(false), [endedOpen, setEndedOpen] = useState(false);
  const [cleared, setCleared] = useState<ReadonlySet<string>>(new Set());
  const [now, setNow] = useState(Date.now);
  const trigger = useRef<HTMLButtonElement>(null), panel = useRef<HTMLDivElement>(null);
  const live = jobs.filter(jobActive), ended = jobs.filter(job => !jobActive(job) && !cleared.has(job.job_id));
  const visible = live.length + ended.length > 0;
  const ticking = open && live.some(job => records?.some(record => record.kind === 'background' && record.native_id === job.job_id));
  useEffect(() => { if (!ticking) return; setNow(Date.now()); const clock = setInterval(() => setNow(Date.now()), 1000); return () => clearInterval(clock); }, [ticking]);
  const { refs, floatingStyles, isPositioned } = useFloating({ open: open && visible, placement: 'bottom-start', strategy: 'fixed', whileElementsMounted: autoUpdate,
    middleware: [offset(5), flip({ padding: 16 }), shift({ padding: 16 }), size({ padding: 16, apply: ({ availableHeight, elements }) => { elements.floating.style.maxHeight = `${Math.max(0, Math.min(480, availableHeight))}px`; } })] });
  const close = (restore = false) => { setOpen(false); if (restore) trigger.current?.focus({ preventScroll: true }); };
  const controls = () => [...(panel.current?.querySelectorAll<HTMLElement>(FOCUSABLE) ?? [])].filter(element => !element.closest('[hidden]'));
  useLayoutEffect(() => { if (open && isPositioned) controls()[0]?.focus({ preventScroll: true }); }, [open, isPositioned]);
  useEffect(() => {
    if (!open) return;
    const outside = (event: PointerEvent) => { if (event.target instanceof Node && !trigger.current?.contains(event.target) && !panel.current?.contains(event.target)) close(); };
    document.addEventListener('pointerdown', outside);
    return () => document.removeEventListener('pointerdown', outside);
  }, [open]);
  useEffect(() => { if (!visible) setOpen(false); }, [visible]);
  const navigate = (event: KeyboardEvent) => {
    if (event.nativeEvent.isComposing || event.altKey || event.ctrlKey || event.metaKey) return;
    if (event.key === 'Escape') { event.preventDefault(); event.stopPropagation(); close(true); return; }
    const rows = controls(), index = rows.indexOf(document.activeElement as HTMLElement);
    if (event.key === 'Tab') {
      if (event.shiftKey && index === 0 || !event.shiftKey && index === rows.length - 1) { event.preventDefault(); close(true); }
    } else if (['ArrowDown', 'ArrowUp', 'Home', 'End'].includes(event.key)) {
      event.preventDefault();
      const next = event.key === 'Home' ? 0 : event.key === 'End' ? rows.length - 1 : event.key === 'ArrowDown' ? (index + 1) % rows.length : (index + rows.length - 1) % rows.length;
      rows[next]?.focus({ preventScroll: true });
    }
  };
  if (!visible) return null;
  const item = (job: RuntimeClientJob) => {
    const start = records?.find(record => record.kind === 'background' && record.native_id === job.job_id)?.timing.started_at;
    const elapsed = jobActive(job) && start ? Math.max(0, now - Date.parse(start)) : undefined;
    return <div key={job.job_id} className={css.item} data-live={jobActive(job)} hidden={!jobActive(job) && !endedOpen}>
    <small className={css.status}><StateDot state={jobDot(job.state)} size={7}/>{jobStateLabel(tx, job.state)}</small>
    <JobCard job={job} client={client} sessionId={sessionId}/>
    {elapsed !== undefined && Number.isFinite(elapsed) && <small className={css.duration}>{tx('common:activity.job-duration', { duration: formatDuration(Math.floor(elapsed / 1000) * 1000, tx) })}</small>}
    {!jobActive(job) && job.result && <small className={css.duration}>{tx('common:activity.job-duration', { duration: formatDuration(Number(job.result.duration_ms), tx) })}</small>}
  </div>;
  };
  return <>
    <button ref={element => { trigger.current = element; refs.setReference(element); }} type="button" className={css.trigger} aria-label={tx('common:activity.background-jobs')} aria-haspopup="dialog" aria-controls={open ? id : undefined} aria-expanded={open} onClick={() => setOpen(value => !value)} onKeyDown={event => { if (event.key === 'ArrowDown') { event.preventDefault(); setOpen(true); } }}>
      <StateDot state={live.length ? 'ongoing' : 'idle'} size={8}/><span>{tx(live.length ? 'common:activity.jobs-running' : 'common:activity.jobs-ended', { count: live.length || ended.length })}</span><IconChevronDownOutline14 className={open ? css.triggerOpen : undefined}/>
    </button>
    {open && createPortal(<div id={id} role="dialog" aria-label={tx('common:activity.background-jobs')} ref={element => { panel.current = element; refs.setFloating(element); }} className={css.menu} style={floatingStyles} onKeyDown={navigate}>
      <div className={css.body}>
        {live.length > 0 && <div className={css.section}>{tx('common:activity.jobs-active', { count: live.length })}</div>}
        {live.map(item)}
        {ended.length > 0 && <div className={css.section}>
          <button type="button" className={css.action} aria-expanded={endedOpen} aria-controls={`${id}-ended`} onClick={() => setEndedOpen(value => !value)}><IconChevronDownOutline14 className={endedOpen ? undefined : css.sectionClosed}/>{tx('common:activity.jobs-ended', { count: ended.length })}</button>
          <button type="button" className={css.action} aria-label={tx('common:activity.clear-ended-jobs')} onClick={() => { setCleared(previous => new Set([...previous, ...ended.map(job => job.job_id)])); setEndedOpen(false); if (!live.length) { close(); [...trigger.current?.closest('header')?.querySelectorAll<HTMLElement>('button:not(:disabled)') ?? []].find(element => element !== trigger.current)?.focus({ preventScroll: true }); } else trigger.current?.focus({ preventScroll: true }); }}>{tx('common:activity.clear-jobs')}</button>
        </div>}
        <div id={`${id}-ended`} hidden={!endedOpen}>{ended.map(item)}</div>
      </div>
    </div>, document.body)}
  </>;
}
