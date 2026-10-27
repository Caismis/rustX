/* Copyright (c) 2026 DeepSeek. MIT. Harness rule layout adapted to native task mutations; see PROVENANCE.md. */
import { useEffect, useRef, useState } from 'react';
import type { ScheduleHeader, ScheduleSnapshot, ScheduleMutation } from '../../../../protocol/app-server/v61';
import { useLocale } from '../../locale/react';
import { Button } from '../../presentation/primitives/Button';
import { TaskChangeError, taskRef, type SchedulerModel } from './model';
import { useTaskCopy } from './copy';
import { timingDraft, timingValue, sameTiming, TimingInputError, displayInstant, type TimingDraft } from './timing';
import { TimingEditor } from './TimingEditor';
import css from './TaskManager.module.css';
import extra from './scheduler.module.css';

interface Draft { title: string; content: string; timing: TimingDraft }
function draftOf(value: ScheduleSnapshot): Draft {
  return { title: value.title, content: value.target.type === 'conversation_inbound' ? value.target.text : JSON.stringify(value.target.arguments, null, 2), timing: timingDraft(value.spec) };
}
const same = (a: Draft, b: Draft) => a.title === b.title && a.content === b.content && sameTiming(a.timing, b.timing);
export function TaskRule({ value, record, model, connected, reload }: {
  value: ScheduleSnapshot; record: ScheduleHeader; model: SchedulerModel; connected: boolean; reload: () => Promise<void>;
}) {
  const t = useTaskCopy(), locale = useLocale().active;
  const [baseline, setBaseline] = useState(value), [draft, setDraft] = useState(() => draftOf(value));
  const [incoming, setIncoming] = useState<ScheduleSnapshot>(), [saving, setSaving] = useState(false);
  const [error, setError] = useState<string>(), [savedRefreshError, setSavedRefreshError] = useState(false);
  const mounted = useRef(true);
  useEffect(() => { mounted.current = true; return () => { mounted.current = false; }; }, []);
  const original = draftOf(baseline), dirty = !same(draft, original);
  const dirtyRef = useRef(dirty); dirtyRef.current = dirty;
  useEffect(() => {
    if (!dirtyRef.current) { setBaseline(value); setDraft(draftOf(value)); setIncoming(undefined); }
    else if (value.revision !== baseline.revision) setIncoming(value);
  }, [value]);
  const finished = record.state === 'completed';
  const disabled = !connected || saving || finished;
  const timeChanged = !sameTiming(draft.timing, original.timing);
  let validation: string | undefined;
  if (!draft.title.trim() || [...draft.title.trim()].length > 120) validation = t('content.invalid-name');
  else if (new TextEncoder().encode(draft.content).byteLength > 16 * 1024) validation = t('content.too-large');
  else if (value.target.type === 'conversation_inbound' && !draft.content.trim()) validation = t('content.invalid-message');
  else if (value.target.type === 'workflow') {
    try { JSON.parse(draft.content); } catch { validation = t('content.invalid-json'); }
  }
  if (!validation && timeChanged) {
    try { timingValue(draft.timing); } catch (cause) { validation = t(cause instanceof TimingInputError ? cause.key : 'timing.invalid'); }
  }
  const save = async () => {
    if (disabled || !dirty || validation || incoming) return;
    const timing = timeChanged ? timingValue(draft.timing) : undefined;
    const common = { id: value.id, expected_revision: baseline.revision, title: draft.title.trim(), ...(timing ? { timing } : {}) };
    const mutation: ScheduleMutation = value.target.type === 'workflow' && record.target.type === 'workflow'
      ? { type: 'update_workflow', ...common, workflow_id: record.target.workflow_id, arguments: JSON.parse(draft.content) }
      : { type: 'update', ...common, text: draft.content };
    setSaving(true); setError(undefined); setSavedRefreshError(false);
    try {
      await model.change(taskRef(record), mutation);
    } catch (cause) {
      if (!mounted.current) return;
      setError(t(cause instanceof TaskChangeError && cause.kind === 'conflict' ? 'edit.conflict'
        : cause instanceof TaskChangeError && cause.kind === 'uncertain' ? 'edit.uncertain' : 'edit.invalid'));
      if (cause instanceof TaskChangeError && cause.kind === 'conflict') { try { await reload(); } catch { /* Keep the saved version and the conflict notice. */ } }
      if (mounted.current) setSaving(false);
      return;
    }
    if (!mounted.current) return;
    try {
      const fresh = await model.get(taskRef(record));
      if (mounted.current && fresh) { setBaseline(fresh); setDraft(draftOf(fresh)); setIncoming(undefined); }
      await reload();
    } catch { if (mounted.current) setSavedRefreshError(true); }
    finally { if (mounted.current) setSaving(false); }
  };
  const cancel = () => {
    const saved = incoming ?? value;
    setBaseline(saved); setDraft(draftOf(saved)); setIncoming(undefined); setError(undefined);
  };
  const instant = record.state === 'enabled' && record.next_due_utc ? displayInstant(record.next_due_utc, locale) : undefined;
  return <>
    <header className={css.detailHeader}>
      {finished ? <h2 className={css.readonlyName}>{value.title}</h2> : <input className={css.editName} aria-label={t('detail.name')} value={draft.title} disabled={disabled} maxLength={240} onChange={event => setDraft({ ...draft, title: event.target.value })}/>}
    </header>
    {record.state === 'disabled' && <p className={css.detailNotice} role="status">{t('task.paused-note')}</p>}
    {instant && <div className={css.nextRun}><span>{t('detail.nextRun')}</span><span>{instant.absolute} <span className={css.nextRunRelative}>({instant.relative})</span></span></div>}
    <div className={css.ruleCard}>
      <label className={extra.instructionLabel} htmlFor={'task-content-' + record.id.ordinal}>{t(value.target.type === 'workflow' ? 'content.arguments' : 'content.label')}</label>
      {finished ? <pre className={css.readonlyPrompt}>{draft.content}</pre> : <textarea id={'task-content-' + record.id.ordinal} className={css.instruction} value={draft.content} disabled={disabled} spellCheck={value.target.type !== 'workflow'} onChange={event => setDraft({ ...draft, content: event.target.value })}/>}
    </div>
    {record.target.type === 'workflow' && <p className={css.detailNotice}>{t('target.workflow-name', { name: record.target.workflow_id })}</p>}
    <TimingEditor value={draft.timing} change={timing => setDraft({ ...draft, timing })} disabled={disabled} t={t} locale={locale}/>
    {record.last_failure && <p className={css.detailNotice} role="alert">{t('task.failure')}</p>}
    {savedRefreshError && <p role="status" className={css.detailNotice}>{t('edit.saved-refresh')}</p>}
    {incoming && <section className={css.saveFailure} role="alert">
      <p>{t('edit.conflict')}</p><strong>{t('edit.latest')}</strong><p>{incoming.title}</p>
      <pre className={extra.savedVersion}>{incoming.target.type === 'conversation_inbound' ? incoming.target.text : JSON.stringify(incoming.target.arguments, null, 2)}</pre>
      <div className={extra.inlineActions}><Button size="sm" onClick={cancel}>{t('edit.use-latest')}</Button><Button size="sm" onClick={() => { setBaseline(incoming); setIncoming(undefined); setError(undefined); }}>{t('edit.keep-edits')}</Button></div>
    </section>}
    {dirty && !finished && <footer className={css.saveFooter}>
      <div className={css.saveNotice}>{t('edit.unsaved')}</div>
      {(validation || error) && <p className={css.saveFailure} role="alert">{validation ?? error}</p>}
      <div className={extra.inlineActions}><Button disabled={saving} onClick={cancel}>{t('action.cancel')}</Button>
        <Button variant="primary" disabled={disabled || !!validation || !!incoming} onClick={() => void save()}>{t(saving ? 'action.saving' : 'action.save')}</Button></div>
    </footer>}
  </>;
}
