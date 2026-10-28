/* Copyright (c) 2026 DeepSeek. MIT. Adapted from ui-schedule/PickerPopover; see PROVENANCE.md. */
import { useEffect, useRef, type ReactNode, type RefObject } from 'react';
import { createPortal } from 'react-dom';
import { autoUpdate, flip, offset, shift, useFloating } from '@floating-ui/react-dom';
import clsx from 'clsx';
import css from './PickerPopover.module.css';
export function PickerPopover({ open, anchorRef, label, className, onClose, children }: {
  open: boolean; anchorRef: RefObject<HTMLElement | null>; label: string; className?: string;
  onClose: () => void; children: ReactNode;
}) {
  const panel = useRef<HTMLDivElement | null>(null);
  const { refs, floatingStyles } = useFloating({ open, placement: 'bottom-start', strategy: 'fixed',
    middleware: [offset(4), flip(), shift({ padding: 12 })], whileElementsMounted: autoUpdate });
  useEffect(() => { refs.setReference(anchorRef.current); }, [open, anchorRef, refs]);
  useEffect(() => {
    if (!open) return;
    const dismiss = (event: PointerEvent) => {
      if (!panel.current?.contains(event.target as Node) && !anchorRef.current?.contains(event.target as Node)) onClose();
    };
    document.addEventListener('pointerdown', dismiss);
    return () => document.removeEventListener('pointerdown', dismiss);
  }, [open, anchorRef, onClose]);
  if (!open) return null;
  return createPortal(<div ref={element => { panel.current = element; refs.setFloating(element); }}
    className={clsx(css.panel, className)} style={floatingStyles} role="dialog" aria-label={label}
    onClick={event => event.stopPropagation()} onKeyDown={event => { if (event.key === 'Escape') { event.preventDefault(); event.stopPropagation(); onClose(); anchorRef.current?.focus(); } }}>
    {children}
  </div>, document.body);
}
