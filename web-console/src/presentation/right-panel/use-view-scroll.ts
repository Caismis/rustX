import { useCallback, useLayoutEffect, useRef, type UIEvent } from 'react';

/** Restores only this document's scrollport. Conversation scrolling belongs to ChatViewport. */
export function useViewScroll(top: number, left: number, save: (top: number, left: number) => void, ready: unknown) {
  const scrollport = useRef<HTMLDivElement>(null);
  const retained = useRef({ top, left, save });
  retained.current = { top, left, save };
  const restore = useCallback(() => {
    const node = scrollport.current;
    if (!node) return;
    node.scrollTop = retained.current.top;
    node.scrollLeft = retained.current.left;
  }, []);
  useLayoutEffect(restore, [ready, restore]);
  return {
    ref: scrollport,
    onScroll: (event: UIEvent<HTMLDivElement>) => retained.current.save(event.currentTarget.scrollTop, event.currentTarget.scrollLeft),
    restore,
  };
}
