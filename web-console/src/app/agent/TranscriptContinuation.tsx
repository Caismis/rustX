import { useEffect, useRef } from 'react';
import css from '../../presentation/agent/Chat.module.css';

/** Continue the bounded native window at its trailing edge without pagination chrome. */
export function TranscriptContinuation({ cursor, loading, load }: { cursor: string; loading: boolean; load: () => void | Promise<void> }) {
  const marker = useRef<HTMLDivElement>(null), latest = useRef({ cursor, loading, load });
  latest.current = { cursor, loading, load };
  useEffect(() => {
    const node = marker.current!, viewport = node.closest<HTMLElement>('[data-conversation-scroll]') ?? node.closest<HTMLElement>('.conversation-scroll');
    if (!viewport) return;
    let active = true, frame: number | undefined, touchY: number | undefined, pointerY: number | undefined;
    let pending: { cursor: string } | undefined;
    const read = () => {
      const current = latest.current;
      if (!active || current.loading || pending?.cursor === current.cursor || node.closest('[hidden]')) return;
      const edge = node.getBoundingClientRect(), view = viewport.getBoundingClientRect();
      if (edge.top > view.bottom + 96 || edge.bottom < view.top) return;
      const operation = { cursor: current.cursor }; pending = operation;
      void Promise.resolve(current.load()).finally(() => { if (pending === operation) pending = undefined; });
    };
    // Sample after the input's default scrolling. Layout/anchor corrections
    // emit scroll events too, so those events must never start another page.
    const queue = () => { if (frame === undefined) frame = requestAnimationFrame(() => { frame = undefined; read(); }); };
    const editable = (target: EventTarget | null) => target instanceof Element && !!target.closest('input, textarea, select, [contenteditable="true"]');
    const wheel = (event: WheelEvent) => { if (event.deltaY > 0 && !editable(event.target)) queue(); };
    const key = (event: KeyboardEvent) => {
      if (editable(event.target)) return;
      if (event.key === ' ' && event.target instanceof Element && event.target.closest('button, a')) return;
      if (['ArrowDown', 'PageDown', 'End', ' '].includes(event.key) && !event.shiftKey && !event.ctrlKey && !event.metaKey && !event.altKey) queue();
    };
    const touchStart = (event: TouchEvent) => { touchY = event.touches.length === 1 && !editable(event.target) ? event.touches[0].clientY : undefined; };
    const touchMove = (event: TouchEvent) => {
      if (editable(event.target)) { touchY = undefined; return; }
      const next = event.touches.length === 1 ? event.touches[0].clientY : undefined;
      if (touchY !== undefined && next !== undefined && next < touchY) queue();
      touchY = next;
    };
    const pointerMove = (event: PointerEvent) => {
      if (event.pointerType !== 'touch' && event.buttons === 1 && pointerY !== undefined && event.clientY > pointerY && !editable(event.target)) queue();
      pointerY = event.clientY;
    };
    viewport.addEventListener('wheel', wheel, { passive: true });
    viewport.addEventListener('keydown', key);
    viewport.addEventListener('touchstart', touchStart, { passive: true });
    viewport.addEventListener('touchmove', touchMove, { passive: true });
    viewport.addEventListener('pointermove', pointerMove, { passive: true });
    return () => {
      active = false;
      if (frame !== undefined) cancelAnimationFrame(frame);
      viewport.removeEventListener('wheel', wheel);
      viewport.removeEventListener('keydown', key);
      viewport.removeEventListener('touchstart', touchStart);
      viewport.removeEventListener('touchmove', touchMove);
      viewport.removeEventListener('pointermove', pointerMove);
    };
  }, []);
  return <div ref={marker} className={css.continuation} data-chat-history-boundary={cursor} aria-hidden="true"/>;
}
