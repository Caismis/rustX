/* Copyright (c) 2026 DeepSeek. MIT. Width interaction patterns adapted; see PROVENANCE.md. */
import { useLayoutEffect, useRef, useState } from 'react';
import { useTranslation } from '../../locale/react';
import { displayedWidth, readWidthPreference, saveWidthPreference, WIDTH_EDGE, WIDTH_MIN } from './conversation-width';
import css from '../../presentation/agent/Conversation.module.css';

/** CSS publication is local to this column; dragging never invalidates Session. */
export function ConversationWidthControls({ active }: { active: boolean }) {
  const tx = useTranslation(), marker = useRef<HTMLSpanElement>(null);
  const [initialPreference] = useState(readWidthPreference);
  const preference = useRef<number | undefined>(initialPreference);
  const column = useRef(0), frame = useRef<number | undefined>(undefined);
  const drag = useRef<{ pointer: number; origin: number; latest: number; base: number; side: 'left' | 'right'; element: HTMLElement } | undefined>(undefined);
  const [geometry, setGeometry] = useState({ column: 0, width: 0 });
  const publish = (intent = preference.current) => {
    const body = marker.current?.parentElement;
    if (!body) return;
    const width = displayedWidth(column.current, intent);
    body.style.setProperty('--dsh-conversation-column-width', `${column.current}px`);
    body.style.setProperty('--dsh-chat-content-width', `${width}px`);
    setGeometry(previous => previous.column === column.current && previous.width === width ? previous : { column: column.current, width });
    return width;
  };
  const cancelFrame = () => { if (frame.current !== undefined) cancelAnimationFrame(frame.current); frame.current = undefined; };
  const candidate = () => {
    const value = drag.current!;
    return value.base + (value.latest - value.origin) * (value.side === 'right' ? 2 : -2);
  };
  const end = (commit: boolean) => {
    const value = drag.current;
    if (!value) return;
    cancelFrame();
    if (commit && Math.abs(value.latest - value.origin) >= 2) {
      const width = displayedWidth(column.current, candidate());
      preference.current = width; saveWidthPreference(width);
    }
    drag.current = undefined;
    value.element.removeAttribute('data-dragging');
    if (value.element.hasPointerCapture?.(value.pointer)) value.element.releasePointerCapture(value.pointer);
    publish();
  };
  useLayoutEffect(() => {
    const body = marker.current!.parentElement!;
    const measure = () => {
      column.current = body.getBoundingClientRect().width;
      if (column.current < WIDTH_MIN + WIDTH_EDGE) end(false);
      publish(drag.current ? candidate() : preference.current);
    };
    measure();
    const observer = new ResizeObserver(measure); observer.observe(body);
    return () => {
      observer.disconnect(); cancelFrame();
      const value = drag.current; drag.current = undefined;
      if (value?.element.hasPointerCapture?.(value.pointer)) value.element.releasePointerCapture(value.pointer);
    };
  }, []);
  useLayoutEffect(() => { if (!active) end(false); }, [active]);
  const usable = active && geometry.column >= WIDTH_MIN + WIDTH_EDGE && (geometry.column - geometry.width) / 2 >= 58;
  return <><span ref={marker} hidden data-conversation-width-owner/>{usable && (['left','right'] as const).map(side => <div key={side}
    className={css.widthHandle} data-side={side} data-width-handle={side} role="slider" tabIndex={0}
    aria-label={tx(side === 'left' ? 'agent:reading.width-left' : 'agent:reading.width-right')}
    aria-orientation="horizontal" aria-valuemin={WIDTH_MIN} aria-valuemax={Math.max(WIDTH_MIN, geometry.column-WIDTH_EDGE)} aria-valuenow={Math.round(geometry.width)}
    onPointerDown={event => {
      if (event.button !== 0 || drag.current) return;
      event.preventDefault(); event.currentTarget.focus();
      event.currentTarget.setPointerCapture(event.pointerId);
      drag.current = { pointer:event.pointerId, origin:event.clientX, latest:event.clientX, base:geometry.width, side, element:event.currentTarget };
      event.currentTarget.setAttribute('data-dragging','');
    }} onPointerMove={event => {
      const value=drag.current;
      if (!value || value.pointer !== event.pointerId || !event.currentTarget.hasPointerCapture(event.pointerId)) return;
      value.latest=event.clientX;
      event.currentTarget.style.setProperty('--dsh-width-handle-pointer-y', `${event.clientY-event.currentTarget.getBoundingClientRect().top}px`);
      frame.current ??= requestAnimationFrame(() => { frame.current=undefined; if(drag.current)publish(candidate()); });
    }} onPointerUp={event => { if(drag.current?.pointer===event.pointerId){drag.current.latest=event.clientX;end(true);} }}
    onPointerCancel={() => end(false)} onLostPointerCapture={() => end(false)}
    onKeyDown={event => {
      if(event.key==='Escape'){end(false);return;}
      if(!['ArrowLeft','ArrowRight','Home','End'].includes(event.key))return;
      event.preventDefault();end(false);
      const value=event.key==='Home'?WIDTH_MIN:event.key==='End'?geometry.column-WIDTH_EDGE:geometry.width+(event.key==='ArrowRight'?1:-1)*(event.shiftKey?64:16);
      const width=displayedWidth(column.current,value);preference.current=width;saveWidthPreference(width);publish();
    }}/>)}</>;
}
