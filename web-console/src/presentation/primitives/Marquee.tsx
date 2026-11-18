import { forwardRef, useCallback, useEffect, useMemo, useRef } from 'react';
import type { HTMLAttributes, PointerEvent, RefObject } from 'react';
import clsx from 'clsx';
import css from './Marquee.module.css';

/* Overflow this small hides no meaningful tail; travelling for it reads as an
   accidental jitter, so the text stays put. */
const MIN_REVEAL_PX = 8;

/* Travel speed: slow enough to read the text as it passes. */
const PX_PER_MS = 0.03;

/** Pointer handlers of the row whose hover marquees one text; spread them onto the row. */
export interface HoverMarquee {
  /** Start the travel; touch pointers and text that fits do nothing. */
  readonly onPointerEnter: (event: PointerEvent) => void;
  /** Return the text to its start in one step, unless the marquee is held. */
  readonly onPointerLeave: () => void;
}

/** Options of {@link useHoverMarquee}. */
export interface HoverMarqueeOptions {
  /** Marquee the text while true, with or without the pointer on the row, such as while the row's menu is open. */
  readonly hold?: boolean | undefined;
}

function travel(text: HTMLElement): void {
  if (text.dataset.marquee !== undefined) return;
  const range = text.scrollWidth - text.clientWidth;
  if (range <= MIN_REVEAL_PX) return;
  text.style.setProperty('--dsh-marquee-range', `${range}px`);
  text.style.setProperty('--dsh-marquee-duration', `${Math.round(range / PX_PER_MS)}ms`);
  text.dataset.marquee = '';
}

function rest(text: HTMLElement): void {
  delete text.dataset.marquee;
  text.style.removeProperty('--dsh-marquee-range');
  text.style.removeProperty('--dsh-marquee-duration');
}

/**
 * Marquee one-line text wider than its cell while the pointer is on its row:
 * the text travels at 30px/s until its far edge is in view, then rests there
 * under the pointer. Overflow of at most 8px stays put.
 */
export function useHoverMarquee(text: RefObject<HTMLElement | null>, { hold = false }: HoverMarqueeOptions = {}): HoverMarquee {
  const hovered = useRef(false);
  const held = useRef(hold);
  const sync = useCallback((): void => {
    const element = text.current;
    if (element === null) return;
    if (hovered.current || held.current) travel(element);
    else rest(element);
  }, [text]);
  useEffect(() => {
    held.current = hold;
    sync();
  }, [hold, sync]);
  return useMemo(() => ({
    onPointerEnter: (event: PointerEvent): void => {
      if (event.pointerType === 'touch') return;
      hovered.current = true;
      sync();
    },
    onPointerLeave: (): void => {
      hovered.current = false;
      sync();
    },
  }), [sync]);
}

/** Props of single-line text that a {@link useHoverMarquee} can marquee. */
export type MarqueeTextProps = HTMLAttributes<HTMLSpanElement>;

/**
 * Render single-line text that clips with an ellipsis at rest and, while a
 * {@link useHoverMarquee} marquees it, drops the ellipsis and fades whichever
 * edge still hides text.
 */
export const MarqueeText = forwardRef<HTMLSpanElement, MarqueeTextProps>(function MarqueeText({ className, children, ...attributes }, ref) {
  return (
    <span {...attributes} ref={ref} className={clsx(css.text, className)}>
      <span className={css.travel}>{children}</span>
    </span>
  );
});
