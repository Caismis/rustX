import { useTranslation } from '../../locale/react';
/* Copyright (c) 2026 DeepSeek. MIT. Rewritten from ui-input-trigger/MenuView; see PROVENANCE.md. */
import { useEffect, useRef } from 'react';
import type { CommandDefinition, CommandId } from './registry';
import { IconPaperclipOutline16, IconDataOutline16, IconCompactOutline16, IconNewChatOutline16, IconBranchOutline16, IconForkOutline16, IconGoalOutline16, IconCodeOutline16 } from '../../presentation/primitives/icons';
import css from './Commands.module.css';
export type ComposerAction = CommandId | 'attach';
export type ComposerActionRow = Pick<CommandDefinition, 'labelKey'> & { id: ComposerAction };
const icons = { attach: IconPaperclipOutline16, model: IconDataOutline16, compact: IconCompactOutline16, new: IconNewChatOutline16, fork: IconForkOutline16, branch: IconBranchOutline16, goal: IconGoalOutline16, tools: IconCodeOutline16 };
export function CommandMenu({ rows, active, select, highlight }: {
  rows: readonly ComposerActionRow[]; active: number; select: (id: ComposerAction) => void; highlight: (index: number) => void;
}) {
  const tx = useTranslation();
  const root = useRef<HTMLDivElement>(null);
  useEffect(() => { root.current?.querySelector('[aria-selected="true"]')?.scrollIntoView?.({ block: 'nearest' }); }, [active]);
  return <div ref={root} id="composer-commands" role="listbox" aria-label={tx('commands:command-menu.commands')} className={css.menu}>
    {rows.length ? rows.map((command, index) => { const Icon = icons[command.id]; return <button type="button" tabIndex={-1} role="option" id={`command-${command.id}`} key={command.id}
      className={css.row} aria-selected={index === active} onMouseEnter={() => highlight(index)}
      onMouseDown={event => { event.preventDefault(); select(command.id); }}>
      <span className={css.label}><span aria-hidden="true"><Icon /></span>{tx(command.labelKey)}</span>{command.id !== 'attach' && <small>/{command.id}</small>}
    </button> }) : <p role="status">{tx('commands:command-menu.unsupported-command-edit-the-draft-it-will-not-be-sent-as-a-prom')}</p>}
  </div>;
}
