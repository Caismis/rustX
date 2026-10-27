/* Copyright (c) 2026 DeepSeek. MIT. Harness task detail layout adapted to native reads; see PROVENANCE.md. */
import { useEffect, useId, useLayoutEffect, useRef, useState } from 'react';
import type { ScheduleHeader, ScheduleSnapshot } from '../../../../protocol/app-server/v61';
import type { AppServerClient } from '../../client/app-server';
import { useClientSelector, sameValue } from '../../client/selectors';
import { Button } from '../../presentation/primitives/Button';
import { Menu } from '../../presentation/primitives/Menu';
import { Modal } from '../../presentation/primitives/Modal';
import { Tooltip } from '../../presentation/primitives/Tooltip';
import { IconCloseOutline16, IconEllipsisOutline16, IconTrashOutline16, IconLinkOutline16 } from '../../presentation/primitives/icons';
import { taskKey, taskRef, type TaskRef, useTaskCatalog, TaskChangeError } from './model';
import { useTaskCopy } from './copy';
import { TaskRule } from './TaskRule';
import { TaskRuns } from './TaskRuns';
import css from './TaskManager.module.css';
import extra from './scheduler.module.css';

export function TaskTabTitle({ client, identity, fallback }: { client: AppServerClient; identity: string; fallback: string }) {
  const { records } = useTaskCatalog(client);
  return <>{records.find(record => taskKey(taskRef(record)) === identity)?.title ?? fallback}</>;
}

export function TaskDeleteDialog({ client, record, close, deleted }: {
  client: AppServerClient; record: ScheduleHeader; close: () => void; deleted?: () => void;
}) {
  const { model, connected } = useTaskCatalog(client), t = useTaskCopy();
  const [busy, setBusy] = useState(false), [failure, setFailure] = useState<'failed' | 'conflict' | 'uncertain'>();
  const mounted = useRef(true);
  useEffect(() => { mounted.current = true; return () => { mounted.current = false; }; }, []);
  const remove = async () => {
    if (busy || !connected) return;
    setBusy(true); setFailure(undefined);
    try {
      await model.change(taskRef(record), { type: 'delete', id: record.id, expected_revision: record.revision });
      if (mounted.current) { deleted?.(); close(); }
    } catch (cause) {
      if (mounted.current) setFailure(cause instanceof TaskChangeError && cause.kind !== 'invalid' ? cause.kind : 'failed');
    } finally { if (mounted.current) setBusy(false); }
  };
  return <Modal open title={t('delete.confirm')} closeLabel={t('action.cancel')} description={t('delete.explanation')} onClose={close}
    className={css.confirmDialog} footer={<div className={css.confirmActions}><Button onClick={close}>{t('action.cancel')}</Button>
      <Button variant="primary" className={css.deleteButton} disabled={!connected || busy} onClick={() => void remove()}>{t(busy ? 'delete.pending' : 'action.delete')}</Button></div>}>
    <p>{record.title}</p>
    {failure && <p role="alert">{t(failure === 'conflict' ? 'edit.conflict' : failure === 'uncertain' ? 'edit.uncertain' : 'delete.failed')}</p>}
  </Modal>;
}
export function TaskDetail({ client, task, title, close, openSession }: {
  client: AppServerClient; task: TaskRef; title?: string; close?: () => void; openSession?: (ref: TaskRef) => void;
}) {
  const t = useTaskCopy(), tabId = useId();
  const { model, records, sessions, connected, loading: catalogLoading, issue } = useTaskCatalog(client);
  const connection = useClientSelector(client, state => ({ generation: state.generation, authorityId: state.authorityId }), sameValue);
  const key = taskKey(task);
  useEffect(() => model.watchNode(task), [model, key]);
  const record = records.find(row => taskKey(taskRef(row)) === key);
  const [body, setBody] = useState<{ key: string; authorityId?: string; generation: number; value?: ScheduleSnapshot }>();
  const [tab, setTab] = useState<'rule' | 'records'>('rule'), [menu, setMenu] = useState(false);
  const [loading, setLoading] = useState(false), [failure, setFailure] = useState(false), [changing, setChanging] = useState(false);
  const [deleting, setDeleting] = useState<ScheduleHeader>();
  const aside = useRef<HTMLElement>(null), request = useRef({});
  const authority = JSON.stringify([key, connection.generation, connection.authorityId]);
  const loaded = body?.key === key && (!connection.authorityId || body.authorityId === connection.authorityId) ? body.value : undefined;
  const verified = connected && body?.generation === connection.generation && body?.authorityId === connection.authorityId && loaded?.revision === record?.revision;
  const reload = async () => {
    const owner = {}; request.current = owner;
    setLoading(true); setFailure(false);
    try { const value = await model.get(task, () => request.current === owner); if (request.current === owner) setBody({ key, authorityId: connection.authorityId, generation: connection.generation, value }); }
    catch { if (request.current === owner) setFailure(true); }
    finally { if (request.current === owner) setLoading(false); }
  };
  useEffect(() => { if (connected && record) void reload(); return () => { request.current = {}; }; }, [authority, record?.revision, connected]);
  useLayoutEffect(() => { aside.current?.focus(); }, [key]);
  const session = sessions.get(task.session_id);
  const sessionName = session?.name ?? session?.generated_title?.text ?? session?.preview ?? task.session_id;
  const toggle = async () => {
    if (!record || changing) return;
    setMenu(false); setChanging(true);
    try { await model.change(task, { type: 'set_enabled', id: task.id, expected_revision: record.revision, enabled: record.state !== 'enabled' }); await reload(); }
    catch { setFailure(true); }
    finally { setChanging(false); }
  };
  const row = loaded && record;
  return <aside ref={aside} className={css.detail} tabIndex={-1} aria-label={t('detail.label')} data-task-detail>
    <div className={css.detailTabsBar}>
      <div className={css.detailTabs} role="tablist" aria-label={t('detail.tabs')}>
        {(['rule', 'records'] as const).map((name, index, tabs) => <button type="button" key={name} id={tabId + '-tab-' + name} role="tab" className={css.detailTab}
          aria-selected={tab === name} aria-controls={tabId + '-panel-' + name} tabIndex={tab === name ? 0 : -1}
          onClick={() => setTab(name)} onKeyDown={event => {
            if (event.altKey || event.ctrlKey || event.metaKey || event.shiftKey) return;
            const next = event.key === 'Home' ? 0 : event.key === 'End' ? 1 : event.key === 'ArrowRight' ? (index + 1) % 2 : event.key === 'ArrowLeft' ? (index + 1) % 2 : undefined;
            if (next === undefined) return; event.preventDefault(); setTab(tabs[next]); (event.currentTarget.parentElement?.children[next] as HTMLElement)?.focus();
          }}>{t(name === 'rule' ? 'detail.rule' : 'records.title')}</button>)}
      </div>
      <div className={css.detailActions}>
        {openSession && <Tooltip label={session ? t('scope.original') + ' · ' + sessionName : t('scope.unavailable')}>
          <Button size="sm" className={css.detailIconButton} aria-label={t('scope.original')} disabled={!connected || !session} onClick={() => openSession(task)}><IconLinkOutline16/></Button>
        </Tooltip>}
        <Menu open={menu} onClose={() => setMenu(false)} align="end" items={[
          { id: 'toggle', label: t(record?.state === 'enabled' ? 'action.pause' : 'action.resume'), disabled: !connected || !record || record.state === 'completed' || changing },
          { id: 'delete', label: t('action.delete'), icon: <IconTrashOutline16/>, danger: true, disabled: !connected || !record },
        ]} onSelect={id => { setMenu(false); if (id === 'delete' && record) setDeleting(record); else void toggle(); }}
          anchor={<Button size="sm" className={css.detailIconButton} aria-label={t('detail.more')} aria-haspopup="menu" aria-expanded={menu} onClick={() => setMenu(!menu)}><IconEllipsisOutline16/></Button>}/>
        {close && <Button size="sm" className={css.detailIconButton} aria-label={t('detail.close')} onClick={close}><IconCloseOutline16/></Button>}
      </div>
    </div>
    <div className={css.detailScroll}>
      {!connected && <p className={css.detailNotice} role="status">{t('connection.disconnected')}</p>}
      {connected && loaded && record && loaded.revision !== record.revision && <p className={css.detailNotice} role="status">{t('list.refreshing')} <Button size="sm" onClick={() => void reload()}>{t('action.retry')}</Button></p>}
      {(loading || catalogLoading) && !loaded && <div className={css.empty}><p role="status">{t('list.loading')}</p></div>}
      {(failure || issue && !record) && <div className={css.notice} role="alert"><p>{t('list.error')}</p><Button size="sm" onClick={() => { void model.refreshNode(task); if (record) void reload(); }}>{t('action.retry')}</Button></div>}
      {!loading && !catalogLoading && (!loaded || !record) && !failure && !issue && <div className={css.empty}><h2>{title}</h2><p>{t('list.no-task')}</p></div>}
      {row && <div id={tabId + '-panel-' + tab} role="tabpanel" aria-labelledby={tabId + '-tab-' + tab}>
        <div hidden={tab !== 'rule'}><TaskRule value={loaded} record={record} model={model} connected={verified} reload={reload}/></div>
        {tab === 'records' && <TaskRuns record={record} model={model} connected={connected} generation={connection.generation}/>}
        {tab === 'rule' && <details className={extra.technical}><summary>{t('records.details')}</summary><dl>
          <dt>{t('task.identifier')}</dt><dd>{record.id.ordinal}</dd><dt>{t('scope.original')}</dt><dd>{task.session_id}</dd><dt>{t('scope.node')}</dt><dd>{task.node_id}</dd>
        </dl>{record.last_failure && <p>{record.last_failure}</p>}</details>}
      </div>}
    </div>
    {deleting && <TaskDeleteDialog client={client} record={deleting} close={() => setDeleting(undefined)} deleted={close}/>}
  </aside>;
}
