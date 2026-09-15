/* Copyright (c) 2026 DeepSeek. MIT. Adapted Menu portal placement; see PROVENANCE.md. */
import { useLayoutEffect, useState, type RefObject } from 'react';
/** One geometry boundary shared by menus and popovers; no registry or domain owner. */
export function useAnchoredSurface(open: boolean, anchor: RefObject<HTMLElement | null>, panel: RefObject<HTMLElement | null>) {
  const [position, setPosition] = useState({ left: 0, top: 0 });
  useLayoutEffect(() => {
    if (!open) return;
    const place = () => {
      const trigger = anchor.current, surface = panel.current;
      if (!trigger || !surface) return;
      const rect = trigger.getBoundingClientRect();
      const gap = 8;
      const left = Math.max(gap, Math.min(rect.left, innerWidth - surface.offsetWidth - gap));
      const below = rect.bottom + 4;
      const top = Math.max(gap, Math.min(below + surface.offsetHeight > innerHeight - gap ? rect.top - surface.offsetHeight - 4 : below, innerHeight - surface.offsetHeight - gap));
      setPosition({ left, top });
    };
    place();
    window.addEventListener('resize', place);
    window.addEventListener('scroll', place, true);
    return () => { window.removeEventListener('resize', place); window.removeEventListener('scroll', place, true); };
  }, [open, anchor, panel]);
  return position;
}
