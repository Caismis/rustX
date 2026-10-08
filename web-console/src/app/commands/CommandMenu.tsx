import { useTranslation } from '../../locale/react';
/* Copyright (c) 2026 DeepSeek. MIT. Rewritten from ui-input-trigger/MenuView; see PROVENANCE.md. */
import { Fragment, useLayoutEffect, useRef } from 'react';
import type { CommandId } from './registry';
import { CommandIcon } from './CommandIcons';
import css from './Commands.module.css';
export type MenuAction = CommandId | 'file';
const section = (id: MenuAction) => id === 'file' || id === 'goal' ? 'add' : 'commands';
export function CommandMenu({ rows, active, select, highlight, fileDescription }: {
  rows: readonly { id: MenuAction }[]; active: number; select: (id: MenuAction) => void; highlight: (index: number) => void;
  fileDescription?: string;
}) {
  const tx = useTranslation();
  const root = useRef<HTMLDivElement>(null);
  useLayoutEffect(() => {
    const element = root.current;
    if (!element) return;
    const measure = () => { element.style.maxHeight = `${Math.max(0, Math.min(400, (element.parentElement?.getBoundingClientRect().top ?? 408) - 12))}px`; };
    measure();
    const observer = new ResizeObserver(measure);
    if (element.parentElement) observer.observe(element.parentElement);
    window.addEventListener('resize', measure);
    return () => { observer.disconnect(); window.removeEventListener('resize', measure); };
  }, []);
  useLayoutEffect(() => {
    const element = root.current, selected = element?.querySelector<HTMLElement>('[aria-selected="true"]');
    if (!element || !selected) return;
    const viewport = element.getBoundingClientRect(), row = selected.getBoundingClientRect();
    // Scroll only the menu, never its containing conversation or page.
    if (row.top < viewport.top) element.scrollTop -= viewport.top - row.top;
    else if (row.bottom > viewport.bottom) element.scrollTop += row.bottom - viewport.bottom;
  }, [active, rows]);
  return <div ref={root} id="composer-commands" role="listbox" aria-label={tx('commands:command-menu.commands')} className={css.menu}>
    {rows.length ? rows.map((command, index) => <Fragment key={command.id}>
      {(!index || section(command.id) !== section(rows[index - 1].id)) && <div className={css.sectionTitle} role="presentation">{tx(`commands:menu.${section(command.id)}`)}</div>}
      <button type="button" tabIndex={-1} role="option" id={`command-${command.id}`}
        aria-label={`${tx(`commands:menu.${command.id}`)} ${command.id}`} aria-describedby={`command-description-${command.id}`}
        className={css.menuRow} aria-selected={index === active} onMouseMove={() => highlight(index)}
        onMouseDown={event => event.preventDefault()} onClick={() => select(command.id)}>
        <span className={css.icon} aria-hidden="true"><CommandIcon id={command.id}/></span>
        <span className={css.name}>{tx(`commands:menu.${command.id}`)}</span>
        <small className={css.alias}>{command.id}</small>
        <span id={`command-description-${command.id}`} className={css.description}>{command.id === 'file' && fileDescription ? fileDescription : tx(`commands:menu.description.${command.id}`)}</span>
      </button>
    </Fragment>) : <p role="status">{tx('commands:command-menu.unsupported-command-edit-the-draft-it-will-not-be-sent-as-a-prom')}</p>}
  </div>;
}
