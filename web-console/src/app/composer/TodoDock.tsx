import type { Translate } from '../../locale/translation';
import { useTranslation } from '../../locale/react';
/* Copyright (c) 2026 DeepSeek. MIT. Source-derived; see PROVENANCE.md. */
// Adapted from DeepSeek Harness ui-conversation TodoPanel: empty renders nothing,
// collapsed count summary, bounded expanded list and status glyphs. The rustX
// Conversation owns the list; this card owns only its disclosure state and has no
// mutation. Harness's turn-scoped plan lifetime is deliberately NOT adopted:
// rustX Todo is the conversation-owned authority and is never cleared locally on
// input, turn start, Assistant completion, navigation, reconnect or disclosure.
import { useEffect, useId, useState } from 'react';
import type { TodoTask } from '../../../../protocol/app-server/v80';
import type { TodoDockState } from '../../bindings/composer-context';
import { IconChecklistOutline14, IconChevronDownOutline14 } from '../../presentation/primitives/icons';
import { StateDot } from '../../presentation/primitives/StateDot';

/** Zero-count segments are omitted; a non-empty list keeps at least one. */
function progress(tx: Translate, tasks: readonly TodoTask[]) {
 const done = tasks.filter(task => task.status === 'completed').length;
 const active = tasks.filter(task => task.status === 'in_progress').length;
 const pending = tasks.length - done - active;
 return [done && tx('agent:copy.value-completed', { p0: done }), active && tx('agent:copy.value-in-progress', { p0: active }), pending && tx('agent:copy.value-pending', { p0: pending })].filter(Boolean).join(' · ');
}

export function TodoDock({ state }: { state: TodoDockState }) {
 const tx = useTranslation();
 const STATUS: Record<TodoTask['status'], string> = { pending: tx('agent:copy.pending'), in_progress: tx('agent:copy.in-progress'), completed: tx('agent:copy.completed'), deleted: tx('agent:copy.deleted') };
 const [collapsed, setCollapsed] = useState(true);
 const listId = useId();
 // A list that disappears and returns is a new presentation, not a restored one.
 // Ordinary non-empty updates — a task completing, a new task arriving — leave an
 // open disclosure open; only the visible list going away resets it.
 const listed = state.kind === 'current' && (state.tasks.length > 0 || !!state.deferred);
 useEffect(() => { if (!listed) setCollapsed(true); }, [listed]);
 // Harness renders no panel for an empty plan, and rustX keeps that result for
 // both native facts: extension absent and extension composed with an empty
 // current list are distinct runtime facts with one ordinary visual outcome —
 // no dock, no wrapper and no reserved composer-stack height. An empty card is
 // not capability discovery; that belongs to the capability/settings surfaces.
 if (state.kind === 'absent' || !listed) return null;
 const { tasks } = state;
 return <section className="box-border flex-none overflow-hidden my-0 mx-auto w-[calc(100%-2*var(--dsh-composer-side-clearance)-4*var(--dsh-composer-dock-inset))] max-w-[calc(var(--dsh-composer-card-max-width)-4*var(--dsh-composer-dock-inset))] flex flex-col gap-[8px] px-[12px] py-[6px] border-0 rounded-[var(--dsw-radius-lg)] bg-[var(--dsw-specific-menu)] [backdrop-filter:var(--dsw-menu-backdrop-filter)] shadow-[var(--dsw-elevation-panel)] [--dsw-elevation-stroke-color:var(--dsw-alias-border-l1)] [--dsh-scrollbar-thumb:var(--dsw-alias-scrollbar-bg-l2)] [--dsh-scrollbar-thumb-hover:var(--dsw-alias-scrollbar-hover-l2)]" aria-label={tx('agent:todo-dock.to-dos')} data-todo-state="current">
 <button type="button" className="box-border flex items-center gap-[10px] w-full p-0 border-none bg-transparent [color:inherit] text-left cursor-pointer focus-visible:[outline:2px_solid_var(--dsw-alias-label-tertiary)] focus-visible:-outline-offset-2 focus-visible:rounded-[12px]" aria-expanded={!collapsed} aria-controls={collapsed ? undefined : listId} onClick={() => setCollapsed(value => !value)}>
 <span className="grid flex-none place-items-center [color:var(--dsw-alias-label-tertiary)] w-[16px] h-[16px]" aria-hidden><IconChecklistOutline14 /></span><span className="flex-none text-[13px] leading-[24px] font-medium [color:var(--dsw-alias-label-primary)]">{tx('agent:todo-dock.to-dos')}</span>
 <span className="flex-auto min-w-0 overflow-hidden text-[13px] leading-[20px] [color:var(--dsw-alias-label-tertiary)] text-ellipsis whitespace-nowrap">{progress(tx, tasks)}{state.deferred && <>{tasks.length ? ' · ' : ''}{tx('common:activity.loading')}</>}</span>
 <span className="grid flex-none place-items-center [color:var(--dsw-alias-label-tertiary)] w-[16px] h-[16px] data-[collapsed=true]:rotate-180" data-collapsed={collapsed} aria-hidden><IconChevronDownOutline14 /></span>
 </button>
 {!collapsed && <><ol id={listId} className="flex flex-col gap-[8px] max-h-[180px] m-0 p-0 overflow-y-auto list-none">{tasks.map(task => {
 const text = task.status === 'in_progress' && task.active_form ? task.active_form : task.subject;
 return <li key={task.id} className="flex items-center gap-[10px] min-w-0 text-[13px] leading-[20px] [color:var(--dsw-alias-label-secondary)]" data-status={task.status} data-task-id={task.id}>
 <span className="grid flex-none place-items-center [color:var(--dsw-alias-label-tertiary)] w-[16px] h-[16px]" role="img" aria-label={STATUS[task.status]}><StateDot state={task.status === 'completed' ? 'done' : task.status === 'in_progress' ? 'ongoing' : 'idle'} /></span>
 <span className="flex-auto min-w-0 overflow-hidden text-ellipsis whitespace-nowrap" title={text}>{text}</span>
 {!!task.blocked_by?.length && <span className="flex-none text-[12px] [color:var(--dsw-alias-label-tertiary)] whitespace-nowrap">{tx('agent:todo-dock.after')}{' '}{task.blocked_by.map(id => `#${id}`).join(', ')}</span>}
 </li>;
 })}</ol>{state.deferred && <p role="status">{tx('common:activity.loading')}</p>}</>}
 </section>;
}
