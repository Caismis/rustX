/* Copyright (c) 2026 DeepSeek. MIT. Rewritten from ui-input-trigger/MenuView; see PROVENANCE.md. */
import { useEffect, useRef } from 'react';
import type { CommandDefinition, CommandId } from './registry';
import css from './Commands.module.css';
export function CommandMenu({ rows, active, select, highlight }: {
  rows: readonly CommandDefinition[]; active: number; select: (id: CommandId) => void; highlight: (index: number) => void;
}) {
  const root = useRef<HTMLDivElement>(null);
  useEffect(() => { root.current?.querySelector('[aria-selected="true"]')?.scrollIntoView?.({ block: 'nearest' }); }, [active]);
  return <div ref={root} id="composer-commands" role="listbox" aria-label="Commands" className={css.menu}>
    {rows.length ? rows.map((command, index) => <button type="button" tabIndex={-1} role="option" id={`command-${command.id}`} key={command.id}
      className={css.row} aria-selected={index === active} onMouseEnter={() => highlight(index)}
      onMouseDown={event => { event.preventDefault(); select(command.id); }}>
      <span>{command.label}</span><small>/{command.id}</small>
    </button>) : <p role="status">Unsupported command. Edit the draft; it will not be sent as a prompt.</p>}
  </div>;
}
