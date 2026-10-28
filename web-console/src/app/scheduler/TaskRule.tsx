/* Copyright (c) 2026 DeepSeek. MIT. Harness rule layout adapted to native task mutations; see PROVENANCE.md. */
import { useEffect, useRef, useSyncExternalStore } from 'react';
import type { ScheduleHeader, ScheduleSnapshot, ScheduleMutation } from '../../../../protocol/app-server/v62';
import { useLocale } from '../../locale/react';
import { Button } from '../../presentation/primitives/Button';
import { TaskChangeError, taskRef, type SchedulerModel } from './model';
import { useTaskCopy } from './copy';
import { timingValue, sameTiming, TimingInputError, displayInstant } from './timing';
import { TimingEditor } from './TimingEditor';
import css from './TaskManager.module.css';
import extra from './scheduler.module.css';
import { hasUnrepresentableProperty, readJson } from '../../../shared/json-numbers';

export function TaskRule({ value, record, model, connected, reload }: {
  value: ScheduleSnapshot; record: ScheduleHeader; model: SchedulerModel; connected: boolean; reload: () => Promise<void>;
}) {
  const t = useTaskCopy(), locale = useLocale().active;
  const editor = model.editor(value);
  const { baseline, draft, contentEdited, incoming, saving, error, savedRefreshError } = useSyncExternalStore(editor.subscribe, editor.getSnapshot, editor.getSnapshot);
  const mounted = useRef(true);
  useEffect(() => { mounted.current = true; return () => { mounted.current = false; }; }, []);
  const original = baseline.values, dirty = editor.dirty;
  useEffect(() => editor.observe(value), [editor, value]);
  const finished = record.state === 'completed';
  const disabled = !connected || saving || finished;
  const readOnlyArguments = baseline.unsafeArguments || value.target.type === 'workflow' && hasUnrepresentableProperty(value.target, 'arguments');
  const timeChanged = !sameTiming(draft.timing, original.timing);
  let validation: string | undefined;
  if (!draft.title.trim() || [...draft.title.trim()].length > 120) validation = t('content.invalid-name');
  else if (value.target.type === 'conversation_inbound') {
    if (new TextEncoder().encode(draft.content).byteLength > 16 * 1024) validation = t('content.too-large');
    else if (!draft.content.trim()) validation = t('content.invalid-message');
  } else if (value.target.type === 'workflow' && contentEdited) {
    const parsed = readJson(draft.content);
    if (readOnlyArguments) validation = t('content.unsafe-numbers');
    else if (!parsed.ok) validation = t(parsed.reason === 'number' ? 'content.unsafe-numbers' : 'content.invalid-json');
    else if (new TextEncoder().encode(JSON.stringify(parsed.value)).byteLength > 16 * 1024) validation = t('content.too-large');
  }
  if (!validation && timeChanged) {
    try { timingValue(draft.timing); } catch (cause) { validation = t(cause instanceof TimingInputError ? cause.key : 'timing.invalid'); }
  }
  const save = async () => {
    if (disabled || !dirty || validation || incoming || editor.getSnapshot().draft !== draft) return;
    const ticket = editor.beginSave(); if (!ticket) return;
    try {
      const timing = !sameTiming(ticket.draft.timing, ticket.baseline.values.timing) ? timingValue(ticket.draft.timing) : undefined;
      const common = { id: value.id, expected_revision: ticket.baseline.revision, title: ticket.draft.title.trim(), ...(timing ? { timing } : {}) };
      const inputs = value.target.type === 'workflow' && ticket.contentEdited ? readJson(ticket.draft.content) : undefined;
      if (inputs && (!inputs.ok || readOnlyArguments)) throw new TaskChangeError('invalid');
      const mutation: ScheduleMutation = value.target.type === 'workflow' && record.target.type === 'workflow'
        ? !ticket.contentEdited
          ? { type: 'update_metadata', ...common }
          : { type: 'update_workflow', ...common, workflow_id: record.target.workflow_id, arguments: inputs!.ok ? inputs!.value : undefined }
        : { type: 'update', ...common, text: ticket.draft.content };
      const result = await model.change(taskRef(record), mutation);
      if (result.status !== 'applied') throw new TaskChangeError('invalid');
      editor.applied(result.receipt, ticket.draft);
    } catch (cause) {
      editor.failed(cause instanceof TaskChangeError && cause.kind === 'conflict' ? 'conflict'
        : cause instanceof TaskChangeError && cause.kind === 'uncertain' ? 'uncertain' : 'invalid');
      if (mounted.current && cause instanceof TaskChangeError && cause.kind === 'conflict') { try { await reload(); } catch { /* Keep the saved version and the conflict notice. */ } }
      return;
    }
    if (!mounted.current) return;
    try {
      const fresh = await model.get(taskRef(record));
      if (fresh) editor.observe(fresh);
      await reload();
    } catch { editor.refreshFailed(); }
  };
  const cancel = editor.discard;
  const instant = record.state === 'enabled' && record.next_due_utc ? displayInstant(record.next_due_utc, locale) : undefined;
  return <>
    <header className={css.detailHeader}>
      {finished ? <h2 className={css.readonlyName}>{value.title}</h2> : <input className={css.editName} aria-label={t('detail.name')} value={draft.title} disabled={disabled} maxLength={240} onChange={event => editor.edit({ ...draft, title: event.target.value })}/>}
    </header>
    {record.state === 'disabled' && <p className={css.detailNotice} role="status">{t('task.paused-note')}</p>}
    {instant && <div className={css.nextRun}><span>{t('detail.nextRun')}</span><span>{instant.absolute} <span className={css.nextRunRelative}>({instant.relative})</span></span></div>}
    <div className={css.ruleCard}>
      <label className={extra.instructionLabel} htmlFor={'task-content-' + record.id.ordinal}>{t(value.target.type === 'workflow' ? 'content.arguments' : 'content.label')}</label>
      {finished ? <pre className={css.readonlyPrompt}>{readOnlyArguments ? t('content.unsafe-saved') : draft.content}</pre> : <textarea id={'task-content-' + record.id.ordinal} className={css.instruction} value={readOnlyArguments ? '' : draft.content} disabled={disabled || readOnlyArguments} spellCheck={value.target.type !== 'workflow'} onChange={event => editor.edit({ ...draft, content: event.target.value })}/>}
    </div>
    {readOnlyArguments && !finished && <p className={css.detailNotice} role="status">{t('content.unsafe-saved')}</p>}
    {record.target.type === 'workflow' && <p className={css.detailNotice}>{t('target.workflow-name', { name: record.target.workflow_id })}</p>}
    {record.target.type === 'workflow' && <p className={css.detailNotice}>{t('content.workflow-version')}</p>}
    <TimingEditor value={draft.timing} change={timing => editor.edit({ ...draft, timing })} disabled={disabled} t={t} locale={locale}/>
    {record.last_failure && <p className={css.detailNotice} role="alert">{t('task.failure')}</p>}
    {savedRefreshError && <p role="status" className={css.detailNotice}>{t('edit.saved-refresh')}</p>}
    {incoming && <section className={css.saveFailure} role="alert">
      <p>{t('edit.conflict')}</p><strong>{t('edit.latest')}</strong><p>{incoming.values.title}</p>
      <pre className={extra.savedVersion}>{incoming.unsafeArguments ? t('content.unsafe-saved') : incoming.values.content}</pre>
      <div className={extra.inlineActions}><Button size="sm" onClick={cancel}>{t('edit.use-latest')}</Button><Button size="sm" onClick={editor.rebase}>{t('edit.keep-edits')}</Button></div>
    </section>}
    {dirty && !finished && <footer className={css.saveFooter}>
      <div className={css.saveNotice}>{t('edit.unsaved')}</div>
      {(validation || error) && <p className={css.saveFailure} role="alert">{validation ?? t(error === 'conflict' ? 'edit.conflict' : error === 'uncertain' ? 'edit.uncertain' : 'edit.invalid')}</p>}
      <div className={extra.inlineActions}><Button disabled={saving} onClick={cancel}>{t('action.cancel')}</Button>
        <Button variant="primary" disabled={disabled || !!validation || !!incoming} onClick={() => void save()}>{t(saving ? 'action.saving' : 'action.save')}</Button></div>
    </footer>}
  </>;
}
