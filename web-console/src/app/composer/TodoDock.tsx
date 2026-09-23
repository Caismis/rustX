/* Copyright (c) 2026 DeepSeek. MIT. Source-derived; see PROVENANCE.md. */
// Adapted from DeepSeek Harness ui-conversation TodoPanel: empty renders nothing,
// collapsed count summary, bounded expanded list and status glyphs. The rustX
// Conversation owns the list; this card owns only its disclosure state and has no
// mutation. Harness's turn-scoped plan lifetime is deliberately NOT adopted:
// rustX Todo is the conversation-owned authority and is never cleared locally on
// input, turn start, Assistant completion, navigation, reconnect or disclosure.
import { useEffect, useId, useState } from 'react';
import type { TodoTask } from '../../../../protocol/app-server/v19';
import type { TodoDockState } from '../../bindings/composer-context';
import { IconChecklistOutline14, IconChevronDownOutline14 } from '../../presentation/primitives/icons';
import css from './TodoDock.module.css';

const STATUS: Record<TodoTask['status'], string> = { pending: 'Pending', in_progress: 'In progress', completed: 'Completed', deleted: 'Deleted' };

function StatusGlyph({ status }: { status: TodoTask['status'] }) {
  if (status === 'completed') return <svg width={14} height={14} viewBox="0 0 14 14" fill="none" className={css.completed}>
    <circle cx="7" cy="7" r="6.4" stroke="currentColor" strokeWidth="1.2" /><path d="M4 7.1 6.1 9.1 10.2 4.9" stroke="currentColor" strokeWidth="1.3" />
  </svg>;
  if (status === 'in_progress') return <svg width={14} height={14} viewBox="0 0 14 14" fill="none" className={css.progressGlyph}>
    <circle cx="7" cy="7" r="6.4" stroke="currentColor" strokeWidth="1.2" strokeDasharray="30 10" />
  </svg>;
  return <svg width={14} height={14} viewBox="0 0 14 14" fill="none" className={css.pending}>
    <circle cx="7" cy="7" r="6.4" stroke="currentColor" strokeWidth="1.2" strokeDasharray="2.4 2.4" />
  </svg>;
}

const ChecklistGlyph = () => <IconChecklistOutline14 />;

/** Zero-count segments are omitted; a non-empty list keeps at least one. */
function progress(tasks: readonly TodoTask[]) {
  const done = tasks.filter(task => task.status === 'completed').length;
  const active = tasks.filter(task => task.status === 'in_progress').length;
  const pending = tasks.length - done - active;
  return [done && `${done} completed`, active && `${active} in progress`, pending && `${pending} pending`].filter(Boolean).join(' · ');
}

export function TodoDock({ state }: { state: TodoDockState }) {
  const [collapsed, setCollapsed] = useState(true);
  const listId = useId();
  // A list that disappears and returns is a new presentation, not a restored one.
  // Ordinary non-empty updates — a task completing, a new task arriving — leave an
  // open disclosure open; only the visible list going away resets it.
  const listed = state.kind === 'current' && state.tasks.length > 0;
  useEffect(() => { if (!listed) setCollapsed(true); }, [listed]);
  // Harness renders no panel for an empty plan, and rustX keeps that result for
  // both native facts: extension absent and extension composed with an empty
  // current list are distinct runtime facts with one ordinary visual outcome —
  // no dock, no wrapper and no reserved composer-stack height. An empty card is
  // not capability discovery; that belongs to the capability/settings surfaces.
  if (state.kind === 'absent' || !state.tasks.length) return null;
  const { tasks } = state;
  return <section className={css.root} aria-label="To-dos" data-todo-state="current">
    <button type="button" className={css.header} aria-expanded={!collapsed} aria-controls={collapsed ? undefined : listId} onClick={() => setCollapsed(value => !value)}>
      <span className={css.lead}><ChecklistGlyph /></span><span className={css.title}>To-dos</span>
      <span className={css.progress}>{progress(tasks)}</span>
      <span className={css.chevron} data-collapsed={collapsed} aria-hidden><IconChevronDownOutline14 /></span>
    </button>
    {!collapsed && <ol id={listId} className={css.list}>{tasks.map(task => {
      const text = task.status === 'in_progress' && task.active_form ? task.active_form : task.subject;
      return <li key={task.id} className={css.item} data-status={task.status} data-task-id={task.id}>
        <span className={css.glyph} aria-hidden><StatusGlyph status={task.status} /></span>
        <span className={css.content} title={text}>{text}</span>
        <span className={css.hidden}>{STATUS[task.status]}</span>
        {!!task.blocked_by?.length && <span className={css.dependency}>after {task.blocked_by.map(id => `#${id}`).join(', ')}</span>}
      </li>;
    })}</ol>}
  </section>;
}
