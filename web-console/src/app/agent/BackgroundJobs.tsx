/* Copyright (c) 2026 DeepSeek. MIT. JobListAction grouping and header surface adapted to native Jobs; see PROVENANCE.md. */
import { useEffect, useId, useLayoutEffect, useMemo, useRef, useState, type KeyboardEvent } from 'react';
import { createPortal } from 'react-dom';
import { autoUpdate, flip, offset, shift, size, useFloating } from '@floating-ui/react-dom';
import type { RuntimeClientJob, TraceRecord } from '../../../../protocol/app-server/v57';
import type { AppServerClient } from '../../client/app-server';
import { useClientSelector } from '../../client/selectors';
import { useTranslation } from '../../locale/react';
import { JobStateDot } from './JobStateDot';
import { IconChevronDownOutline14 } from '../../presentation/primitives/icons';
import { BackgroundJobItem } from './BackgroundJobItem';
import { MenuSurface } from '../../presentation/primitives/MenuSurface';
import { jobActive } from './job-state';
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
  const [open, setOpen] = useState(false), [endedOpen, setEndedOpen] = useState<boolean>();
  const [expandedKey, setExpandedKey] = useState<string>(), [armedKey, setArmedKey] = useState<string>();
  const [acceptedStop, setAcceptedStop] = useState<{ key: string; current: () => boolean }>();
  const [cleared, setCleared] = useState<ReadonlySet<string>>(new Set());
  const [now, setNow] = useState(Date.now);
  const keyboardEntry = useRef(false);
  const trigger = useRef<HTMLButtonElement>(null), panel = useRef<HTMLDivElement>(null);
  const timings = useMemo(() => new Map(records?.filter(record => record.kind === 'background').map(record => [record.native_id, record.timing])), [records]);
  const timing = (job: RuntimeClientJob) => timings.get(job.job_id);
  const ordered = [...jobs].sort((left, right) => {
    if (jobActive(left) !== jobActive(right)) return jobActive(left) ? -1 : 1;
    const a = timing(left), b = timing(right);
    return jobActive(left) ? a && b ? Date.parse(a.started_at) - Date.parse(b.started_at) : 0
      : a && b ? Date.parse(b.ended_at ?? b.started_at) - Date.parse(a.ended_at ?? a.started_at) : 0;
  });
  const live = ordered.filter(jobActive), ended = ordered.filter(job => !jobActive(job) && !cleared.has(job.job_id));
  const endedExpanded = endedOpen ?? live.length === 0;
  const visible = live.length + ended.length > 0;
  const ticking = open && live.some(job => timings.has(job.job_id));
  useEffect(() => { if (!ticking) return; setNow(Date.now()); const clock = setInterval(() => setNow(Date.now()), 1000); return () => clearInterval(clock); }, [ticking]);
  const { refs, floatingStyles, isPositioned } = useFloating({ open: open && visible, placement: 'bottom-start', strategy: 'fixed', whileElementsMounted: autoUpdate,
    middleware: [offset(5), flip({ padding: 16 }), shift({ padding: 16 }), size({ padding: 16, apply: ({ availableHeight, elements }) => { elements.floating.style.maxHeight = `${Math.max(0, Math.min(480, availableHeight))}px`; } })] });
  const close = (restore = false) => { setOpen(false); setArmedKey(undefined); if (restore) trigger.current?.focus({ preventScroll: true }); };
  const controls = () => [...(panel.current?.querySelectorAll<HTMLElement>(FOCUSABLE) ?? [])].filter(element => !element.closest('[hidden]'));
  useEffect(() => { if (!armedKey) return; const timer = setTimeout(() => setArmedKey(undefined), 3000); return () => clearTimeout(timer); }, [armedKey]);
  useEffect(() => { if (!jobs.some(job => job.job_id === armedKey && (job.state === 'running' || job.state === 'starting'))) setArmedKey(undefined); }, [jobs, armedKey]);
  useLayoutEffect(() => { if (open && isPositioned && keyboardEntry.current) { controls()[0]?.focus({ preventScroll: true }); keyboardEntry.current = false; } }, [open, isPositioned]);
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
    if (event.key === 'Tab' && (event.shiftKey ? index === 0 : index === rows.length - 1)) {
      if (event.shiftKey) event.preventDefault();
      trigger.current?.focus({ preventScroll: true });
    }
  };
  if (!visible) return null;
  const item = (job: RuntimeClientJob) => <BackgroundJobItem key={job.job_id} job={job} timing={timing(job)} now={now} expanded={expandedKey === job.job_id} onToggle={() => setExpandedKey(current => current === job.job_id ? undefined : job.job_id)} armed={armedKey === job.job_id} onArm={() => setArmedKey(current => current === job.job_id ? undefined : job.job_id)} stopping={acceptedStop?.key === job.job_id && acceptedStop.current()} onStopped={current => setAcceptedStop({ key: job.job_id, current })} client={client} sessionId={sessionId}/>;
  return <>
    <button ref={element => { trigger.current = element; refs.setReference(element); }} type="button" className={css.trigger} aria-label={tx('common:activity.background-jobs')} aria-haspopup="dialog" aria-controls={open ? id : undefined} aria-expanded={open} onClick={() => { setNow(Date.now()); setOpen(value => !value); setArmedKey(undefined); }} onKeyDown={event => { if (event.key === 'ArrowDown') { event.preventDefault(); keyboardEntry.current = true; if (open) controls()[0]?.focus({ preventScroll: true }); else setOpen(true); } else if (open && event.key === 'Tab' && !event.shiftKey) { event.preventDefault(); controls()[0]?.focus({ preventScroll: true }); } else if (open && event.key === 'Escape') { event.preventDefault(); close(true); } }}>
      {live.length > 0 && <JobStateDot state="ongoing"/>}<span className={css.count}>{tx(live.length ? 'common:activity.jobs-running' : 'common:activity.jobs-count', { count: live.length || ended.length })}</span><IconChevronDownOutline14 size={12} className={open ? css.triggerOpen : undefined}/>
    </button>
    {open && createPortal(<MenuSurface id={id} role="dialog" aria-label={tx('common:activity.background-jobs')} ref={element => { panel.current = element; refs.setFloating(element); }} className={css.menu} style={{ ...floatingStyles, visibility: isPositioned ? undefined : 'hidden' }} onKeyDown={navigate}>
      <ul className={css.menuBody}>
        {live.length > 0 && <li className={css.sectionHeader}>{tx('common:activity.jobs-active')}</li>}
        {live.map(item)}
        {ended.length > 0 && <li className={css.sectionHeader}>
          <button type="button" className={css.sectionToggle} aria-expanded={endedExpanded} onClick={() => setEndedOpen(!endedExpanded)}><IconChevronDownOutline14 size={12} className={endedExpanded ? `${css.sectionChevron} ${css.sectionChevronOpen}` : css.sectionChevron}/>{tx('common:activity.jobs-ended', { count: ended.length })}</button>
          <button type="button" className={css.sectionClear} aria-label={tx('common:activity.clear-ended-jobs')} onClick={() => { setCleared(previous => new Set([...previous, ...ended.map(job => job.job_id)])); if (ended.some(job => job.job_id === expandedKey)) setExpandedKey(undefined); if (!live.length) { close(); [...trigger.current?.closest('header')?.querySelectorAll<HTMLElement>('button:not(:disabled)') ?? []].find(element => element !== trigger.current)?.focus({ preventScroll: true }); } else trigger.current?.focus({ preventScroll: true }); }}>{tx('common:activity.clear-jobs')}</button>
        </li>}
        {endedExpanded && ended.map(item)}
      </ul>
    </MenuSurface>, document.body)}
  </>;
}
