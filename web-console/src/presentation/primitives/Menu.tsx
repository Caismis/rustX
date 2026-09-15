/* Copyright (c) 2026 DeepSeek. MIT. Rewritten from ui-primitives/Menu.tsx; see PROVENANCE.md. */
import { useId, useLayoutEffect, useRef, useState } from 'react';
import { createPortal } from 'react-dom';
import { useAnchoredSurface } from './useAnchoredSurface';
import { Button } from './Button';
import css from './Menu.module.css';
export interface MenuItem { id: string; label: string; disabled?: boolean }
/** Flat action menu for product-owned actions; no registry or submenu platform. */
export function Menu({ label, items, onSelect, disabled = false }: {
  label: string; items: readonly MenuItem[]; onSelect: (id: string) => void; disabled?: boolean;
}) {
  const [open, setOpen] = useState(false);
  const root = useRef<HTMLDivElement>(null);
  const trigger = useRef<HTMLButtonElement>(null);
  const list = useRef<HTMLDivElement>(null);
  const initial = useRef<'first' | 'last'>('first');
  const id = useId();
  const position = useAnchoredSurface(open && !disabled, trigger, list);
  const buttons = () => Array.from(list.current?.querySelectorAll<HTMLButtonElement>('button:not(:disabled)') ?? []);
  useLayoutEffect(() => {
    if (!open || disabled) return;
    const rows = buttons(); (initial.current === 'last' ? rows.at(-1) : rows[0])?.focus();
    if (!rows.length) list.current?.focus();
    const outside = (event: PointerEvent) => {
      if (event.target instanceof Node && !root.current?.contains(event.target) && !list.current?.contains(event.target)) setOpen(false);
    };
    document.addEventListener('pointerdown', outside);
    return () => document.removeEventListener('pointerdown', outside);
  }, [open, disabled]);
  useLayoutEffect(() => { if (disabled) setOpen(false); }, [disabled]);
  const close = () => { setOpen(false); trigger.current?.focus(); };
  return <div ref={root} className={css.root} onBlur={event => {
    if (event.relatedTarget instanceof Node && !root.current?.contains(event.relatedTarget) && !list.current?.contains(event.relatedTarget)) setOpen(false);
  }}>
    <Button ref={trigger} disabled={disabled} aria-haspopup="menu" aria-expanded={open && !disabled} aria-controls={id}
      onClick={() => { initial.current = 'first'; setOpen(value => !value); }} onKeyDown={event => {
        if (event.key === 'ArrowDown' || event.key === 'ArrowUp') { event.preventDefault(); initial.current = event.key === 'ArrowUp' ? 'last' : 'first'; setOpen(true); }
      }}>{label}</Button>
    {open && !disabled && createPortal(<div ref={list} id={id} role="menu" aria-label={label} tabIndex={-1} className={css.list} style={position} onKeyDown={event => {
      const rows = buttons(); const index = rows.indexOf(document.activeElement as HTMLButtonElement);
      if (event.key === 'Escape') { event.preventDefault(); event.stopPropagation(); close(); }
      else if (event.key === 'Tab') { close(); }
      else if (['ArrowDown', 'ArrowUp', 'Home', 'End'].includes(event.key)) {
        event.preventDefault();
        const next = event.key === 'Home' ? 0 : event.key === 'End' ? rows.length - 1 : (index + (event.key === 'ArrowDown' ? 1 : -1) + rows.length) % rows.length;
        rows[next]?.focus();
      } else if (event.key.length === 1 && event.key !== ' ') {
        const ordered = [...rows.slice(index + 1), ...rows.slice(0, index + 1)];
        ordered.find(row => row.textContent?.toLowerCase().startsWith(event.key.toLowerCase()))?.focus();
      }
    }}>{items.map(item => <button key={item.id} type="button" role="menuitem" tabIndex={-1} disabled={item.disabled} className={css.item}
      onClick={() => { close(); onSelect(item.id); }}>{item.label}</button>)}</div>, root.current?.closest('dialog') ?? document.body)}
  </div>;
}
