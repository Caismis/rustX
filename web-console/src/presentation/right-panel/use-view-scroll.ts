import { useCallback, useLayoutEffect, useRef, type UIEvent } from 'react';

/** Restores only this document's scrollport. Conversation scrolling belongs to ChatViewport. */
export function useViewScroll<T extends HTMLElement = HTMLDivElement>(top: number, left: number, save: (top: number, left: number) => void, ready: unknown) {
  const scrollport = useRef<T | null>(null);
  const clampedRestore = useRef(false);
  const retained = useRef({ top, left, save });
  retained.current = { top, left, save };
  const restore = useCallback(() => {
    const node = scrollport.current;
    if (!node) return;
    node.scrollTop = retained.current.top;
    node.scrollLeft = retained.current.left;
    clampedRestore.current = node.scrollTop !== retained.current.top || node.scrollLeft !== retained.current.left;
  }, []);
  useLayoutEffect(restore, [ready, restore]);
  useLayoutEffect(() => {
    const node = scrollport.current; if (!node) return;
    const userIntent = () => { if (clampedRestore.current) retained.current.save(node.scrollTop, node.scrollLeft); clampedRestore.current = false; };
    node.addEventListener('wheel', userIntent, { passive: true }); node.addEventListener('pointerdown', userIntent); node.addEventListener('keydown', userIntent);
    return () => { node.removeEventListener('wheel', userIntent); node.removeEventListener('pointerdown', userIntent); node.removeEventListener('keydown', userIntent); };
  }, [ready]);
  const onPosition = (node: HTMLElement) => { if (!clampedRestore.current) retained.current.save(node.scrollTop, node.scrollLeft); };
  return {
    ref: scrollport,
    onScroll: (event: UIEvent<HTMLElement>) => onPosition(event.currentTarget), onPosition,
    restore,
  };
}
