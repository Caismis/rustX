/* Copyright (c) 2026 DeepSeek. MIT. Adapted from Harness Toast; see PROVENANCE.md. */
import { Fragment, useEffect, useLayoutEffect, useMemo, useRef, type CSSProperties, type ReactNode } from 'react';
import { createPortal } from 'react-dom';
import { autoUpdate, offset, shift, useFloating } from '@floating-ui/react-dom';
import css from './Toast.module.css';
import { IconCheckOutline16 } from './icons';

/** Non-blocking Harness announcement, centered over its owner's composer.
 * Key each new announcement separately, including repeated identical errors. */
export function Toast({ text, icon, anchor, label, tone, holdMs = 3000, actions, onDone }: {
  text: string; label?: string; icon?: ReactNode; anchor?: HTMLElement | null; tone?: 'success'; holdMs?: number;
  actions?: readonly { label: string; onClick: () => void }[]; onDone: () => void;
}) {
  const done = useRef(onDone);
  useLayoutEffect(() => { done.current = onDone; }, [onDone]);
  useEffect(() => {
    const timer = setTimeout(() => done.current(), holdMs + 1000);
    return () => clearTimeout(timer);
  }, [holdMs]);
  const reference = useMemo(() => ({
    contextElement: anchor ?? undefined,
    getBoundingClientRect: () => {
      const rect = anchor?.getBoundingClientRect();
      return DOMRect.fromRect({ x: rect ? rect.left + rect.width / 2 : window.innerWidth / 2, y: 32 });
    },
  }), [anchor]);
  const { refs, floatingStyles, isPositioned } = useFloating({
    placement: 'bottom', strategy: 'fixed', transform: false,
    elements: { reference }, whileElementsMounted: autoUpdate,
    middleware: [offset(8), shift({ padding: 24 })],
  });
  return createPortal(<div ref={refs.setFloating} className={css.toast} role={tone === 'success' ? 'status' : 'alert'} aria-label={label} data-toast
    style={{ ...floatingStyles, '--dsh-toast-hold': `${holdMs}ms`, visibility: isPositioned ? undefined : 'hidden' } as CSSProperties}>
    {tone === 'success' ? <span className={`${css.icon} ${css.success}`} aria-hidden><IconCheckOutline16/></span> : icon && <span className={css.icon} aria-hidden>{icon}</span>}
    <span className={css.text}>{text}{actions?.map(action => <Fragment key={action.label}>{' '}<button type="button" className={css.action} onClick={action.onClick}>{action.label}</button></Fragment>)}</span>
  </div>, document.body);
}
