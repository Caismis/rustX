/* Copyright (c) 2026 DeepSeek. MIT. Adapted from Harness Toast; see PROVENANCE.md. */
// Toast: coss ui/Base UI anchored toast behind rustX's declarative notice
// contract. One adapter instance owns one manager so replacement keys and
// unmounting never share or replay an obsolete announcement.
import { Fragment, useEffect, useLayoutEffect, useMemo, useRef, useState, type ReactNode } from 'react';
import { AnchoredToastProvider, ToastPrimitive } from '@/components/ui/toast';

function Emitter({ text, icon, anchor, label, tone, foreground, holdMs, actions, onDone }: {
  text: string; label?: string; foreground?: boolean; icon?: ReactNode; anchor?: HTMLElement | null; tone?: 'success'; holdMs: number;
  actions?: readonly { label: string; onClick: () => void }[]; onDone: () => void;
}) {
  const { add, update } = ToastPrimitive.useToastManager();
  const id = useRef<string | undefined>(undefined);
  const done = useRef(onDone);
  useLayoutEffect(() => { done.current = onDone; }, [onDone]);

  const reference = useMemo(() => ({
    contextElement: anchor ?? undefined,
    getBoundingClientRect: () => {
      const rect = anchor?.getBoundingClientRect();
      return DOMRect.fromRect({ x: rect ? rect.left + rect.width / 2 : window.innerWidth / 2, y: 32 });
    },
  }), [anchor]) as unknown as Element;

  const title = useMemo(() => <>
    {text}
    {actions?.map(action => <Fragment key={action.label}>{' '}<button type="button" className="font-medium underline underline-offset-2" onClick={event => { event.stopPropagation(); action.onClick(); }}>{action.label}</button></Fragment>)}
  </>, [text, actions]);

  useEffect(() => {
    const options = {
      title,
      type: tone === 'success' ? 'success' : 'error',
      // Keep the visible card itself as the single assertive/polite node;
      // Base UI's high-priority duplicate would expose the same text twice.
      priority: 'low' as const,
      timeout: holdMs + 1000,
      onClose: () => done.current(),
      positionerProps: {
        anchor: reference,
        positionMethod: 'fixed' as const,
        side: 'bottom' as const,
        align: 'center' as const,
        sideOffset: 8,
        collisionPadding: 24,
      },
      data: {
        icon,
        rootProps: {
          role: tone === 'success' ? 'status' : 'alert',
          'aria-label': label,
          'data-toast': '',
          'data-foreground': foreground || undefined,
        },
      },
    };
    if (id.current === undefined) id.current = add(options);
    else {
      // Base UI treats the mere presence of `timeout` as a timer reset. A
      // parent translation/action refresh must update the content in place
      // while the original display lifetime continues.
      const { timeout: _timeout, ...content } = options;
      update(id.current, content);
    }
  }, [add, update, title, tone, holdMs, reference, icon, label, foreground]);

  useEffect(() => () => {
    if (id.current !== undefined) update(id.current, { timeout: 0, onClose: undefined });
  }, [update]);
  return null;
}

/** Non-blocking announcement, centered horizontally over its owner's composer.
 * Key each new announcement separately, including repeated identical errors. */
export function Toast({ text, icon, anchor, label, tone, portal, foreground, holdMs = 3000, actions, onDone }: {
  text: string; label?: string; portal?: HTMLElement; foreground?: boolean; icon?: ReactNode; anchor?: HTMLElement | null; tone?: 'success'; holdMs?: number;
  actions?: readonly { label: string; onClick: () => void }[]; onDone: () => void;
}) {
  const [toastManager] = useState(() => ToastPrimitive.createToastManager());
  return <AnchoredToastProvider toastManager={toastManager} timeout={holdMs + 1000} portalProps={{ container: portal }}>
    <Emitter text={text} icon={icon} anchor={anchor} label={label} tone={tone} foreground={foreground} holdMs={holdMs} actions={actions} onDone={onDone} />
  </AnchoredToastProvider>;
}
