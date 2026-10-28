import { useEffect, useRef, useState } from 'react';
import type { ScheduleHeader, ScheduledWorkflowRecovery } from '../../../../protocol/app-server/v62';
import { Button } from '../../presentation/primitives/Button';
import { IconClockOutline16 } from '../../presentation/primitives/icons';
import { useLocale } from '../../locale/react';
import { useTaskCopy, type TaskManagerKey } from './copy';
import { type SchedulerModel, taskKey, taskRef } from './model';
import { displayInstant } from './timing';
import css from './TaskManager.module.css';
import extra from './scheduler.module.css';

export function TaskRuns({ record, model, connected, generation }: { record: ScheduleHeader; model: SchedulerModel; connected: boolean; generation: number }) {
  const t = useTaskCopy(), locale = useLocale().active;
  const receipt = record.last_trigger;
  const start = receipt?.target.type === 'workflow' ? receipt.target.start_id : undefined;
  const identity = JSON.stringify([taskKey(taskRef(record)), start, generation]);
  const [data, setData] = useState<{ identity: string; recovery: ScheduledWorkflowRecovery }>();
  const [loading, setLoading] = useState(false), [error, setError] = useState(false), [busy, setBusy] = useState<'cancel' | 'reconcile'>();
  const owner = useRef({}); const mounted = useRef(true);
  useEffect(() => { mounted.current = true; return () => { mounted.current = false; owner.current = {}; }; }, []);
  const shown = data?.identity === identity ? data.recovery : undefined;
  const read = async (after = '0') => {
    if (!start || !connected) return;
    const ticket = {}; owner.current = ticket;
    setLoading(true); setError(false);
    try {
      const value = await model.recovery(taskRef(record), start, after);
      if (!mounted.current || owner.current !== ticket) return;
      if (!value) { setError(true); return; }
      setData(previous => ({ identity, recovery: { ...value, effects: after === '0' ? value.effects
        : [...(previous?.identity === identity ? previous.recovery.effects : []), ...value.effects].filter((effect, index, effects) => effects.findIndex(row => row.ordinal === effect.ordinal) === index) } }));
    } catch { if (mounted.current && owner.current === ticket) setError(true); }
    finally { if (mounted.current && owner.current === ticket) setLoading(false); }
  };
  useEffect(() => { setBusy(undefined); void read(); return () => { owner.current = {}; }; }, [identity, connected]);
  const manage = async (action: 'cancel' | 'reconcile') => {
    if (!start || !connected || busy) return;
    const ticket = {}; owner.current = ticket;
    setBusy(action); setError(false);
    try {
      const value = await model.manageRun(taskRef(record), start, action);
      if (mounted.current && owner.current === ticket && value) setData({ identity, recovery: value });
    } catch { if (mounted.current && owner.current === ticket) setError(true); }
    finally { if (mounted.current && owner.current === ticket) setBusy(undefined); }
  };
  const status = (value: ScheduledWorkflowRecovery): TaskManagerKey => {
    if (value.start.state.type === 'accepted') return 'records.waiting';
    if (value.start.state.type === 'claimed') return 'records.running';
    if (value.start.state.outcome.type === 'failed') return 'records.failed';
    return ({ completed: 'records.completed', cancelled: 'records.cancelled', timed_out: 'records.timed-out', rejected_before_start: 'records.rejected',
      interrupted: 'records.interrupted', outcome_unknown: 'records.unknown' } as const)[value.start.state.outcome.type];
  };
  if (!receipt) return <p className={css.emptyTitle}>{t('records.empty')}</p>;
  const when = displayInstant(receipt.trigger_id.occurrence_utc, locale);
  return <div className={css.recordsPanel}>
    <article className={css.delivery}>
      <span className={css.deliveryGlyph}><IconClockOutline16/></span>
      <div className={css.deliveryBody}><header className={css.deliveryHead}><time className={css.deliveryTime} dateTime={receipt.trigger_id.occurrence_utc}>{when.absolute}</time>
        <span>{start ? shown ? t(status(shown)) : t('records.loading') : t('records.queued')}</span></header>
        <p>{t('records.latest-only')}</p>
        {!start && <p className={css.detailNotice}>{t('records.trigger-help')}</p>}
        {loading && <p role="status">{t('records.loading')}</p>}
        {error && <div role="alert"><p>{t('records.error')}</p><Button size="sm" disabled={loading || !connected} onClick={() => void read()}>{t('action.retry')}</Button></div>}
        {shown && <>
          {shown.start.cancellation_requested_at && <p role="status">{t('records.cancellation')}</p>}
          {(shown.capacity_blocked || shown.start.recovery_blocked) && <p className={css.detailNotice}>{t('records.blocked')}</p>}
          {shown.start.state.type === 'settled' && shown.start.state.outcome.type === 'outcome_unknown' && <p>{t('records.unknown-detail')}</p>}
          {shown.start.state.type === 'settled' && shown.start.state.outcome.type === 'rejected_before_start' && <p>{t('records.failed-start')}</p>}
          <div className={extra.inlineActions}>
            <Button size="sm" disabled={!connected || !!busy} onClick={() => void manage('reconcile')}>{t(busy === 'reconcile' ? 'records.checking' : 'records.check')}</Button>
            {shown.start.state.type !== 'settled' && <Button size="sm" disabled={!connected || !!busy || !!shown.start.cancellation_requested_at} onClick={() => void manage('cancel')}>{t(busy === 'cancel' ? 'records.stopping' : 'records.stop')}</Button>}
          </div>
          {!!shown.effects.length && <section><h3 className={extra.sectionTitle}>{t('records.operations')}</h3><ul className={extra.steps}>{shown.effects.map(effect => <li key={effect.ordinal}>
            <span>{effect.node.node}</span><span>{t(effect.physical_settlement_proven ? 'records.step-done' : 'records.step-waiting')}</span>
          </li>)}</ul></section>}
          {shown.next_effect_ordinal != null && <Button size="sm" disabled={loading || !connected} onClick={() => void read(shown.next_effect_ordinal!)}>{t('records.more')}</Button>}
          {shown.result != null && <section><h3 className={extra.sectionTitle}>{t('records.result')}</h3><pre className={extra.result}>{JSON.stringify(shown.result, null, 2)}</pre></section>}
          <details className={extra.technical}><summary>{t('records.details')}</summary><dl>
            <dt>{t('records.run-id')}</dt><dd>{JSON.stringify(shown.start.run_id)}</dd>
            <dt>{t('records.start-id')}</dt><dd>{shown.start.id}</dd>
          </dl>{shown.start.recovery_blocked && <p>{shown.start.recovery_blocked}</p>}
            {shown.start.state.type === 'accepted' && shown.start.state.waiting && <p>{shown.start.state.waiting}</p>}
            {shown.start.state.type === 'settled' && 'reason' in shown.start.state.outcome && <p>{shown.start.state.outcome.reason}</p>}
            {shown.effects.map(effect => <p key={effect.ordinal}>{effect.node.node} · {effect.owner.type} {effect.diagnostic}</p>)}
          </details>
        </>}
      </div>
    </article>
  </div>;
}
