/* Copyright (c) 2026 DeepSeek. MIT. Source-derived; see PROVENANCE.md. */
// Adapted from DeepSeek Harness ui-chat TurnNavigator: a fixed-pitch virtual
// rail of every known turn with hover/focus previews. rustX supplies the
// native Attempt outline; an unloaded mark pages its native outline page in
// before navigating.
import { memo, useCallback, useEffect, useId, useLayoutEffect, useMemo, useRef, useState, type CSSProperties } from 'react';
import type { ConversationTurn } from '../../../../protocol/app-server/v39';
import type { AppServerClient } from '../../client/app-server';
import { shallowEqual, useClientSelector } from '../../client/selectors';
import { turnKey } from '../../client/transcript';
import { useTranslation } from '../../locale/react';
import css from './TurnNavigator.module.css';
import { currentTurnLocation, turnRail, turnRailRange, TURN_SPACING_PX, RAIL_INSET_PX, type TurnRailItem } from './turn-rail-items';
import { HISTORY_PAGE_SIZE } from '../../client/transcript';

/** Fade band the mask reserves at a scrollable end. */
const FADE_PX = 24;

function preferredScrollBehavior(): 'auto' | 'smooth' {
  return typeof matchMedia === 'function' && matchMedia('(prefers-reduced-motion: reduce)').matches ? 'auto' : 'smooth';
}

const TurnMark = memo(function TurnMark({ item, index, start, label, active, busy, unavailable, previewId, onNavigate, onPreview, onFocusChange }: {
  item: TurnRailItem; index: number; start: number; label: string; active: boolean; busy: boolean; unavailable: boolean; previewId: string | undefined;
  onNavigate: (item: TurnRailItem) => void;
  onPreview: (key: string | null) => void; onFocusChange: (key: string | null) => void;
}) {
  const classes = [css.mark];
  if (!item.turn) classes.push(css.markUnloaded);
  if (active) classes.push(css.markActive);
  else if (previewId !== undefined) classes.push(css.markPreview);
  if (busy) classes.push(css.markBusy);
  return <button data-index={index} type="button" className={classes.join(' ')} style={{ transform: `translateY(${start}px)` }}
    data-turn-id={item.id} data-turn-ordinal={item.ordinal || undefined}
    aria-label={label} aria-current={active ? 'true' : undefined} aria-busy={busy ? 'true' : undefined}
    aria-disabled={unavailable ? 'true' : undefined} aria-describedby={previewId}
    onPointerMove={() => { onPreview(item.key); }} onClick={() => { if (!unavailable) onNavigate(item); }}
    onFocus={() => { onFocusChange(item.key); }} onBlur={() => { onFocusChange(null); }} />;
});

/** Every turn of one native outline: loaded marks scroll to their exact
 * native anchor, unloaded marks read their outline page first. Overflow
 * scrolls inside the frame with gradient fades at each scrollable end; the
 * active mark centers only outside the fade-free band while the pointer is
 * elsewhere. Previews follow pointer movement or focus, never scrolling. */
export function TurnNavigator({ client, sessionId, onNavigate, active }: { client: AppServerClient; sessionId: string; onNavigate: (turn: ConversationTurn | number) => void; active?: string | null }) {
  const tx = useTranslation(), previewId = useId();
  const [previewKey, setPreviewKey] = useState<string | null>(null);
  const [focusedKey, setFocusedKey] = useState<string | null>(null);
  const view = useClientSelector(client, state => {
    const view = state.views[sessionId];
    return { target: view?.target, attachment: view?.attachment, outline: view?.turnOutline, navigation: view?.turnNavigation,
      attempt: view?.snapshot?.attempt?.attempt_id, phase: view?.snapshot?.attempt?.phase.type,
      conversation: view?.snapshot?.conversation_id, running: !!view?.snapshot?.attempt && view.snapshot.attempt.phase.type !== 'settled',
      location: currentTurnLocation(view?.snapshot), generation: state.generation };
  }, shallowEqual);
  useEffect(() => {
    if (view.attachment === 'attached') void client.refreshTurns(sessionId);
  }, [client, sessionId, view.target, view.attachment, view.attempt, view.phase, view.generation]);
  const currentId = useMemo(() => view.running && view.conversation && view.attempt ? { conversation_id: view.conversation, attempt_id: view.attempt } : undefined,
    [view.running, view.conversation, view.attempt]);
  const page = view.outline?.page;
  const items = useMemo(() => turnRail(page, currentId, view.location), [page, currentId, view.location]);
  const previewRead = useRef<string | null>(null);
  useEffect(() => { previewRead.current = null; }, [view.target, view.generation]);
  useEffect(() => {
    if (!previewKey) { previewRead.current = null; return; }
    const index = items.indexOfKey(previewKey), item = index === undefined ? undefined : items.item(index);
    if (!item || item.turn || view.attachment !== 'attached' || view.outline?.loading || view.navigation?.pending) return;
    const key = `${view.generation}:${view.conversation}:${item.key}`;
    if (previewRead.current === key) return;
    const timer = setTimeout(() => {
      previewRead.current = key;
      void client.readTurns(sessionId, Math.floor((item.ordinal - 1) / HISTORY_PAGE_SIZE) * HISTORY_PAGE_SIZE);
    }, 150);
    return () => clearTimeout(timer);
  }, [client, sessionId, items, previewKey, view.generation, view.conversation, view.attachment, view.outline?.loading, view.navigation?.pending]);
  const missingLocation = !!page && !!view.location && BigInt(page.cut.transcript) < BigInt(view.location);
  useEffect(() => { if (missingLocation && !view.outline?.loading && !view.outline?.error && view.attachment === 'attached') void client.refreshTurns(sessionId); },
    [client, sessionId, missingLocation, view.outline?.loading, view.outline?.error, view.attachment, page?.offset]);
  const attached = view.attachment === 'attached';
  const isActive = (item: TurnRailItem) => !!item.id && (active !== undefined ? active === `turn:${item.id}` : !!currentId && turnKey(currentId) === item.id);
  // The owning viewport starts its gesture immediately, before outline I/O.
  const navigate = (item: TurnRailItem) => onNavigate(item.turn ?? item.ordinal);
  const error = view.navigation?.error ?? view.outline?.error;
  // No rail and nothing to recover: no slot layer over the reading surface.
  if (items.count < 2 && !error) return null;
  return <div className={css.slot} data-turn-navigator>
    <TurnRail navigationLabel={tx('agent:reading.turn-navigation')} loading={!!view.outline?.loading} items={items} activeId={active !== undefined ? active?.slice(5) : currentId && turnKey(currentId)} isActive={isActive}
      isBusy={item => view.navigation?.pending === item.key || !!item.id && view.navigation?.pending === item.id} attached={attached} onNavigate={navigate}
      previewKey={previewKey} setPreviewKey={setPreviewKey} focusedKey={focusedKey} setFocusedKey={setFocusedKey} previewId={previewId}
      label={item => item.ordinal === 0 ? tx('agent:reading.current-turn') : tx(item.turn ? 'agent:reading.jump-turn' : 'agent:reading.jump-load-turn', { n: item.ordinal })}
      title={item => item.turn?.prompt || (item.ordinal === 0 ? tx('agent:reading.current-turn') : tx('agent:reading.turn', { n: item.ordinal }))} />
    {error && <div className={css.feedback} role="alert"><p>{error}</p><button type="button" onClick={() => { client.invalidateReading(sessionId); void client.refreshTurns(sessionId); }}>{tx('agent:reading.reload-turns')}</button></div>}
  </div>;
}

function TurnRail({ navigationLabel, loading, items, activeId, isActive, isBusy, attached, onNavigate, previewKey, setPreviewKey, focusedKey, setFocusedKey, previewId, label, title }: {
  navigationLabel: string; loading: boolean; items: ReturnType<typeof turnRail>; activeId?: string | null; isActive: (item: TurnRailItem) => boolean; isBusy: (item: TurnRailItem) => boolean; attached: boolean;
  onNavigate: (item: TurnRailItem) => void; previewKey: string | null; setPreviewKey: (key: string | null) => void;
  focusedKey: string | null; setFocusedKey: (key: string | null) => void; previewId: string;
  label: (item: TurnRailItem) => string; title: (item: TurnRailItem) => string;
}) {
  const scrollerRef = useRef<HTMLDivElement | null>(null);
  const drag = useRef<{ pointer: number; start: number; moved: boolean; index?: number } | null>(null);
  const suppressClick = useRef(false);
  const initialization = useRef({ placed: false, index: 0, follow: null as { index: number; count: number; height: number } | null });
  const pointerInsideRef = useRef(false);
  const [geometry, setGeometry] = useState({ top: 0, height: 0 });
  const activeAt = items.indexOfTurn(activeId ?? undefined);
  useLayoutEffect(() => { initialization.current.index = activeAt ?? items.count - 1; }, [activeAt, items.count]);
  const totalSize = items.count * TURN_SPACING_PX + 2 * RAIL_INSET_PX - TURN_SPACING_PX;
  const focusedIndex = items.indexOfKey(focusedKey), previewIndex = items.indexOfKey(previewKey);
  const onFocusChange = useCallback((key: string | null) => { setFocusedKey(key); setPreviewKey(key); }, [setFocusedKey, setPreviewKey]);
  useLayoutEffect(() => {
    const element = scrollerRef.current;
    if (!element || typeof ResizeObserver === 'undefined') return;
    const observer = new ResizeObserver(([entry]) => {
      if (!entry) return;
      const height = Math.round(entry.borderBoxSize?.[0]?.blockSize ?? entry.contentRect.height);
      const initial = initialization.current;
      if (!initial.placed && height > 0) {
        const max = Math.max(0, totalSize - height), center = initial.index * TURN_SPACING_PX + RAIL_INSET_PX;
        initial.placed = true;
        initial.follow = { index: initial.index, count: items.count, height };
        element.scrollTop = Math.max(0, Math.min(max, center - height / 2));
      }
      setGeometry({ top: element.scrollTop, height });
    });
    observer.observe(element, { box: 'border-box' });
    return () => observer.disconnect();
  }, [items.count, totalSize]);
  const scrollTop = geometry.top, viewHeight = geometry.height;
  const virtualItems = turnRailRange(items.count, scrollTop, viewHeight, focusedIndex).map(index => ({ index, key: items.item(index)!.key, start: index * TURN_SPACING_PX + RAIL_INSET_PX - TURN_SPACING_PX / 2, size: TURN_SPACING_PX }));
  useEffect(() => {
    if (viewHeight <= 0) { initialization.current.follow = null; return; }
    if (activeAt === undefined || pointerInsideRef.current) return;
    const previous = initialization.current.follow;
    if (previous?.index === activeAt && previous.count === items.count && previous.height === viewHeight) return;
    initialization.current.follow = { index: activeAt, count: items.count, height: viewHeight };
    const element = scrollerRef.current, center = activeAt * TURN_SPACING_PX + RAIL_INSET_PX;
    if (!element || center >= element.scrollTop + FADE_PX && center <= element.scrollTop + viewHeight - FADE_PX) return;
    const top = Math.max(0, Math.min(Math.max(0, totalSize - viewHeight), center - viewHeight / 2));
    if (top === element.scrollTop) return;
    element.scrollTo({ top,
      behavior: previous?.count === items.count && previous.height === viewHeight ? preferredScrollBehavior() : 'instant' });
  }, [activeAt, items.count, viewHeight, totalSize]);
  if (items.count < 2) return null;
  const preview = previewIndex === undefined ? undefined : items.item(previewIndex);
  const previewPosition = virtualItems.find(item => item.index === previewIndex);
  const scroller = [css.scroller];
  if (scrollTop > 1) scroller.push(css.fadeTop);
  if (scrollTop < totalSize - viewHeight - 1) scroller.push(css.fadeBottom);
  return <nav className={css.frame} aria-label={navigationLabel} aria-busy={loading || undefined}
    onPointerEnter={() => { pointerInsideRef.current = true; }}
    onPointerLeave={() => { pointerInsideRef.current = false; setPreviewKey(null); }}>
    <div ref={scrollerRef} className={scroller.join(' ')} onScroll={event => { const top = event.currentTarget.scrollTop; setGeometry(value => ({ ...value, top })); }}
      onPointerDown={event => {
        if (event.button !== 0) return;
        suppressClick.current = false;
        drag.current = { pointer: event.pointerId, start: event.clientY, moved: false };
      }}
      onPointerMove={event => {
        const gesture = drag.current;
        if (!gesture || gesture.pointer !== event.pointerId || !gesture.moved && Math.abs(event.clientY - gesture.start) < 4) return;
        gesture.moved = true;
        event.currentTarget.setPointerCapture(event.pointerId);
        const rect = event.currentTarget.getBoundingClientRect();
        if (event.clientY < rect.top + 16) event.currentTarget.scrollTop -= TURN_SPACING_PX;
        if (event.clientY > rect.bottom - 16) event.currentTarget.scrollTop += TURN_SPACING_PX;
        const index = Math.max(0, Math.min(items.count - 1, Math.round((event.clientY - rect.top + event.currentTarget.scrollTop - RAIL_INSET_PX) / TURN_SPACING_PX)));
        gesture.index = index;
        const top = event.currentTarget.scrollTop;
        setGeometry(value => ({ ...value, top }));
        setPreviewKey(items.item(index)?.key ?? null);
      }}
      onPointerUp={event => {
        const gesture = drag.current;
        drag.current = null;
        if (event.currentTarget.hasPointerCapture(event.pointerId)) event.currentTarget.releasePointerCapture(event.pointerId);
        if (!gesture?.moved) return;
        suppressClick.current = true;
        const item = gesture.index === undefined ? undefined : items.item(gesture.index);
        if (attached && item && (!item.turn || item.turn.cursor != null)) onNavigate(item);
        setPreviewKey(null);
      }}
      onPointerCancel={() => { drag.current = null; setPreviewKey(null); }}
      onClickCapture={event => {
        if (suppressClick.current) { event.preventDefault(); event.stopPropagation(); suppressClick.current = false; }
      }}>
      <div className={css.marks} style={{ height: totalSize }}>
        {virtualItems.map(({ index, key, start }) => {
          const item = items.item(index);
          if (item === undefined) return null;
          return <TurnMark key={key} item={item} index={index} start={start} label={label(item)} active={isActive(item)} busy={isBusy(item)}
              unavailable={!!item.turn && (item.turn.cursor == null || !attached) || !item.turn && !attached}
              previewId={item.key === previewKey ? previewId : undefined}
              onNavigate={onNavigate} onPreview={setPreviewKey} onFocusChange={onFocusChange} />;
        })}
      </div>
    </div>
    {preview !== undefined && previewPosition !== undefined && <div id={previewId} role="tooltip" className={css.preview}
      style={{ '--turn-preview-center': `${previewPosition.start + previewPosition.size / 2 - scrollTop}px` } as CSSProperties}>
      <div className={css.previewPrompt}>{title(preview)}</div>
      {preview.turn?.response ? <div className={css.previewResponse}>{preview.turn.response}</div> : null}
    </div>}
  </nav>;
}
