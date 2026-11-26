/* Copyright (c) 2026 DeepSeek. MIT. Adapted from ui-schedule/ScheduleCatalogAction; see PROVENANCE.md. */
import { createContext, useContext, useEffect, useRef, useState, type ReactNode } from 'react';
import clsx from 'clsx';
import type { AppServerClient } from '../../client/app-server';
import type { ScheduleHeader } from '../../../../protocol/app-server/v80';
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
  return <span className="relative ml-[4px]">
    <Tooltip label={label}><button ref={anchor} type="button" className="inline-flex flex-none items-center justify-center w-[28px] h-[28px] p-0 border-0 rounded-[var(--dsw-radius-panel)] bg-transparent [color:var(--dsw-alias-label-tertiary)] cursor-pointer [&_svg]:flex-none focus-visible:[color:var(--dsw-alias-label-secondary)] focus-visible:bg-[var(--dsw-alias-interactive-bg-hover)] hover:[color:var(--dsw-alias-label-secondary)] hover:bg-[var(--dsw-alias-interactive-bg-hover)] aria-expanded:bg-[var(--dsw-alias-interactive-bg-hover)]" disabled={disabled} aria-label={label}
      aria-haspopup={rows.length === 1 ? undefined : 'dialog'} aria-expanded={rows.length === 1 ? undefined : open}
      onClick={() => { if (rows.length === 1 && navigation) show(rows[0]); else setOpen(!open); }}><IconClockOutline16/></button></Tooltip>
    <PickerPopover open={open} anchorRef={anchor} label={t('header.reminders')} className="fixed z-[100] box-border flex flex-col gap-[1px] w-[336px] max-w-[min(336px,calc(100vw-32px))] max-h-[min(420px,calc(100vh-140px))] m-0 p-[3px] overflow-auto list-none border-0 rounded-[var(--dsw-radius-lg)] bg-[var(--dsw-specific-menu)] [backdrop-filter:var(--dsw-menu-backdrop-filter)] [--dsh-scrollbar-thumb:var(--dsw-alias-scrollbar-bg-l2)] [--dsh-scrollbar-thumb-hover:var(--dsw-alias-scrollbar-hover-l2)] [--dsw-elevation-stroke-color:var(--dsw-alias-border-l1)] shadow-[var(--dsw-elevation-prominent)]" onClose={() => setOpen(false)}>
      {issue && <div className="flex items-start shrink-0 gap-[2px] box-border w-full min-h-[48px] px-[12px] py-[10px] rounded-[var(--dsw-radius-md)] [color:var(--dsw-alias-label-primary)]"><p>{t('list.error')}</p><Button size="sm" disabled={loading || !connected} onClick={() => void model.refreshNode({ session_id: sessionId, node_id: nodeId })}>{t('action.retry')}</Button></div>}
      <ul className="list-none m-0 p-0">{rows.map(row => {
        const next = row.next_due_utc && displayInstant(row.next_due_utc, locale), overdue = row.next_due_utc && Date.parse(row.next_due_utc) < Date.now();
        return <li key={taskKey(taskRef(row))} className={clsx('flex items-start shrink-0 gap-[2px] box-border w-full min-h-[48px] px-[12px] py-[10px] rounded-[var(--dsw-radius-md)] [color:var(--dsw-alias-label-primary)]', 'relative hover:bg-[var(--dsw-alias-interactive-bg-hover)]', overdue && 'bg-[var(--dsw-alias-state-warn-tertiary)]! hover:bg-[var(--dsw-alias-state-warn-tertiary)]!')}>
          <div className="flex flex-1 flex-col gap-[3px] min-w-0"><button type="button" className={clsx('text-[13px] leading-[18px] wrap-anywhere whitespace-normal', 'block p-0 border-0 bg-none [color:inherit] [font:inherit] text-left cursor-pointer after:content-[""] after:absolute after:inset-0 after:rounded-[var(--dsw-radius-md)] focus-visible:outline-none focus-visible:after:[outline:2px_solid_var(--dsw-focus-ring-color,var(--dsw-alias-state-business-primary))] focus-visible:after:-outline-offset-2')} onClick={() => show(row)}>{row.title}</button>
            <span className={clsx('flex flex-wrap items-center gap-[4px] min-w-0 [color:var(--dsw-alias-label-tertiary)] text-[10px] leading-[15px]', overdue && '[color:var(--dsw-alias-state-warn-label)]!')}>{frequency(row.spec, t, locale)}</span>
            {next && <span className="flex flex-wrap items-center gap-[4px] min-w-0 [color:var(--dsw-alias-label-tertiary)] text-[10px] leading-[15px]">{next.absolute} <span className="min-w-0 wrap-anywhere">({next.relative})</span></span>}
          </div>
          <button type="button" className="relative inline-flex flex-none items-center justify-center w-[20px] h-[20px] p-0 border-0 rounded-[var(--dsw-radius-sm)] bg-none [color:var(--dsw-alias-label-secondary)] cursor-pointer disabled:cursor-wait focus-visible:[color:var(--dsw-alias-label-primary)] focus-visible:bg-[var(--dsw-alias-interactive-bg-hover)] hover:[color:var(--dsw-alias-label-primary)] hover:bg-[var(--dsw-alias-interactive-bg-hover)]" aria-label={t('action.delete') + ' · ' + row.title} disabled={!connected} onClick={() => setDeleting(row)}><IconTrashOutline16 size={14}/></button>
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
