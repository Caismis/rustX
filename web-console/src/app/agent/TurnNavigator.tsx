/* Copyright (c) 2026 DeepSeek. MIT. Source-derived; see PROVENANCE.md. */
// Adapted from DeepSeek Harness ui-chat TurnNavigator: a fixed-pitch virtual
// rail of every known turn with hover/focus previews. rustX supplies the
// native Attempt outline; an unloaded mark pages its native outline page in
// before navigating.
import { memo, useCallback, useEffect, useId, useLayoutEffect, useMemo, useRef, useState, type CSSProperties } from 'react';
import type { ObservedTurn, ObservedTurnPage, ObservedTurnIdentity } from '../../../../protocol/app-server/v80';
import type { AppServerClient } from '../../client/app-server';
import { shallowEqual, useClientSelector } from '../../client/selectors';
import { turnKey } from '../../client/transcript';
import { useTranslation } from '../../locale/react';
import { currentTurnLocation, turnRail, turnRailRange, TURN_SPACING_PX, RAIL_INSET_PX, type TurnRailItem } from './turn-rail-items';
import { TURN_PAGE_SIZE, type TurnSelection } from '../../client/transcript';

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
  const tick = !item.turn
    ? 'before:[transform:translateY(-50%)_scaleX(0.4)] before:opacity-60'
    : active
      ? 'before:[transform:translateY(-50%)_scaleX(1)] before:bg-[var(--dsw-alias-label-primary)]'
      : previewId !== undefined
        ? 'before:[transform:translateY(-50%)_scaleX(0.9)] before:bg-[var(--dsw-alias-label-tertiary)]'
        : '';
  const busyTick = busy ? ' before:animate-[dsh-turn-mark-busy_1s_ease-in-out_infinite]' : '';
  return <button data-index={index} type="button" className={"tn-mark absolute top-0 right-0 left-0 h-[10px] p-0 border-0 rounded-[var(--dsw-radius-sm)] bg-transparent cursor-pointer aria-disabled:cursor-default before:absolute before:top-1/2 before:right-0 before:w-[20px] before:h-[2px] before:rounded-[var(--dsw-radius-xs)] before:bg-[var(--dsw-alias-border-l4)] before:content-[''] before:[transform:translateY(-50%)_scaleX(0.6)] before:origin-right before:transition-[transform,background-color] before:duration-140 before:ease-in-out focus-visible:outline-none focus-visible:before:[transform:translateY(-50%)_scaleX(1)] focus-visible:before:bg-[var(--dsw-alias-state-business-primary)] focus-visible:after:absolute focus-visible:after:inset-y-0 focus-visible:after:left-auto focus-visible:after:right-0 focus-visible:after:w-[20px] focus-visible:after:rounded-[inherit] focus-visible:after:[outline:1px_solid_var(--dsw-alias-state-business-primary)] focus-visible:after:-outline-offset-1 focus-visible:after:content-[''] motion-reduce:before:transition-none motion-reduce:before:animate-none " + tick + busyTick} style={{ transform: `translateY(${start}px)` }}
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
export function TurnNavigator({ client, sessionId, onNavigate, active, previewCurrent }: { previewCurrent?: () => boolean; client: AppServerClient; sessionId: string; onNavigate: (turn: TurnSelection) => void; active?: string | null }) {
  const view = useClientSelector(client, state => {
    const view = state.views[sessionId];
    return { target: view?.target, attachment: view?.attachment, outline: view?.turnOutline, navigation: view?.turnNavigation,
      attempt: view?.snapshot?.attempt?.attempt_id,attemptSource:view?.snapshot?.attempt?.attempt_id_source, phase: view?.snapshot?.attempt?.phase.type,
      conversation: view?.snapshot?.conversation_id, running: !!view?.snapshot?.attempt && ['admitted','running'].includes(view.snapshot.attempt.phase.type),
      location: currentTurnLocation(view?.snapshot), generation: state.generation };
  }, shallowEqual);
  useEffect(() => {
    if (view.attachment === 'attached') void client.refreshTurns(sessionId);
  }, [client, sessionId, view.target, view.attachment, view.attempt, view.phase,view.attemptSource, view.generation]);
  const currentId = useMemo(() => view.running && view.conversation && (view.attempt!=null||view.attemptSource!=null) ? { conversation_id: view.conversation, attempt_id: view.attempt,attempt_id_source:view.attemptSource } : undefined,
    [view.running, view.conversation, view.attempt,view.attemptSource]);
  const page = view.outline?.page;
  const readPreview = useCallback((ordinal: number, current: () => boolean) =>
    client.previewTurns(sessionId, Math.floor((ordinal - 1) / TURN_PAGE_SIZE) * TURN_PAGE_SIZE, current), [client, sessionId, view.generation, view.target?.attachment_id]);
  const missingLocation = !!page && !!view.location && BigInt(page.cut.transcript) < BigInt(view.location);
  useEffect(() => { if (missingLocation && !view.outline?.loading && !view.outline?.error && view.attachment === 'attached') void client.refreshTurns(sessionId); },
    [client, sessionId, missingLocation, view.outline?.loading, view.outline?.error, view.attachment, page?.offset]);
  return <TurnNavigation page={page} currentId={currentId} location={view.location} loading={!!view.outline?.loading}
    pending={view.navigation?.pending} error={view.navigation?.error ?? view.outline?.error}
    previewCurrent={previewCurrent} attached={view.attachment === 'attached'} active={active} onNavigate={onNavigate} readPreview={readPreview}
    onReload={() => { client.invalidateReading(sessionId); void client.refreshTurns(sessionId); }}/>;
}

/** Same Harness navigation surface for parent and child native outlines. */
export function TurnNavigation({ page, currentId, location, loading = false, pending, error, attached, active, onNavigate, onReload, readPreview, previewCurrent }: {
  previewCurrent?: () => boolean;
  page?: ObservedTurnPage; currentId?: ObservedTurnIdentity; location?: string; loading?: boolean;
  pending?: string; error?: string; attached: boolean; active?: string | null;
  onNavigate: (turn: TurnSelection) => void; onReload: () => void; readPreview?: (ordinal: number, current: () => boolean) => Promise<ObservedTurnPage | undefined>;
}) {
  const tx = useTranslation(), previewId = useId();
  const [previewKey, setPreviewKey] = useState<string | null>(null);
  const [focusedKey, setFocusedKey] = useState<string | null>(null);
  const rail = useMemo(() => {
    try { return {items:turnRail(page,currentId,location),error:undefined}; }
    catch(error) { return {items:turnRail(undefined,currentId,location),error:String(error)}; }
  }, [page,currentId,location]);
  const items=rail.items,feedbackError=rail.error??error;
  const [previewTurn, setPreviewTurn] = useState<ObservedTurn>();
  const previewOwner = useRef(0);
  useLayoutEffect(() => { ++previewOwner.current; setPreviewKey(null); setFocusedKey(null); setPreviewTurn(undefined); }, [previewCurrent]);
  useEffect(() => {
    const owner = ++previewOwner.current;
    if (previewTurn) setPreviewTurn(undefined);
    const index = items.indexOfKey(previewKey), item = index === undefined ? undefined : items.item(index);
    if (!readPreview || !item || item.turn || !attached || pending) return;
    const current = () => previewOwner.current === owner && (previewCurrent?.() ?? true);
    const timer = setTimeout(() => {
      void readPreview(item.ordinal, current).then(page => {
        if (current()) setPreviewTurn(page?.turns.find(turn => turn.ordinal === item.ordinal));
      });
    }, 150);
    return () => { clearTimeout(timer); ++previewOwner.current; };
  }, [readPreview, previewKey, attached, pending, page?.cut.mutation_revision, previewCurrent]);
  const isActive = (item: TurnRailItem) => !!item.id && (active !== undefined ? active === `turn:${item.id}` : !!currentId && turnKey(currentId) === item.id);
  // The owning viewport starts its gesture immediately, before outline I/O.
  const navigate = (item: TurnRailItem) => { ++previewOwner.current; onNavigate(item.turn ? {turn:item.turn,cut:page?.cut} : item.ordinal); };
  // No rail and nothing to recover: no slot layer over the reading surface.
  if (items.count < 2 && !feedbackError) return null;
  return <div className="absolute inset-x-0 top-0 bottom-[var(--dsh-composer-height,0px)] z-[7] px-[calc(var(--dsh-composer-side-clearance)+16px)] [container-type:inline-size] pointer-events-none" data-turn-navigator>
    <TurnRail navigationLabel={tx('agent:reading.turn-navigation')} loading={loading} items={items} activeId={active !== undefined ? active?.slice(5) : currentId && turnKey(currentId)} isActive={isActive}
      isBusy={item => pending === item.key || !!item.id && pending === item.id} attached={attached} onNavigate={navigate}
      previewKey={previewKey} previewTurn={previewTurn} setPreviewKey={setPreviewKey} focusedKey={focusedKey} setFocusedKey={setFocusedKey} previewId={previewId}
      label={item => item.ordinal === 0 ? tx('agent:reading.current-turn') : tx(item.turn ? 'agent:reading.jump-turn' : 'agent:reading.jump-load-turn', { n: item.ordinal })}
      title={item => item.turn?.prompt || (item.ordinal === 0 ? tx('agent:reading.current-turn') : tx('agent:reading.turn', { n: item.ordinal }))} />
    {feedbackError && <div className="tn-feedback box-border w-[min(300px,calc(100cqw-120px))] px-[12px] py-[10px] border-0 rounded-[var(--dsw-radius-lg)] [color:var(--dsw-alias-label-primary)] bg-[var(--dsw-alias-bg-layer-1)] shadow-[var(--dsw-elevation-panel)] absolute top-[16px] right-[52px] [font:var(--dsw-font-xxs-12)] wrap-anywhere pointer-events-auto [&_p]:mt-0 [&_p]:mb-[8px] [&_button]:px-[10px] [&_button]:py-[4px] [&_button]:[border:0.5px_solid_var(--dsw-alias-border-l3)] [&_button]:rounded-[var(--dsw-radius-sm)] [&_button]:[color:inherit] [&_button]:bg-transparent [&_button]:[font:inherit] [&_button]:cursor-pointer [&_button:focus-visible]:[outline:2px_solid_var(--dsw-alias-state-business-primary)] [&_button:focus-visible]:outline-offset-1" role="alert"><p>{feedbackError}</p><button type="button" onClick={() => { onReload(); }}>{tx('agent:reading.reload-turns')}</button></div>}
  </div>;
}

function TurnRail({ navigationLabel, loading, items, activeId, isActive, isBusy, attached, onNavigate, previewKey, previewTurn, setPreviewKey, focusedKey, setFocusedKey, previewId, label, title }: {
  navigationLabel: string; loading: boolean; items: ReturnType<typeof turnRail>; activeId?: string | null; isActive: (item: TurnRailItem) => boolean; isBusy: (item: TurnRailItem) => boolean; attached: boolean;
  previewTurn?: ObservedTurn;
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
  const previewContent = preview?.turn ?? (preview?.ordinal === previewTurn?.ordinal ? previewTurn : undefined);
  const previewPosition = virtualItems.find(item => item.index === previewIndex);
  const fadeUp = scrollTop > 1, fadeDown = scrollTop < totalSize - viewHeight - 1;
  const scrollerMask = fadeUp && fadeDown
    ? '[mask-image:linear-gradient(to_bottom,transparent_0,#000_24px,#000_calc(100%-24px),transparent_100%)]'
    : fadeUp
      ? '[mask-image:linear-gradient(to_bottom,transparent_0,#000_24px,#000_100%)]'
      : fadeDown
        ? '[mask-image:linear-gradient(to_bottom,#000_0,#000_calc(100%-24px),transparent_100%)]'
        : '';
  return <nav className="tn-frame [--turn-preview-height:100px] absolute top-1/2 right-[12px] flex flex-col w-[28px] max-h-[min(max(0px,calc(100%-64px)),420px)] contain-layout cursor-pointer pointer-events-auto -translate-y-1/2 @max-[900px]:hidden motion-reduce:transition-none motion-reduce:animate-none" aria-label={navigationLabel} aria-busy={loading || undefined}
    onPointerEnter={() => { pointerInsideRef.current = true; }}
    onPointerLeave={() => { pointerInsideRef.current = false; setPreviewKey(null); }}>
    <div ref={scrollerRef} className={"tn-scroller relative min-h-0 overflow-y-auto overscroll-contain [scrollbar-width:none] [&::-webkit-scrollbar]:hidden motion-reduce:transition-none motion-reduce:animate-none " + scrollerMask} onScroll={event => { const top = event.currentTarget.scrollTop; setGeometry(value => ({ ...value, top })); }}
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
      <div className="relative" style={{ height: totalSize }}>
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
    {preview !== undefined && previewPosition !== undefined && <div id={previewId} role="tooltip" className="box-border w-[min(300px,calc(100cqw-120px))] px-[12px] py-[10px] border-0 rounded-[var(--dsw-radius-lg)] [color:var(--dsw-alias-label-primary)] bg-[var(--dsw-alias-bg-layer-1)] shadow-[var(--dsw-elevation-panel)] absolute top-[clamp(0px,calc(var(--turn-preview-center)-var(--turn-preview-height)/2),calc(100%-var(--turn-preview-height)))] right-[calc(100%+10px)] max-h-[var(--turn-preview-height)] overflow-hidden pointer-events-none animate-[dsh-turn-preview-enter_120ms_ease-out] transition-[top] duration-140 ease-[cubic-bezier(0.2,0.8,0.2,1)] motion-reduce:transition-none motion-reduce:animate-none"
      style={{ '--turn-preview-center': `${previewPosition.start + previewPosition.size / 2 - scrollTop}px` } as CSSProperties}>
      <div className="[display:-webkit-box] overflow-hidden [-webkit-box-orient:vertical] wrap-anywhere [font:var(--dsw-font-xs-strong-13)] line-clamp-1">{previewContent?.prompt || title(preview)}</div>
      {previewContent?.response ? <div className="[display:-webkit-box] overflow-hidden [-webkit-box-orient:vertical] wrap-anywhere mt-[4px] [color:var(--dsw-alias-label-caption)] [font:var(--dsw-font-xxs-12)] line-clamp-3">{previewContent.response}</div> : null}
    </div>}
  </nav>;
}
