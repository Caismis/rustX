import { useEffect, useId, useRef, useState, useSyncExternalStore } from 'react';
import type { ScheduleTiming, SessionModelConfig, SourceSettings } from '../../../../protocol/app-server/v77';
import type { AppServerClient } from '../../client/app-server';
import { sameValue, transportSelection, useClientSelector } from '../../client/selectors';
import type { WorkspaceAuthority } from '../../workspaces/authority';
import type { ProductHostWorkspaces, WorkspaceCatalog } from '../../workspaces/host';
import { sameEndpoint } from '../../workspaces/endpoint';
import { catalogChoices, nextModelSelection } from '../../bindings/model-catalog';
import { ModelSelect } from '../../presentation/agent/ModelSelect';
import { Button } from '../../presentation/primitives/Button';
import { Input } from '../../presentation/primitives/Input';
import { Menu } from '../../presentation/primitives/Menu';
import { Modal } from '../../presentation/primitives/Modal';
import { IconChevronDownOutline14 } from '../../presentation/primitives/icons';
import { useLocale, useTranslation } from '../../locale/react';
import { displayText, type DisplayText } from '../../locale/translation';
import { sessionCatalog, sessionModelBlock, WorkspaceControls } from '../new-conversation/WorkspaceControls';
import { TaskCreation } from './creation';
import type { TaskRef } from './model';
import { useTaskCopy, type TaskManagerKey } from './copy';
import { browserZone, timingDraft, timingValue, TimingInputError } from './timing';
import { TimingEditor } from './TimingEditor';
import css from './TaskCreateDialog.module.css';

const initialTiming = () => timingDraft({ type: 'cron', expression: '0 9 * * *', time_zone: browserZone() });
const restoreTiming = (timing?: ScheduleTiming) => !timing ? initialTiming()
  : timing.type === 'cron' ? timingDraft(timing)
  : timing.type === 'at' ? timingDraft({ type: 'at', at_utc: timing.at })
  : timingDraft({ type: 'interval', every_seconds: timing.type === 'interval' ? timing.every_seconds : timing.seconds, anchor_utc: new Date().toISOString() });
const errorCopy = {
  invalid: 'create.error.invalid', disconnected: 'connection.disconnected', workspace_changed: 'create.error.workspace', authority_changed: 'connection.changed',
  create_failed: 'create.error.session', create_uncertain: 'create.error.session-uncertain', schedule_failed: 'create.error.schedule', schedule_uncertain: 'create.error.schedule-uncertain',
} as const;

/** Draft controls only read Workspace configuration. The explicit Create gesture
 * owns the dedicated Session and its first native schedule command. */
export function TaskCreateDialog({ client, host, authority, creation, open, close, created }: {
  client: AppServerClient; host: ProductHostWorkspaces; authority: WorkspaceAuthority; creation: TaskCreation;
  open: boolean; close: () => void; created: (task: TaskRef, title: string) => void;
}) {
  const t = useTaskCopy(), tx = useTranslation(), locale = useLocale().active, id = useId();
  const transport = useClientSelector(client, transportSelection, sameValue);
  const state = useSyncExternalStore(creation.subscribe, creation.getSnapshot);
  const [text, setText] = useState(state.draft?.text ?? ''), [title, setTitle] = useState(state.draft?.title ?? ''), [timing, setTiming] = useState(() => restoreTiming(state.draft?.timing));
  const [advanced, setAdvanced] = useState(true), [workspaceMenu, setWorkspaceMenu] = useState(false);
  const [catalog, setCatalog] = useState<WorkspaceCatalog>(), [catalogFailed, setCatalogFailed] = useState(false);
  const [catalogRead, setCatalogRead] = useState(0), [workspaceId, setWorkspace] = useState(state.draft?.workspaceId ?? ''), [intent, setIntent] = useState<SessionModelConfig | undefined>(state.draft?.model);
  const lifetime = useRef<object>({});
  useEffect(() => { const owner = {}; lifetime.current = owner; return () => { lifetime.current = {}; }; }, [creation]);
  useEffect(() => {
    const draft = creation.getSnapshot().draft;
    setText(draft?.text ?? ''); setTitle(draft?.title ?? ''); setTiming(restoreTiming(draft?.timing)); setWorkspace(draft?.workspaceId ?? ''); setIntent(draft?.model); setValidation(undefined);
  }, [creation]);
  const [validation, setValidation] = useState<TaskManagerKey>();
  const pending = state.phase === 'creating' || state.phase === 'scheduling';
  const connected = transport.connection === 'connected';
  const disabled = state.locked || pending;
  const bound = sameEndpoint(catalog?.endpoint, transport.endpoint);
  const selected = bound ? catalog?.workspaces.find(value => value.id === workspaceId) : undefined;
  useEffect(() => {
    if (!open) return;
    const abort = new AbortController(); setCatalogFailed(false);
    void authority.observe(abort.signal).then(value => {
      if (abort.signal.aborted || !value.current()) return;
      setCatalog(value.catalog);
      setWorkspace(current => current || value.catalog.workspaces[0]?.id || '');
    }, () => { if (!abort.signal.aborted) setCatalogFailed(true); });
    return () => abort.abort();
  }, [open, authority, transport.endpoint, transport.authorityRevision, catalogRead]);
  const dismiss = () => { if (!pending) close(); };
  const completed = (task: TaskRef | undefined, name: string, owner: object) => {
    if (!task || owner !== lifetime.current || !creation.isCurrentAuthority() || creation.getSnapshot().task !== task || !creation.reset()) return;
    created(task, name); setText(''); setTitle(''); setTiming(initialTiming()); setIntent(undefined); setValidation(undefined);
  };
  // A receipt may arrive while this route is unmounted. Reopening the dialog
  // adopts that retained result only in its original native authority.
  useEffect(() => {
    if (!open || state.phase !== 'created' || !state.task || !creation.isCurrentAuthority()) return;
    const owner = lifetime.current;
    queueMicrotask(() => completed(state.task, state.draft?.title ?? title, owner));
  }, [open, state.phase, creation]);
  const submit = async () => {
    if (pending) return;
    const owner = lifetime.current;
    if (state.locked) { completed(await creation.retry(), title.trim() || Array.from(text.trim().split(/\r?\n/)[0]).slice(0, 120).join(''), owner); return; }
    const name = title.trim() || Array.from(text.trim().split(/\r?\n/)[0]).slice(0, 120).join('');
    if (!text.trim()) { setValidation('content.invalid-message'); return; }
    if (new TextEncoder().encode(text).length > 16_384) { setValidation('content.too-large'); return; }
    if (!name || Array.from(name).length > 120 || new TextEncoder().encode(name).length > 480) { setValidation('content.invalid-name'); return; }
    try {
      const value = timingValue(timing); setValidation(undefined);
      completed(await creation.submit({ workspaceId, title: name, text, timing: value, ...(intent ? { model: intent } : {}) }), name, owner);
    } catch (error) { setValidation(error instanceof TimingInputError ? error.key : 'create.error.invalid'); }
  };
  const render = (source?: SourceSettings, reading = false, blocked?: DisplayText, readError?: string, reread?: () => void) => {
    const selection = intent ?? (source?.session_models?.kind === 'available' ? source.session_models.default_model : undefined);
    const modelBlock = sessionModelBlock(source, intent);
    const ready = !!selected && !!source && source.session_models?.kind === 'available' && !reading && !readError && !blocked && !modelBlock;
    const sourceProblem = readError ? t('create.error.workspace-read')
      : !reading && blocked ? source?.prospective_diagnostic ?? displayText(tx, blocked)
      : !reading && source && !source.session_models ? t('create.error.workspace-read') : undefined;
    return <Modal open={open} onClose={dismiss} title={t('create.title')} closeLabel={t('create.close')} className={css.dialog} contentClassName={css.content}
      footer={<><Button disabled={pending} onClick={dismiss}>{t('action.cancel')}</Button>{state.phase === 'failed' && state.locked && state.retryable && <Button onClick={() => creation.reset()}>{t('create.edit')}</Button>}{state.errorKind === 'create_uncertain' && <Button onClick={() => creation.reset()}>{t('create.start-over')}</Button>}<Button variant="primary" disabled={!connected || pending || (state.locked ? !state.retryable : !ready || !text.trim())} onClick={() => void submit()}>{t(pending ? 'create.pending' : state.locked ? 'create.retry' : 'create.confirm')}</Button></>}>
      <div className={css.form}>
        <label className={css.instructionLabel} htmlFor={id + '-instruction'}>{t('create.instruction')}</label>
        <textarea data-modal-autofocus="" id={id + '-instruction'} className={css.instruction} placeholder={t('create.placeholder')} value={text} disabled={disabled} onChange={event => { setText(event.target.value); setValidation(undefined); }}/>
        <div className={css.timing}><TimingEditor value={timing} change={value => { setTiming(value); setValidation(undefined); }} disabled={disabled} t={t} locale={locale}/></div>
        <button className={css.advancedToggle} type="button" aria-expanded={advanced} aria-controls={id + '-advanced'} onClick={() => setAdvanced(value => !value)}>{t('create.advanced')}<IconChevronDownOutline14 className={advanced ? css.expanded : undefined}/></button>
        {advanced && <div id={id + '-advanced'} className={css.advanced}>
          <div className={css.row}><span>{t('create.workspace')}</span><Menu className={css.workspaceMenu} open={workspaceMenu} selectedId={selected?.id} onClose={() => setWorkspaceMenu(false)} items={(bound ? catalog?.workspaces ?? [] : []).map(value => ({ id: value.id, label: value.displayName }))}
            onSelect={id => { setWorkspace(id); setIntent(undefined); setWorkspaceMenu(false); }}
            anchor={<Button size="sm" className={css.workspace} aria-label={t('create.workspace')} aria-haspopup="menu" aria-expanded={workspaceMenu} disabled={disabled || !bound || !catalog?.workspaces.length} onClick={() => setWorkspaceMenu(value => !value)}><span>{selected?.displayName ?? t('create.choose-workspace')}</span><IconChevronDownOutline14/></Button>}/></div>
          <div className={css.row}><label htmlFor={id + '-name'}>{t('create.name')}</label><Input id={id + '-name'} aria-label={t('create.name')} className={css.name} value={title} placeholder={t('create.name-placeholder')} disabled={disabled} onChange={event => { setTitle(event.target.value); setValidation(undefined); }}/></div>
          <div className={css.row}><span>{t('create.model')}</span><ModelSelect choices={catalogChoices(sessionCatalog(source))} current={selection?.model} profile={selection?.profile ?? undefined} disabled={disabled || !selected || !source || reading} loading={reading} load={() => {}} binding={workspaceId}
            choose={async value => { setIntent(nextModelSelection(selection, value)); return true; }}/></div>
        </div>}
        <p className={css.note}>{t('create.local-note')}</p>
        {!connected && <p className={css.error} role="status">{t('connection.disconnected')}</p>}
        {catalogFailed && <div className={css.error} role="alert">{t('create.error.catalog')} <Button size="sm" onClick={() => setCatalogRead(value => value + 1)}>{t('action.retry')}</Button></div>}
        {!catalogFailed && catalog && !selected && <p className={css.error} role="status">{t('create.error.no-workspace')}</p>}
        {sourceProblem && <div className={css.error} role="alert">{sourceProblem}{reread && <Button size="sm" disabled={pending || reading || !connected} onClick={reread}>{t('action.retry')}</Button>}</div>}
        {modelBlock && <p className={css.error} role="status">{displayText(tx, modelBlock)}</p>}
        {(validation || state.errorKind) && <p className={css.error} role="alert">{t(validation ?? errorCopy[state.errorKind!])}</p>}
      </div>
    </Modal>;
  };
  return open && selected ? <WorkspaceControls visible={open} presentationKey={id} client={client} host={host} workspaceId={selected.id}>{(source, approval) => render(source, approval.reading, approval.block(), approval.actor.getSnapshot().context.readError, () => approval.actor.send({ type: 'REFRESH' }))}</WorkspaceControls> : render();
}
