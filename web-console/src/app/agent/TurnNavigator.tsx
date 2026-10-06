/* Copyright (c) 2026 DeepSeek. MIT. Source-derived; see PROVENANCE.md. */
// Adapted from DeepSeek Harness ui-chat TurnNavigator: a fixed-pitch virtual
// rail of every known turn with hover/focus previews. rustX supplies the
// native Attempt outline; an unloaded mark pages its native outline page in
// before navigating.
import { memo, useCallback, useEffect, useId, useLayoutEffect, useMemo, useRef, useState, type CSSProperties } from 'react';
import { defaultRangeExtractor, elementScroll, observeElementOffset, useVirtualizer, type Range } from '@tanstack/react-virtual';
import type { ConversationTurn } from '../../../../protocol/app-server/v35';
import type { AppServerClient } from '../../client/app-server';
import { shallowEqual, useClientSelector } from '../../client/selectors';
import { turnKey } from '../../client/transcript';
import { useTranslation } from '../../locale/react';
import css from './TurnNavigator.module.css';
import { currentTurnLocation, OUTLINE_PAGE, turnRailItems, type TurnRailItem } from './turn-rail-items';

/** Fixed pitch between neighbouring marks; overflow scrolls inside the frame. */
const TURN_SPACING_PX = 10;
/** Rail padding above the first mark and below the last one, per end. */
const RAIL_INSET_PX = 6;
/** Fade band the mask reserves at a scrollable end. */
const FADE_PX = 24;

function preferredScrollBehavior(): 'auto' | 'smooth' {
  return typeof matchMedia === 'function' && matchMedia('(prefers-reduced-motion: reduce)').matches ? 'auto' : 'smooth';
}

const TurnMark = memo(function TurnMark({ item, index, start, label, active, busy, unavailable, previewId, registerElement, onNavigate, onPreview, onFocusChange }: {
  item: TurnRailItem; index: number; start: number; label: string; active: boolean; busy: boolean; unavailable: boolean; previewId: string | undefined;
  registerElement: (element: HTMLButtonElement | null) => void; onNavigate: (item: TurnRailItem) => void;
  onPreview: (key: string | null) => void; onFocusChange: (key: string | null) => void;
}) {
  const classes = [css.mark];
  if (!item.turn) classes.push(css.markUnloaded);
  if (active) classes.push(css.markActive);
  else if (previewId !== undefined) classes.push(css.markPreview);
  if (busy) classes.push(css.markBusy);
  return <button ref={registerElement} data-index={index} type="button" className={classes.join(' ')} style={{ transform: `translateY(${start}px)` }}
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
export function TurnNavigator({ client, sessionId, onNavigate, active }: { client: AppServerClient; sessionId: string; onNavigate: (turn: ConversationTurn) => void; active?: string | null }) {
  const tx = useTranslation(), previewId = useId();
  const [previewKey, setPreviewKey] = useState<string | null>(null);
  const [focusedKey, setFocusedKey] = useState<string | null>(null);
  const [busyOrdinal, setBusyOrdinal] = useState<number | null>(null);
  const busyRef = useRef<number | null>(null);
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
  const items = useMemo(() => turnRailItems(page, currentId, view.location), [page, currentId, view.location]);
  const missingLocation = !!page && !!view.location && BigInt(page.cut.transcript) < BigInt(view.location);
  useEffect(() => { if (missingLocation && !view.outline?.loading && !view.outline?.error && view.attachment === 'attached') void client.refreshTurns(sessionId); },
    [client, sessionId, missingLocation, view.outline?.loading, view.outline?.error, view.attachment, page?.offset]);
  const attached = view.attachment === 'attached';
  const isActive = (item: TurnRailItem) => !!item.id && (active !== undefined ? active === `turn:${item.id}`
    : view.navigation?.active === item.id || !view.navigation?.active && !!currentId && turnKey(currentId) === item.id);
  const navigate = useCallback((item: TurnRailItem) => {
    if (item.turn) { onNavigate(item.turn); return; }
    const total = client.getSnapshot().views[sessionId]?.turnOutline?.page?.total ?? 0;
    const offset = Math.floor((item.ordinal - 1) / OUTLINE_PAGE) * OUTLINE_PAGE;
    busyRef.current = item.ordinal; setBusyOrdinal(item.ordinal);
    // The newest page restores native latest intent; any other selects that page.
    void (offset >= Math.floor((total - 1) / OUTLINE_PAGE) * OUTLINE_PAGE ? client.readTurns(sessionId) : client.readTurns(sessionId, offset)).then(() => {
      if (busyRef.current !== item.ordinal) return;
      busyRef.current = null; setBusyOrdinal(null);
      const turn = client.getSnapshot().views[sessionId]?.turnOutline?.page?.turns.find(turn => turn.ordinal === item.ordinal);
      if (turn?.cursor != null && client.getSnapshot().views[sessionId]?.attachment === 'attached') onNavigate(turn);
    });
  }, [client, sessionId, onNavigate]);
  const error = view.navigation?.error ?? view.outline?.error;
  // No rail and nothing to recover: no slot layer over the reading surface.
  if (items.length < 2 && !error) return null;
  return <div className={css.slot} data-turn-navigator>
    <TurnRail navigationLabel={tx('agent:reading.turn-navigation')} loading={!!view.outline?.loading} items={items} isActive={isActive}
      isBusy={item => item.ordinal === busyOrdinal || !!item.id && view.navigation?.pending === item.id} attached={attached} onNavigate={navigate}
      previewKey={previewKey} setPreviewKey={setPreviewKey} focusedKey={focusedKey} setFocusedKey={setFocusedKey} previewId={previewId}
      label={item => item.ordinal === 0 ? tx('agent:reading.current-turn') : tx(item.turn ? 'agent:reading.jump-turn' : 'agent:reading.jump-load-turn', { n: item.ordinal })}
      title={item => item.turn?.prompt || (item.ordinal === 0 ? tx('agent:reading.current-turn') : tx('agent:reading.turn', { n: item.ordinal }))} />
    {error && <div className={css.feedback} role="alert"><p>{error}</p><button type="button" onClick={() => { client.invalidateReading(sessionId); void client.refreshTurns(sessionId); }}>{tx('agent:reading.reload-turns')}</button></div>}
  </div>;
}

function TurnRail({ navigationLabel, loading, items, isActive, isBusy, attached, onNavigate, previewKey, setPreviewKey, focusedKey, setFocusedKey, previewId, label, title }: {
  navigationLabel: string; loading: boolean; items: readonly TurnRailItem[]; isActive: (item: TurnRailItem) => boolean; isBusy: (item: TurnRailItem) => boolean; attached: boolean;
  onNavigate: (item: TurnRailItem) => void; previewKey: string | null; setPreviewKey: (key: string | null) => void;
  focusedKey: string | null; setFocusedKey: (key: string | null) => void; previewId: string;
  label: (item: TurnRailItem) => string; title: (item: TurnRailItem) => string;
}) {
  const scrollerRef = useRef<HTMLDivElement | null>(null);
  const initialization = useRef({
    placed: false, index: 0,
    follow: null as { index: number; count: number; height: number } | null,
    publishOffset: null as ((offset: number, scrolling: boolean) => void) | null,
  });
  /** While the pointer works the rail, follow must not move it under the hand. */
  const pointerInsideRef = useRef(false);
  const indexes = useMemo(() => new Map(items.map((item, index) => [item.key, index])), [items]);
  const activeIndex = items.findIndex(isActive);
  const activeAt = activeIndex < 0 ? undefined : activeIndex;
  // Without an active turn the reader is at the latest output: place the newest mark.
  useLayoutEffect(() => { initialization.current.index = activeAt ?? items.length - 1; }, [activeAt, items.length]);
  const focusedIndex = focusedKey === null ? undefined : indexes.get(focusedKey);
  const previewIndex = previewKey === null ? undefined : indexes.get(previewKey);
  const onFocusChange = useCallback((key: string | null) => { setFocusedKey(key); setPreviewKey(key); }, [setFocusedKey, setPreviewKey]);
  const virtualizer = useVirtualizer<HTMLDivElement, HTMLButtonElement>({
    count: items.length,
    enabled: items.length >= 2,
    useScrollendEvent: true,
    getScrollElement: useCallback(() => scrollerRef.current, []),
    getItemKey: useCallback((index: number) => items[index]?.key ?? index, [items]),
    estimateSize: () => TURN_SPACING_PX,
    measureElement: () => TURN_SPACING_PX,
    initialRect: { width: 0, height: 0 },
    initialOffset: 0,
    scrollToFn: (offset, options, instance) => {
      if (initialization.current.placed) elementScroll(offset, options, instance);
    },
    observeElementOffset: (instance, notify) => {
      initialization.current.publishOffset = notify;
      const dispose = observeElementOffset(instance, notify);
      return () => {
        dispose?.();
        initialization.current.placed = false;
        initialization.current.follow = null;
        initialization.current.publishOffset = null;
      };
    },
    observeElementRect: (instance, notify) => {
      const element = instance.scrollElement;
      const Observer = instance.targetWindow?.ResizeObserver;
      if (element === null || Observer === undefined) return;
      const observer = new Observer(([entry]) => {
        if (entry === undefined) return;
        const box = entry.borderBoxSize?.[0];
        const rect = { width: Math.round(box?.inlineSize ?? entry.contentRect.width), height: Math.round(box?.blockSize ?? entry.contentRect.height) };
        const initial = initialization.current;
        if (!initial.placed && rect.height > 0) {
          // The first viewport size places the active mark without a scroll command.
          const max = Math.max(0, instance.getTotalSize() - rect.height);
          const center = initial.index * TURN_SPACING_PX + RAIL_INSET_PX;
          const target = Math.max(0, Math.min(max, center - rect.height / 2));
          initial.placed = true;
          initial.follow = { index: initial.index, count: instance.options.count, height: rect.height };
          element.scrollTop = target;
          initial.publishOffset?.(target, false);
        }
        notify(rect);
      });
      observer.observe(element, { box: 'border-box' });
      return () => { observer.disconnect(); };
    },
    paddingStart: RAIL_INSET_PX - TURN_SPACING_PX / 2,
    paddingEnd: RAIL_INSET_PX - TURN_SPACING_PX / 2,
    scrollPaddingStart: FADE_PX,
    scrollPaddingEnd: FADE_PX,
    overscan: 3,
    rangeExtractor: useCallback((range: Range) => {
      // Keyboard focus and its neighbours stay mounted, so Tab never loses its place.
      const indexes = defaultRangeExtractor(range);
      if (focusedIndex !== undefined) {
        const last = Math.min(range.count - 1, focusedIndex + 1);
        for (let index = Math.max(0, focusedIndex - 1); index <= last; index++) if (!indexes.includes(index)) indexes.push(index);
        indexes.sort((left, right) => left - right);
      }
      return indexes;
    }, [focusedIndex]),
  });
  const scrollTop = virtualizer.scrollOffset ?? 0;
  const viewHeight = virtualizer.scrollRect?.height ?? 0;
  const virtualItems = virtualizer.getVirtualItems();
  const scrollToIndex = useCallback((index: number, behavior: 'auto' | 'smooth' | 'instant') => {
    const item = virtualizer.measurementsCache[index];
    const height = virtualizer.scrollRect?.height ?? 0;
    if (item === undefined || height <= 0) return;
    const current = virtualizer.scrollOffset ?? 0, center = item.start + item.size / 2;
    const { scrollPaddingStart, scrollPaddingEnd } = virtualizer.options;
    if (center >= current + scrollPaddingStart && center <= current + height - scrollPaddingEnd) return;
    const max = Math.max(0, virtualizer.getTotalSize() - height);
    const delta = Math.max(0, Math.min(max, center - height / 2)) - current;
    if (delta !== 0) virtualizer.scrollBy(delta, { behavior });
  }, [virtualizer]);
  useEffect(() => {
    if (viewHeight <= 0) { initialization.current.follow = null; return; }
    if (activeAt === undefined || pointerInsideRef.current) return;
    const previous = initialization.current.follow;
    if (previous?.index === activeAt && previous.count === items.length && previous.height === viewHeight) return;
    initialization.current.follow = { index: activeAt, count: items.length, height: viewHeight };
    scrollToIndex(activeAt, previous?.count === items.length && previous.height === viewHeight ? preferredScrollBehavior() : 'instant');
  }, [activeAt, items.length, viewHeight, scrollToIndex]);
  if (items.length < 2) return null;
  const preview = previewIndex === undefined ? undefined : items[previewIndex];
  const previewPosition = virtualItems.find(item => item.index === previewIndex);
  const scroller = [css.scroller];
  if (scrollTop > 1) scroller.push(css.fadeTop);
  if (scrollTop < virtualizer.getTotalSize() - viewHeight - 1) scroller.push(css.fadeBottom);
  return <nav className={css.frame} aria-label={navigationLabel} aria-busy={loading || undefined}
    onPointerEnter={() => { pointerInsideRef.current = true; }}
    onPointerLeave={() => { pointerInsideRef.current = false; setPreviewKey(null); }}>
    <div ref={scrollerRef} className={scroller.join(' ')}>
      <div className={css.marks} style={{ height: virtualizer.getTotalSize() }}>
        {virtualItems.map(({ index, key, start }) => {
          const item = items[index];
          if (item === undefined) return null;
          return <TurnMark key={key} item={item} index={index} start={start} label={label(item)} active={isActive(item)} busy={isBusy(item)}
              unavailable={!!item.turn && (item.turn.cursor == null || !attached) || !item.turn && !attached}
              previewId={item.key === previewKey ? previewId : undefined} registerElement={virtualizer.measureElement}
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
