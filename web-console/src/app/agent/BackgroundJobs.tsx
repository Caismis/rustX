/* Copyright (c) 2026 DeepSeek. MIT. Adapted from ui-jobs JobsSection.tsx; see PROVENANCE.md. */
import { useEffect, useState } from 'react';
import type { RuntimeClientJob, TraceRecord } from '../../../../protocol/app-server/v80';
import type { AppServerClient } from '../../client/app-server';
import { useClientSelector } from '../../client/selectors';
import { useTranslation } from '../../locale/react';
import { IconChevronDownOutline14, IconChevronUpOutline14 } from '../../presentation/primitives/icons';
import { HoverCardGroup } from '../../presentation/primitives/HoverCard';
import { BackgroundJobItem } from './BackgroundJobItem';
import { useJobsSessionStore, type JobGroup } from './jobs-store';
import { jobActive } from './job-state';
import { toggleElementId } from './session-panel-state';
import { backgroundJobsCss } from './backgroundJobsCss';

const NO_JOBS: RuntimeClientJob[] = [];
const FOLD_LIMIT = 5;
const KILL_ARM_MS = 3000;

/** Blue indicator dot on the Session panel toggle when jobs run while closed. */
export function JobsRunningIndicator({ client, sessionId, panelShown }: {
  client: AppServerClient;
  sessionId: string;
  panelShown: boolean;
}) {
  const tx = useTranslation();
  const running = useClientSelector(
    client,
    state => (state.views[sessionId]?.snapshot?.jobs ?? NO_JOBS).filter(jobActive).length,
  );
  if (panelShown || running === 0) return null;
  return (
    <span className={backgroundJobsCss.runningIndicator} data-jobs-running="">
      <span className={backgroundJobsCss.visuallyHidden}>
        {tx('common:activity.jobs-running', { count: running })}
      </span>
    </span>
  );
}

/** Background jobs section mounted inside SessionPanel while the panel shows. */
export function BackgroundJobs({ client, sessionId }: { client: AppServerClient; sessionId: string }) {
  const jobs = useClientSelector(client, state => state.views[sessionId]?.snapshot?.jobs) ?? NO_JOBS;
  const conversation = useClientSelector(client, state => state.views[sessionId]?.target?.conversation_id);
  const generation = useClientSelector(client, state => state.generation);
  const records = useClientSelector(client, state => state.views[sessionId]?.snapshot?.trace.records);
  const storeKey = `${generation}:${sessionId}:${conversation ?? ''}`;
  return <BackgroundJobsSection key={storeKey} jobs={jobs} records={records} client={client} sessionId={sessionId} storeKey={storeKey}/>;
}

/** Ordered running + finished background jobs list with 5-row folds and session-persisted clear. */
export function BackgroundJobsSection({ jobs, records, client, sessionId, storeKey }: {
  jobs: readonly RuntimeClientJob[];
  records?: readonly TraceRecord[];
  client?: AppServerClient;
  sessionId?: string;
  storeKey?: string;
}) {
  const tx = useTranslation();
  const [localEndedOpen, setLocalEndedOpen] = useState<boolean>();
  const [localUnfolded, setLocalUnfolded] = useState<Record<JobGroup, boolean>>({ live: false, settled: false });
  const [localCleared, setLocalCleared] = useState<ReadonlySet<string>>(new Set());
  const [localAcceptedStop, setLocalAcceptedStop] = useState<{ key: string; current: () => boolean }>();
  const store = useJobsSessionStore(client, storeKey);
  const persisted = !!(client && storeKey);
  const endedOpen = persisted ? store.state.settledOpen : localEndedOpen;
  const unfolded = persisted ? store.state.unfolded : localUnfolded;
  const cleared = persisted ? store.state.cleared : localCleared;
  const acceptedStop = persisted ? store.state.acceptedStop : localAcceptedStop;

  const setEndedOpen = (value: boolean) => { if (persisted) store.setSettledOpen(value); else setLocalEndedOpen(value); };
  const setUnfolded = (group: JobGroup, value: boolean) => { if (persisted) store.setUnfolded(group, value); else setLocalUnfolded(prev => ({ ...prev, [group]: value })); };
  const markCleared = (ids: readonly string[]) => {
    if (persisted) store.clearSettled(ids);
    else { setLocalCleared(prev => new Set([...prev, ...ids])); setLocalUnfolded(prev => ({ ...prev, settled: false })); }
  };
  const setAcceptedStop = (value: { key: string; current: () => boolean } | undefined) => {
    if (persisted) store.setAcceptedStop(value);
    else setLocalAcceptedStop(value);
  };

  const [expandedKey, setExpandedKey] = useState<string>();
  const [armedKey, setArmedKey] = useState<string>();
  const [now, setNow] = useState(Date.now);

  const timings = new Map(records?.filter(record => record.kind === 'background').map(record => [record.native_id, record.timing]));
  const timing = (job: RuntimeClientJob) => timings.get(job.job_id);
  const ordered = [...jobs].sort((left, right) => {
    if (jobActive(left) !== jobActive(right)) return jobActive(left) ? -1 : 1;
    const a = timing(left), b = timing(right);
    if (!a || !b) return 0;
    if (jobActive(left)) return Date.parse(b.started_at) - Date.parse(a.started_at);
    const finished = Date.parse(b.ended_at ?? b.started_at) - Date.parse(a.ended_at ?? a.started_at);
    return finished !== 0 ? finished : Date.parse(a.started_at) - Date.parse(b.started_at);
  });

  const live = ordered.filter(jobActive);
  const ended = ordered.filter(job => !jobActive(job) && !cleared.has(job.job_id));
  const endedExpanded = endedOpen ?? live.length === 0;
  const liveFoldable = live.length > FOLD_LIMIT + 1;
  const shownLive = liveFoldable && !unfolded.live ? live.slice(0, FOLD_LIMIT) : live;
  const endedFoldable = ended.length > FOLD_LIMIT + 1;
  const openEnded = endedExpanded ? ended : NO_JOBS;
  const shownEnded = endedFoldable && !unfolded.settled ? openEnded.slice(0, FOLD_LIMIT) : openEnded;
  const visible = live.length + ended.length > 0;
  const ticking = live.some(job => timings.has(job.job_id));

  useEffect(() => {
    if (!ticking) return;
    setNow(Date.now());
    const clock = setInterval(() => setNow(Date.now()), 1000);
    return () => clearInterval(clock);
  }, [ticking]);

  useEffect(() => {
    if (!armedKey) return;
    const timer = setTimeout(() => setArmedKey(undefined), KILL_ARM_MS);
    return () => clearTimeout(timer);
  }, [armedKey]);

  useEffect(() => {
    if (!jobs.some(job => job.job_id === armedKey && (job.state === 'running' || job.state === 'starting'))) {
      setArmedKey(undefined);
    }
  }, [jobs, armedKey]);

  useEffect(() => {
    if (expandedKey && !ordered.some(job => job.job_id === expandedKey)) setExpandedKey(undefined);
  }, [ordered, expandedKey]);

  if (!visible) return null;

  const item = (job: RuntimeClientJob) => (
    <BackgroundJobItem
      key={job.job_id}
      job={job}
      timing={timing(job)}
      now={now}
      expanded={expandedKey === job.job_id}
      onToggle={() => setExpandedKey(current => current === job.job_id ? undefined : job.job_id)}
      armed={armedKey === job.job_id}
      onArm={() => setArmedKey(current => current === job.job_id ? undefined : job.job_id)}
      stopping={acceptedStop?.key === job.job_id && acceptedStop.current()}
      onStopped={current => setAcceptedStop({ key: job.job_id, current })}
      client={client}
      sessionId={sessionId}
    />
  );

  const fold = (group: JobGroup, hidden: number) => {
    const expanded = unfolded[group];
    const label = expanded
      ? tx('common:activity.jobs-fold-less')
      : tx(group === 'live' ? 'common:activity.jobs-live-more' : 'common:activity.jobs-ended-more', { count: hidden });
    const name = tx(group === 'live' ? 'common:activity.jobs-live-fold-name' : 'common:activity.jobs-ended-fold-name', { label });
    return (
      <li key={`fold-${group}`} className={backgroundJobsCss.item}>
        <div data-session-panel-row="fold">
          <button
            type="button"
            data-fold=""
            data-row-part="body"
            aria-expanded={expanded}
            aria-label={name}
            onClick={() => setUnfolded(group, !expanded)}
          >
            <span data-row-part="icon" aria-hidden="true">
              {expanded ? <IconChevronUpOutline14 size={14}/> : <IconChevronDownOutline14 size={14}/>}
            </span>
            <span data-row-part="content">{label}</span>
          </button>
        </div>
      </li>
    );
  };

  return (
    <HoverCardGroup>
      <ul className={backgroundJobsCss.list} data-jobs-section="" aria-label={tx('common:activity.background-jobs')}>
        {shownLive.map(item)}
        {liveFoldable && fold('live', live.length - FOLD_LIMIT)}
        {ended.length > 0 && (
          <li className={backgroundJobsCss.sectionHeader}>
            <button
              type="button"
              className={backgroundJobsCss.sectionToggle}
              aria-expanded={endedExpanded}
              onClick={() => setEndedOpen(!endedExpanded)}
            >
              {tx('common:activity.jobs-ended', { count: ended.length })}
              <IconChevronDownOutline14
                size={14}
                className={endedExpanded ? `${backgroundJobsCss.sectionChevron} ${backgroundJobsCss.sectionChevronOpen}` : backgroundJobsCss.sectionChevron}
              />
            </button>
            <button
              type="button"
              className={backgroundJobsCss.sectionClear}
              aria-label={tx('common:activity.clear-ended-jobs')}
              onClick={() => {
                markCleared(ended.map(job => job.job_id));
                if (ended.some(job => job.job_id === expandedKey)) setExpandedKey(undefined);
                if (!live.length && sessionId) {
                  document.getElementById(toggleElementId(sessionId))?.focus({ preventScroll: true });
                }
              }}
            >
              {tx('common:activity.clear-jobs')}
            </button>
          </li>
        )}
        {shownEnded.map(item)}
        {endedExpanded && endedFoldable && fold('settled', ended.length - FOLD_LIMIT)}
      </ul>
    </HoverCardGroup>
  );
}
