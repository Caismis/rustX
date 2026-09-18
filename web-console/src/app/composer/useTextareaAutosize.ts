import { useLayoutEffect, type RefObject } from 'react';

/** The textarea is the only draft scrollport. CSS owns the floor and cap;
 * measurement owns the natural height. No draft or runtime state lives here. */
export function useTextareaAutosize(ref: RefObject<HTMLTextAreaElement | null>, draft: string) {
  useLayoutEffect(() => {
    const input = ref.current;
    if (!input) return;
    const resize = () => {
      // Hidden interaction takeovers are measured when their width returns.
      if (!input.clientWidth) return;
      const scrollTop = input.scrollTop;
      input.style.height = '0px';
      input.style.height = `${input.scrollHeight}px`;
      input.scrollTop = scrollTop;
    };
    resize();
    // Width changes include sidebar/mobile reflow, not just window resizing.
    let width = input.getBoundingClientRect().width;
    let frame = 0;
    const observer = new ResizeObserver(() => {
      const nextWidth = input.getBoundingClientRect().width;
      if (width === nextWidth) return;
      width = nextWidth;
      // Resize outside observer delivery: changing height here would cause a
      // ResizeObserver loop warning when the containing layout also observes it.
      cancelAnimationFrame(frame);
      frame = requestAnimationFrame(resize);
    });
    observer.observe(input);
    return () => { observer.disconnect(); cancelAnimationFrame(frame); };
  }, [ref, draft]);
}
