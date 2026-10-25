/* Copyright (c) 2026 DeepSeek. MIT. JobListAction task rows adapted to native Jobs; see PROVENANCE.md. */
import { useEffect, useRef, useState } from 'react';
import type { RuntimeClientJob, TraceTiming } from '../../../../protocol/app-server/v57';
import { domainOriginalValue } from '../../../../protocol/app-server/domain-observation';
import type { AppServerClient } from '../../client/app-server';
import { jobResultOutput } from '../../bindings/jobs';
import { jobStateLabel } from '../../bindings/status-labels';
import { useTranslation } from '../../locale/react';
import { displayText, type Translate } from '../../locale/translation';
import { JobStateDot } from './JobStateDot';
import { TerminalBlock } from '../../presentation/primitives/TerminalBlock';
import { IconChevronDownOutline14, IconStopFill16 } from '../../presentation/primitives/icons';
import { ToolArtifacts } from '../components/Artifact';
import { useActivityRequest } from '../components/useActivityRequest';
import { jobActive, jobDot } from './job-state';
import css from './BackgroundJobs.module.css';

/** Harness uses whole seconds and at most two adjacent units in task rows. */
function duration(ms: number, tx: Translate): string {
  const total = Math.max(0, Math.floor(ms / 1000)), seconds = total % 60;
  if (total >= 3600) return tx('common:activity.job-hours', { hours: Math.floor(total / 3600), minutes: Math.floor(total / 60) % 60 });
  if (total >= 60) return tx('common:activity.job-minutes', { minutes: Math.floor(total / 60), seconds });
  return tx('common:activity.job-seconds', { seconds });
}

/** One task row and its selected output; observation never changes native lifecycle. */
export function BackgroundJobItem({ job, timing, now, expanded, onToggle, armed, onArm, stopping, onStopped, client, sessionId }: {
  job: RuntimeClientJob; timing?: TraceTiming; now: number; expanded: boolean; onToggle: () => void;
  armed: boolean; onArm: () => void; stopping: boolean; onStopped: (current: () => boolean) => void; client?: AppServerClient; sessionId?: string;
}) {
  const tx = useTranslation(), source = useRef(job); source.current = job;
  const stop = useActivityRequest({ client, sessionId });
  const [readError, setReadError] = useState<{ source: RuntimeClientJob; message: string }>();
  const [loaded, setLoaded] = useState<{ source: RuntimeClientJob; value: RuntimeClientJob; current: () => boolean }>();
  const [retry, setRetry] = useState(0), [showFailure, setShowFailure] = useState(false);
  useEffect(() => {
    setShowFailure(!!stop.error);
    if (!stop.error) return;
    const timer = setTimeout(() => setShowFailure(false), 4000);
    return () => clearTimeout(timer);
  }, [stop.error]);
  const value = loaded?.source === job && loaded.current() ? loaded.value : job;
  const deferred = !!value.deferred_fields?.length;
  useEffect(() => {
    if (!expanded || !deferred || !client || !sessionId) return;
    const proof = client.getSnapshot().views[sessionId]?.attachmentObservation;
    if (!client.isAttachmentObservationCurrent(sessionId, proof)) return;
    const target = client.target(sessionId);
    let observing = true;
    const current = () => client.isAttachmentObservationCurrent(sessionId, proof);
    const owned = () => observing && current() && source.current === job;
    setReadError(undefined);
    void (async () => {
      const { page } = await client.request({ method: 'job/status', params: { target, job_id: job.job_id } }, 'domain_page', undefined, owned);
      if (page.domain !== `jobs.${job.job_id}` || page.read_cut.conversation_id !== target.conversation_id) throw new Error('Job details source changed.');
      const original = await domainOriginalValue<RuntimeClientJob>(page, client.reads.domainReader(target, owned));
      if (original.job_id !== job.job_id) throw new Error('Job details named another native execution.');
      if (owned()) setLoaded({ source: job, value: original, current });
    })().catch(cause => { if (owned()) setReadError({ source: job, message: cause instanceof Error ? cause.message : String(cause) }); });
    return () => { observing = false; };
  }, [expanded, job, client, sessionId, retry]);
  const live = jobActive(job), status = job.state === 'succeeded' ? tx('common:activity.job-completed') : job.state === 'cancelled' ? tx('common:activity.job-cancelled') : job.state === 'failed' ? tx('common:activity.job-failed') : jobStateLabel(tx, job.state);
  const label = value.bash?.command ?? value.tool_name;
  const progress = value.progress ? [value.progress.message, value.progress.completed == null ? undefined : value.progress.total == null ? tx('common:activity.completed', { n: value.progress.completed }) : `${value.progress.completed} / ${value.progress.total}`].filter(Boolean).join(' · ') : undefined;
  const elapsed = live ? timing ? now - Date.parse(timing.started_at) : undefined : value.result ? Number(value.result.duration_ms) : timing?.duration_ms ? Number(timing.duration_ms) : undefined;
  const durationCell = elapsed !== undefined && Number.isFinite(elapsed) ? <span className={css.duration}>{duration(elapsed, tx)}</span> : null;
  const output = value.result ? jobResultOutput({ content: value.result.content }) : undefined;
  const diagnostic = value.result && value.result.status.type !== 'cancelled' ? jobResultOutput({ status: value.result.status }) : undefined;
  const observable = live || deferred || !!output || !!diagnostic || !!value.result?.artifacts?.length || !!value.result?.managed_output;
  const pending = stop.pending || stopping;
  const canStop = job.state === 'starting' || job.state === 'running';
  const failure = canStop && showFailure ? stop.error : undefined;
  const stopLabel = armed ? tx('common:activity.job-stop-confirm') : failure ? tx('common:activity.job-stop-failed') : tx('common:activity.job-stop', { label });
  const pressStop = () => {
    if (!armed) { onArm(); return; }
    onArm();
    void stop.run(async (client, target, current) => {
      onStopped(current);
      try {
        const receipt = await client.request({ method: 'job/cancel', params: { target, job_id: job.job_id } }, 'job_cancellation_accepted', undefined, current);
        if (receipt.job_id !== job.job_id) throw new Error('Job cancellation receipt named another native execution.');
      } catch (cause) { if (current()) onStopped(() => false); throw cause; }
    }, false);
  };
  const body = <>
    <JobStateDot state={jobDot(job.state)} size={live ? 14 : 10} className={css.rowDot}/>
    {live ? <span className={css.main}>
      <span className={css.primary}><span className={css.label} title={label}>{label}</span></span>
      <span className={css.secondary} title={progress || status}><span className={css.kind}>{value.tool_name}</span>{progress && <span className={css.status}>{progress}</span>}{durationCell}</span>
    </span> : <><span className={css.kind}>{value.tool_name}</span><span className={css.label} title={label}>{label}</span><span className={css.status} title={status}>{status}</span>{durationCell}</>}
    {observable && <span className={css.chevronBox}><IconChevronDownOutline14 size={12} className={expanded ? `${css.chevron} ${css.chevronOpen}` : css.chevron}/></span>}
  </>;
  return <li className={css.item}><section data-job-id={job.job_id} data-job-state={job.state} aria-label={tx('common:activity.job-label', { name: job.tool_name })}>
    <div className={live ? `${css.rowLine} ${css.rowLineLive}` : css.rowLine}>
      {observable ? <button type="button" className={live ? css.row : `${css.row} ${css.rowSettled}`} aria-label={tx(expanded ? 'common:activity.job-collapse' : 'common:activity.job-expand', { label })} aria-expanded={expanded} onClick={onToggle}>{body}</button>
        : <span className={`${css.row} ${css.rowSettled} ${css.rowStatic}`}>{body}</span>}
      {client && canStop && <button type="button" className={armed ? `${css.stop} ${css.stopArmed}` : failure ? `${css.stop} ${css.stopFailed}` : css.stop} disabled={stop.disabled || pending} data-kill-state={pending ? 'pending' : armed ? 'armed' : failure ? 'failed' : 'idle'} aria-label={stopLabel} title={stopLabel} onClick={pressStop}>
        <IconStopFill16 size={10}/>{armed && <span className={css.stopLabel}>{tx('common:activity.job-stop-action')}</span>}
      </button>}
    </div>
    {failure && <p role="alert" className={`${css.notice} ${css.noticeError}`}>{displayText(tx, failure)}</p>}
    {expanded && observable && <div className={css.panel}>
      {deferred ? <div className={css.notice}>{readError?.source === job ? <><span role="alert">{readError.message}</span><button type="button" className={css.sectionClear} onClick={() => setRetry(value => value + 1)}>{tx('common:activity.job-details')}</button></> : tx('common:activity.loading')}</div> : <>
        <TerminalBlock command={label} output={output} running={live} copyText={label} runStateDot={false} maxLines={Infinity} lifecycle={{ state: jobDot(job.state), label: status }} labels={{
          copy: tx('tools:tool-card.copy'), copied: tx('tools:tool-card.copied'), signal: signal => tx('tools:copy.signal-value', { p0: signal }), exitCode: code => tx('tools:copy.exit-value', { p0: code }), noExitCode: tx('tools:tool-card.exit-status-unavailable'),
          running: tx('tools:tool-card.running'), failed: tx('tools:tool-card.failed'), done: tx('tools:tool-card.done'), noOutput: tx(value.result?.managed_output ? 'common:activity.job-output-retained' : 'tools:tool-card.no-output'), collapseAria: tx('tools:tool-card.collapse-output'), collapse: tx('tools:tool-card.collapse'), expandAria: n => tx('tools:tool-card.show-value-more-lines', { p0: n }), expand: n => tx('tools:tool-card.show-value-more-lines', { p0: n }),
        }}/>
        {diagnostic && <p className={jobDot(job.state) === 'error' ? `${css.notice} ${css.noticeError}` : css.notice}>{diagnostic}</p>}
        {value.result?.truncation?.truncated && <p className={css.notice}>{tx('common:activity.job-output-truncated')}</p>}
        {value.result?.managed_output && 'diagnostic' in value.result.managed_output && <p className={`${css.notice} ${css.noticeError}`}>{value.result.managed_output.diagnostic}</p>}
        {value.result && <ToolArtifacts result={value.result}/>}
      </>}
    </div>}
  </section></li>;
}
