/* Copyright (c) 2026 DeepSeek. MIT. Adapted TurnProcessNodeView; see PROVENANCE.md. */
import { IconChevronDownOutline14 } from '../primitives/icons';
import css from './TurnProcess.module.css';
export function TurnProcess({ id, open, toggle, tools, messages }: { id: string; open: boolean; toggle: () => void; tools: number; messages: number }) {
  const labels = [];
  if (tools) labels.push(`${tools} tool call${tools === 1 ? '' : 's'}`);
  if (messages) labels.push(`${messages} message${messages === 1 ? '' : 's'}`);
  return <button type="button" className={css.root} data-open={open || undefined} data-turn-process={id} aria-expanded={open} onClick={event => { event.currentTarget.focus(); toggle(); }}>
    <span className={css.label}>{labels.join(' · ') || 'Thought for a while'}</span><IconChevronDownOutline14 className={css.chevron}/>
  </button>;
}
