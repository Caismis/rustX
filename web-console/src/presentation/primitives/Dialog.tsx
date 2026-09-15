/* Copyright (c) 2026 DeepSeek. MIT. Rewritten from ui-primitives/Modal.tsx; see PROVENANCE.md. */
import { useId, useLayoutEffect, useRef, type ReactNode } from 'react';
import { Button } from './Button';
import css from './Modal.module.css';
/** Native modal supplies top-layer isolation, focus containment and restoration. */
export function Dialog({ open, onClose, title, children, footer }: {
  open: boolean; onClose: () => void; title: string; children: ReactNode; footer?: ReactNode;
}) {
  const ref = useRef<HTMLDialogElement>(null);
  const titleId = useId();
  useLayoutEffect(() => {
    const dialog = ref.current!;
    if (!open) return;
    const previous = document.activeElement as HTMLElement | null;
    dialog.showModal();
    return () => { dialog.close(); if (previous?.isConnected) previous.focus(); };
  }, [open]);
  return <dialog ref={ref} className={css.dialog} aria-labelledby={titleId} onKeyDown={event => {
      if (event.key !== 'Tab') return;
      const controls = Array.from(event.currentTarget.querySelectorAll<HTMLElement>('button:not(:disabled), input:not(:disabled), select:not(:disabled), textarea:not(:disabled), a[href], [tabindex]'))
        .filter(element => element.tabIndex >= 0 && element.getClientRects().length > 0);
      const first = controls[0], last = controls.at(-1);
      if (event.shiftKey && document.activeElement === first) { event.preventDefault(); last?.focus(); }
      else if (!event.shiftKey && document.activeElement === last) { event.preventDefault(); first?.focus(); }
    }} onCancel={event => { event.preventDefault(); onClose(); }}
    onClick={event => { if (event.target === event.currentTarget) {
      const rect = event.currentTarget.getBoundingClientRect();
      if (event.clientX < rect.left || event.clientX > rect.right || event.clientY < rect.top || event.clientY > rect.bottom) onClose();
    } }}>
    <div className={css.header}><h2 id={titleId} className={css.title}>{title}</h2><Button aria-label="Close dialog" onClick={onClose}>×</Button></div>
    <div className={css.body}>{children}</div>{footer && <div className={css.footer}>{footer}</div>}
  </dialog>;
}
