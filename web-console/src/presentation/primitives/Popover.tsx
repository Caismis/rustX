/* Copyright (c) 2026 DeepSeek. MIT. Rewritten from Menu/HoverCard; see PROVENANCE.md. */
import { useLayoutEffect, useRef, useState, useId, type ReactNode } from 'react';
import { createPortal } from 'react-dom';
import { useAnchoredSurface } from './useAnchoredSurface';
import { Button } from './Button';
import css from './Menu.module.css';

/** Anchored presentation surface; no domain state. Native tab order stays in place. */
export function Popover({ label, children, disabled = false }: { label: string; children: ReactNode; disabled?: boolean }) {
  const [open, setOpen] = useState(false);
  const root = useRef<HTMLDivElement>(null);
  const trigger = useRef<HTMLButtonElement>(null);
  const panel = useRef<HTMLDivElement>(null);
  const id = useId();
  const position = useAnchoredSurface(open && !disabled, trigger, panel);
  useLayoutEffect(() => {
    if (!open || disabled) return;
    const current = panel.current!;
    (current.querySelector<HTMLElement>('input:not(:disabled), button:not(:disabled), [tabindex="0"]') ?? current).focus();
    const outside = (event: PointerEvent) => {
      if (event.target instanceof Node && !root.current?.contains(event.target) && !panel.current?.contains(event.target)) setOpen(false);
    };
    document.addEventListener('pointerdown', outside);
    return () => document.removeEventListener('pointerdown', outside);
  }, [open, disabled]);
  useLayoutEffect(() => { if (disabled) setOpen(false); }, [disabled]);
  return <div ref={root} className={css.root} onBlur={event => {
    if (event.relatedTarget instanceof Node && !root.current?.contains(event.relatedTarget) && !panel.current?.contains(event.relatedTarget)) setOpen(false);
  }} onKeyDown={event => {
    if (open && event.key === 'Escape') { event.preventDefault(); event.stopPropagation(); setOpen(false); trigger.current?.focus(); }
  }}>
    <Button ref={trigger} disabled={disabled} aria-haspopup="dialog" aria-expanded={open && !disabled} aria-controls={id} onClick={() => setOpen(value => !value)}>{label}</Button>
    {open && !disabled && createPortal(<div ref={panel} id={id} role="dialog" aria-label={label} tabIndex={-1} className={css.list} style={position}>{children}</div>, root.current?.closest('dialog') ?? document.body)}
  </div>;
}
