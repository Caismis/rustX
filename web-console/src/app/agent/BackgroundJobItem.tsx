/* Copyright (c) 2026 DeepSeek. MIT. JobListAction task rows adapted to native Jobs; see PROVENANCE.md. */
import { useEffect, useId, useRef, useState } from 'react';
import type { RuntimeClientJob, TraceTiming } from '../../../../protocol/app-server/v80';
import { domainOriginalValue } from '../../../../protocol/app-server/domain-observation';
import type { AppServerClient } from '../../client/app-server';
import { jobResultOutput } from '../../bindings/jobs';
import { jobStateLabel } from '../../bindings/status-labels';
import { useTranslation } from '../../locale/react';
import { displayText, type Translate } from '../../locale/translation';
import { useLiveJobOutput } from './useLiveJobOutput';
import { JobStateDot } from './JobStateDot';
import { HoverCard } from '../../presentation/primitives/HoverCard';
import { MarqueeText, useHoverMarquee } from '../../presentation/primitives/Marquee';
import { TerminalBlock } from '../../presentation/primitives/TerminalBlock';
import { Tooltip } from '../../presentation/primitives/Tooltip';
import { IconChevronDownOutline14, IconClockOutline16, IconStopFill16 } from '../../presentation/primitives/icons';
import { ToolArtifacts } from '../components/Artifact';
import { useActivityRequest } from '../components/useActivityRequest';
import { jobActive, jobDot } from './job-state';
import { backgroundJobsCss } from './backgroundJobsCss';

const KILL_FAILED_MS = 4000;

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
  const [loaded, setLoaded] = useState<{ client: AppServerClient; sessionId: string; value: RuntimeClientJob; current: () => boolean }>();
  const [retry, setRetry] = useState(0), [showFailure, setShowFailure] = useState(false);
  const [onStop, setOnStop] = useState(false);
  const labelRef = useRef<HTMLSpanElement>(null);
  const marquee = useHoverMarquee(labelRef);
  const summaryId = useId();

  useEffect(() => {
    setShowFailure(!!stop.error);
    if (!stop.error) return;
    const timer = setTimeout(() => setShowFailure(false), KILL_FAILED_MS);
    return () => clearTimeout(timer);
  }, [stop.error]);

  const retained = loaded && loaded.client === client && loaded.sessionId === sessionId && loaded.value.job_id === job.job_id && loaded.current() ? loaded.value : undefined;
  const value = job.deferred_fields?.length ? retained ?? job : job;
  const deferred = !!value.deferred_fields?.length;

  useEffect(() => {
    if (!client || !sessionId) return;
    const proof = client.getSnapshot().views[sessionId]?.attachmentObservation;
    if (!client.isAttachmentObservationCurrent(sessionId, proof)) return;
    const current = () => client.isAttachmentObservationCurrent(sessionId, proof);
    if (!job.deferred_fields?.length) { setLoaded({ client, sessionId, value: job, current }); return; }
    if (!expanded) return;
    const target = client.target(sessionId);
    let observing = true;
    const owned = () => observing && current() && source.current === job;
    setReadError(undefined);
    void (async () => {
      const { page } = await client.request({ method: 'job/status', params: { target, job_id: job.job_id } }, 'domain_page', undefined, owned);
      if (page.domain !== `jobs.${job.job_id}` || page.read_cut.conversation_id !== target.conversation_id) throw new Error('Job details source changed.');
      const original = await domainOriginalValue<RuntimeClientJob>(page, client.reads.domainReader(target, owned));
      if (original.job_id !== job.job_id) throw new Error('Job details named another native execution.');
      if (owned()) setLoaded({ client, sessionId, value: original, current });
    })().catch(cause => { if (owned()) setReadError({ source: job, message: cause instanceof Error ? cause.message : String(cause) }); });
    return () => { observing = false; };
  }, [expanded, job, client, sessionId, retry]);

  const live = jobActive(job);
  const status = job.state === 'succeeded' ? tx('common:activity.job-completed') : job.state === 'cancelled' ? tx('common:activity.job-cancelled') : job.state === 'failed' ? tx('common:activity.job-failed') : jobStateLabel(tx, job.state);
  const label = value.bash?.command ?? value.tool_name;
  const progress = value.progress ? [value.progress.message, value.progress.completed == null ? undefined : value.progress.total == null ? tx('common:activity.completed', { n: value.progress.completed }) : `${value.progress.completed} / ${value.progress.total}`].filter(Boolean).join(' · ') : undefined;
  const elapsed = live ? timing ? now - Date.parse(timing.started_at) : undefined : value.result ? Number(value.result.duration_ms) : timing?.duration_ms ? Number(timing.duration_ms) : undefined;
  const durationText = elapsed !== undefined && Number.isFinite(elapsed) ? duration(elapsed, tx) : undefined;
  const durationCell = durationText !== undefined ? <span className={backgroundJobsCss.duration}>{durationText}</span> : null;
  const tail = useLiveJobOutput(client, sessionId, expanded && live ? job.job_id : undefined, expanded && live && job.state !== 'publishing_terminal');
  const output = value.result ? jobResultOutput({ content: value.result.content }) ?? (live ? tail?.text : undefined) : tail?.text;
  const diagnostic = value.result && value.result.status.type !== 'cancelled' ? jobResultOutput({ status: value.result.status }) : undefined;
  const exitDetail = !live && value.result?.exit_code != null && value.result.exit_code !== 0 ? tx('tools:copy.exit-value', { p0: value.result.exit_code }) : undefined;
  const detail = live ? progress : diagnostic ?? exitDetail;
  const stateText = detail ?? status;
  const dot = jobDot(job.state, value.result?.exit_code);
  const observable = live || deferred || !!output || !!diagnostic || !!value.result?.artifacts?.length || !!value.result?.managed_output;
  const pending = stop.pending || stopping;
  const detailError = readError?.source === job ? readError.message : undefined;
  const canStop = job.state === 'starting' || job.state === 'running';
  const failure = canStop && showFailure ? stop.error : undefined;
  const stopTitle = armed ? tx('common:activity.job-stop-confirm') : failure ? tx('common:activity.job-stop-failed') : tx('common:activity.job-stop-tooltip');
  const stopAria = armed || failure ? stopTitle : tx('common:activity.job-stop', { label });

  useEffect(() => {
    if (!canStop) setOnStop(false);
  }, [canStop]);

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
    <span className={backgroundJobsCss.glyph}><JobStateDot state={dot}/></span>
    <MarqueeText ref={labelRef} className={backgroundJobsCss.labelCls} data-job-label="">{label}</MarqueeText>
    <span id={summaryId} className={backgroundJobsCss.visuallyHidden}>{`${value.tool_name} · ${stateText}`}</span>
    {durationCell}
  </>;

  const row = (
    <div className={observable ? `${backgroundJobsCss.rowLine} ${backgroundJobsCss.rowInteractive}` : backgroundJobsCss.rowLine} data-job-row="" {...marquee}>
      {observable
        ? <button type="button" className={backgroundJobsCss.row} aria-expanded={expanded} aria-label={tx(expanded ? 'common:activity.job-collapse' : 'common:activity.job-expand', { label })} aria-describedby={summaryId} onClick={onToggle}>{body}</button>
        : <span className={backgroundJobsCss.row} aria-describedby={summaryId}>{body}</span>}
      {client && canStop && (
        <Tooltip label={stopTitle} side="bottom" delayMs={500}>
          <button
            type="button"
            className={armed ? `${backgroundJobsCss.stop} ${backgroundJobsCss.stopArmed}` : failure ? `${backgroundJobsCss.stop} ${backgroundJobsCss.stopFailed}` : backgroundJobsCss.stop}
            disabled={stop.disabled || pending}
            data-kill-state={pending ? 'pending' : armed ? 'armed' : failure ? 'failed' : 'idle'}
            aria-label={stopAria}
            onPointerEnter={() => setOnStop(true)}
            onPointerLeave={() => setOnStop(false)}
            onClick={pressStop}
          >
            <IconStopFill16 size={12}/>
            {armed && <span className={backgroundJobsCss.stopLabel}>{tx('common:activity.job-stop-action')}</span>}
          </button>
        </Tooltip>
      )}
      <span className={backgroundJobsCss.chevron} aria-hidden="true">
        {observable ? <IconChevronDownOutline14 size={14} className={expanded ? backgroundJobsCss.chevronOpen : undefined}/> : null}
      </span>
    </div>
  );

  return <li className={backgroundJobsCss.item}><section data-job-id={job.job_id} data-job-state={job.state} aria-label={tx('common:activity.job-label', { name: job.tool_name })}>
    <HoverCard
      grouped
      disabled={onStop && canStop}
      anchor={row}
      content={
        <div className={backgroundJobsCss.hoverDetails} data-session-panel-details="">
          <div className={backgroundJobsCss.hoverLine}>{value.tool_name}</div>
          <div className={backgroundJobsCss.hoverStatus}>
            <JobStateDot state={dot}/>
            <span>{stateText}</span>
          </div>
          {durationText !== undefined && (
            <div className={backgroundJobsCss.hoverFacts}>
              <span className={backgroundJobsCss.hoverFact} data-fact="duration">
                <span className={backgroundJobsCss.hoverFactIcon} aria-hidden="true"><IconClockOutline16 size={14}/></span>
                {durationText}
              </span>
            </div>
          )}
        </div>
      }
    />
    {failure && <p role="alert" className={`${backgroundJobsCss.notice} ${backgroundJobsCss.noticeError}`}>{displayText(tx, failure)}</p>}
    {expanded && observable && <div className={backgroundJobsCss.panel}>
      {detailError && <div className={backgroundJobsCss.notice}><span role="alert">{detailError}</span><button type="button" className={backgroundJobsCss.sectionClear} onClick={() => setRetry(value => value + 1)}>{tx('common:activity.job-details')}</button></div>}
      {deferred ? !detailError && <div className={backgroundJobsCss.notice}>{tx('common:activity.loading')}</div> : <>
        {tail?.truncated && <div className={backgroundJobsCss.notice}>{tx('common:activity.job-output-gap')}</div>}
        {tail?.error && <div className={`${backgroundJobsCss.notice} ${backgroundJobsCss.noticeError}`} role="alert">{tx('common:activity.job-output-error', { error: tail.error })}</div>}
        <TerminalBlock command={label} output={output} running={jobActive(value)} copyText={label} runStateDot={false} maxLines={Infinity} lifecycle={{ state: dot, label: status }} labels={{
          copy: tx('tools:tool-card.copy'), copied: tx('tools:tool-card.copied'), signal: signal => tx('tools:copy.signal-value', { p0: signal }), exitCode: code => tx('tools:copy.exit-value', { p0: code }), noExitCode: tx('tools:tool-card.exit-status-unavailable'),
          running: tx('tools:tool-card.running'), failed: tx('tools:tool-card.failed'), done: tx('tools:tool-card.done'), noOutput: tx(value.result?.managed_output ? 'common:activity.job-output-retained' : 'tools:tool-card.no-output'), collapseAria: tx('tools:tool-card.collapse-output'), collapse: tx('tools:tool-card.collapse'), expandAria: n => tx('tools:tool-card.show-value-more-lines', { p0: n }), expand: n => tx('tools:tool-card.show-value-more-lines', { p0: n }),
        }}/>
        {diagnostic && <p className={dot === 'error' ? `${backgroundJobsCss.notice} ${backgroundJobsCss.noticeError}` : backgroundJobsCss.notice}>{diagnostic}</p>}
        {value.result?.truncation?.truncated && <p className={backgroundJobsCss.notice}>{tx('common:activity.job-output-truncated')}</p>}
        {value.result?.managed_output && 'diagnostic' in value.result.managed_output && <p className={`${backgroundJobsCss.notice} ${backgroundJobsCss.noticeError}`}>{value.result.managed_output.diagnostic}</p>}
        {value.result && <ToolArtifacts result={value.result}/>}
      </>}
    </div>}
  </section></li>;
}
