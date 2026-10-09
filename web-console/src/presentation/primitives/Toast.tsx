/* Copyright (c) 2026 DeepSeek. MIT. Adapted from Harness Toast; see PROVENANCE.md. */
import { useEffect, useLayoutEffect, useMemo, useRef, type ReactNode } from 'react';
import { createPortal } from 'react-dom';
import { autoUpdate, offset, shift, useFloating } from '@floating-ui/react-dom';
import css from './Toast.module.css';

/** Non-blocking Harness announcement, centered over its owner's composer.
 * Key each new announcement separately, including repeated identical errors. */
export function Toast({ text, icon, anchor, onDone }: {
  text: string; icon: ReactNode; anchor: HTMLElement | null; onDone: () => void;
}) {
  const done = useRef(onDone);
  useLayoutEffect(() => { done.current = onDone; }, [onDone]);
  useEffect(() => {
    const timer = setTimeout(() => done.current(), 4000);
    return () => clearTimeout(timer);
  }, []);
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
  return createPortal(<div ref={refs.setFloating} className={css.toast} role="alert" data-toast
    style={{ ...floatingStyles, visibility: isPositioned ? undefined : 'hidden' }}>
    <span className={css.icon} aria-hidden>{icon}</span><span className={css.text}>{text}</span>
  </div>, document.body);
}
