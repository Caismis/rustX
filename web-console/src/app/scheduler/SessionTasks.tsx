/* Copyright (c) 2026 DeepSeek. MIT. Adapted from ui-schedule/ScheduleCatalogAction; see PROVENANCE.md. */
import { createContext, useContext, useEffect, useRef, useState, type ReactNode } from 'react';
import clsx from 'clsx';
import type { AppServerClient } from '../../client/app-server';
import type { ScheduleHeader } from '../../../../protocol/app-server/v62';
import { useLocale } from '../../locale/react';
import { Button } from '../../presentation/primitives/Button';
import { Toast } from '../../presentation/primitives/Toast';
import { Tooltip } from '../../presentation/primitives/Tooltip';
import { IconClockOutline16, IconTrashOutline16 } from '../../presentation/primitives/icons';
import { PickerPopover } from './PickerPopover';
import { useTaskCatalog, taskRef, taskKey, type TaskRef } from './model';
import { useTaskCopy } from './copy';
import { displayInstant, frequency } from './timing';
import { TaskDeleteDialog } from './TaskDetail';
import css from './SessionTasks.module.css';

export interface TaskNavigation { openTask: (task: TaskRef, title: string) => void; openTasks: () => void }
const Navigation = createContext<TaskNavigation | undefined>(undefined);
export function TaskNavigationProvider({ value, children }: { value: TaskNavigation; children: ReactNode }) {
  return <Navigation.Provider value={value}>{children}</Navigation.Provider>;
}
export const useTaskNavigation = () => useContext(Navigation);

export function SessionTasks({ client, sessionId, nodeId, disabled }: { client: AppServerClient; sessionId: string; nodeId: string; disabled?: boolean }) {
  const t = useTaskCopy(), locale = useLocale().active, navigation = useTaskNavigation();
  const { model, records, connected, issue, loading } = useTaskCatalog(client);
  useEffect(() => model.watchNode({ session_id: sessionId, node_id: nodeId }), [model, sessionId, nodeId]);
  const [open, setOpen] = useState(false), [deleting, setDeleting] = useState<ScheduleHeader>();
  useEffect(() => { setOpen(false); }, [sessionId, nodeId]);
  const anchor = useRef<HTMLButtonElement>(null);
  const rows = records.filter(row => row.binding.session_id === sessionId && row.binding.node_id === nodeId && row.state === 'enabled')
    .sort((a, b) => (a.next_due_utc ?? '').localeCompare(b.next_due_utc ?? ''));
  const known = useRef(false); if (rows.length) known.current = true;
  if (!rows.length && !(issue && known.current)) return null;
  const label = rows.length === 1 ? t('header.count-one') : rows.length ? t('header.count', { count: rows.length }) : t('header.reminders');
  const show = (row: ScheduleHeader) => { setOpen(false); navigation?.openTask(taskRef(row), row.title); };
  return <span className={css.root}>
    <Tooltip label={label}><button ref={anchor} type="button" className={css.trigger} disabled={disabled} aria-label={label}
      aria-haspopup={rows.length === 1 ? undefined : 'dialog'} aria-expanded={rows.length === 1 ? undefined : open}
      onClick={() => { if (rows.length === 1 && navigation) show(rows[0]); else setOpen(!open); }}><IconClockOutline16/></button></Tooltip>
    <PickerPopover open={open} anchorRef={anchor} label={t('header.reminders')} className={css.menu} onClose={() => setOpen(false)}>
      {issue && <div className={css.row}><p>{t('list.error')}</p><Button size="sm" disabled={loading || !connected} onClick={() => void model.refreshNode({ session_id: sessionId, node_id: nodeId })}>{t('action.retry')}</Button></div>}
      <ul className={css.list}>{rows.map(row => {
        const next = row.next_due_utc && displayInstant(row.next_due_utc, locale), overdue = row.next_due_utc && Date.parse(row.next_due_utc) < Date.now();
        return <li key={taskKey(taskRef(row))} className={clsx(css.row, css.rowTask, overdue && css.rowOverdue)}>
          <div className={css.body}><button type="button" className={clsx(css.title, css.openButton)} onClick={() => show(row)}>{row.title}</button>
            <span className={clsx(css.metadata, overdue && css.metadataOverdue)}>{frequency(row.spec, t, locale)}</span>
            {next && <span className={css.nextRun}>{next.absolute} <span className={css.nextRunRelative}>({next.relative})</span></span>}
          </div>
          <button type="button" className={css.deleteButton} aria-label={t('action.delete') + ' · ' + row.title} disabled={!connected} onClick={() => setDeleting(row)}><IconTrashOutline16 size={14}/></button>
        </li>;
      })}</ul>
      <Button size="sm" onClick={() => { setOpen(false); navigation?.openTasks(); }}>{t('header.more')}</Button>
    </PickerPopover>
    {deleting && <TaskDeleteDialog client={client} record={deleting} close={() => setDeleting(undefined)}/>}
  </span>;
}
export function SchedulerNotices({ client }: { client: AppServerClient }) {
  const { model, notice, noticeId } = useTaskCatalog(client), t = useTaskCopy();
  return notice && <Toast key={noticeId} tone={notice === 'failed' ? undefined : 'success'}
    text={t(notice === 'deleted' ? 'delete.success' : notice === 'updated' ? 'edit.saved' : 'edit.error')} onDone={model.dismissNotice}/>;
}
