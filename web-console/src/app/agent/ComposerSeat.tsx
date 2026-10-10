import { useLayoutEffect, useRef, type ReactNode } from 'react';
import css from '../../presentation/agent/Conversation.module.css';

/** Shared sticky seat: every conversation measures its own visible composer. */
export function ComposerSeat({ children }: { children: ReactNode }) {
  const seat = useRef<HTMLDivElement>(null);
  useLayoutEffect(() => {
    const element = seat.current!, body = element.closest<HTMLElement>('.conversation-panel');
    if (!body) return;
    const measure = () => {
      const height = element.getBoundingClientRect().height;
      // Resident hidden conversations retain their drafts and scroll memory;
      // only the visible seat may position floating conversation controls.
      if (height > 0) body.style.setProperty('--dsh-composer-height', `${height}px`);
    };
    measure();
    const observer = new ResizeObserver(measure);
    observer.observe(element);
    return () => observer.disconnect();
  }, []);
  return <div ref={seat} className={css.composerSeat} data-composer-seat="">{children}</div>;
}
