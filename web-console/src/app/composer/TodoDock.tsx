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
import type { TodoTask } from '../../../../protocol/app-server/v82';
import type { TodoDockState } from '../../bindings/composer-context';
import { IconChecklistOutline14, IconChevronDownOutline14 } from '../../presentation/primitives/icons';
import { StateDot } from '../../presentation/primitives/StateDot';
import css from './TodoDock.module.css';

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
  return <section className={css.root} aria-label={tx('agent:todo-dock.to-dos')} data-todo-state="current">
    <button type="button" className={css.header} aria-expanded={!collapsed} aria-controls={collapsed ? undefined : listId} onClick={() => setCollapsed(value => !value)}>
      <span className={css.lead} aria-hidden><IconChecklistOutline14 /></span><span className={css.title}>{tx('agent:todo-dock.to-dos')}</span>
      <span className={css.progress}>{progress(tx, tasks)}{state.deferred && <>{tasks.length ? ' · ' : ''}{tx('common:activity.loading')}</>}</span>
      <span className={css.chevron} data-collapsed={collapsed} aria-hidden><IconChevronDownOutline14 /></span>
    </button>
    {!collapsed && <><ol id={listId} className={css.list}>{tasks.map(task => {
      const text = task.status === 'in_progress' && task.active_form ? task.active_form : task.subject;
      return <li key={task.id} className={css.item} data-status={task.status} data-task-id={task.id}>
        <span className={css.glyph} role="img" aria-label={STATUS[task.status]}><StateDot state={task.status === 'completed' ? 'done' : task.status === 'in_progress' ? 'ongoing' : 'idle'} /></span>
        <span className={css.content} title={text}>{text}</span>
        {!!task.blocked_by?.length && <span className={css.dependency}>{tx('agent:todo-dock.after')}{' '}{task.blocked_by.map(id => `#${id}`).join(', ')}</span>}
      </li>;
    })}</ol>{state.deferred && <p role="status">{tx('common:activity.loading')}</p>}</>}
  </section>;
}
