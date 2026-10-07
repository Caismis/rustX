import { useTranslation } from '../../locale/react';
import type { Translate } from '../../locale/translation';
/* Copyright (c) 2026 DeepSeek. MIT. Adapted from pinned Harness ui-trajectory/TrajectoryTimeline.tsx; see PROVENANCE.md. */
/**
 * The fixed timing overview above the ledger.
 *
 * Ported from the Harness overview: a 44px lane-label column beside a
 * clipped track, so a zoomed or panned domain never draws over the labels or
 * past the edge. Drag focuses an interval, the wheel zooms, a right-button
 * drag pans a zoomed viewport and a right-button click clears the interval.
 * Each block names its role, recorded range and timing in a delayed tooltip.
 *
 * A timed span requires endpoints in its rendered domain. Request Model spans
 * use provider evidence; Journal terminal timing cannot replace a missing bridge.
 */
import { useEffect, useRef, useState, type CSSProperties, type KeyboardEvent, type PointerEvent } from 'react';
import {
  formatDurationMillis,
  formatRecordedTime,
  timelineProjectionRevision,
  type TrajectorySpan,
  type TrajectoryTimelineModel,
  type TrajectoryTimeRange,
  type TrajectoryTimelineMode,
} from './timeline';
import { Tooltip } from '../../presentation/primitives/Tooltip';
import css from './TrajectoryTimeline.module.css';

const MINIMUM_DRAG_PX = 3;
const MINIMUM_ZOOM_OPERATIONS = 4;
const EDGE_PAN_ZONE_FRACTION = 0.08;
const EDGE_PAN_STEP_FRACTION = 0.025;
const MAXIMUM_EDGE_PAN_PX = 32;
const TIMELINE_TOOLTIP_DELAY_MS = 500;

interface HoverPoint {
  fraction: number;
  recordId: string | null;
}

interface PanGesture {
  anchorClientX: number;
  anchorStart: number;
  moved: boolean;
  pannable: boolean;
  pointerId: number;
}

function orderedRange(left: number, right: number): TrajectoryTimeRange {
  return left <= right ? { start: left, end: right } : { start: right, end: left };
}

function clampFraction(value: number): number {
  return Math.min(1, Math.max(0, value));
}

function centeredRange(center: number, width: number, minimum: number, maximum: number): TrajectoryTimeRange {
  const clampedWidth = Math.min(maximum - minimum, Math.max(0, width));
  const start = Math.min(Math.max(center - clampedWidth / 2, minimum), maximum - clampedWidth);
  return { start, end: start + clampedWidth };
}

function rangeFraction(range: TrajectoryTimeRange, start: number, duration: number, minimum: number, maximum: number): TrajectoryTimeRange {
  const bounded = orderedRange(
    Math.min(maximum, Math.max(minimum, range.start)),
    Math.min(maximum, Math.max(minimum, range.end)),
  );
  return { start: (bounded.start - start) / duration, end: (bounded.end - start) / duration };
}

/** Harness tooltip copy: role, recorded range, then total and phase timing. */
function tooltipLabel(tx: Translate, span: TrajectorySpan): string {
  const duration = span.durationMs === undefined
    ? null
    : tx('trajectory:timeline.total', { duration: formatDurationMillis(tx, span.durationMs) });
  const range = span.startedAt === undefined
    ? null
    : span.durationMs === undefined
      ? tx('trajectory:timeline.started', { time: formatRecordedTime(tx, span.startedAt) })
      : `${formatRecordedTime(tx, span.startedAt)} → ${formatRecordedTime(tx, span.startedAt + span.durationMs)}`;
  const segments = span.ttftMs === undefined || span.generationMs === undefined
    ? null
    : tx('trajectory:timeline.ttft-decoding', {
      ttft: formatDurationMillis(tx, span.ttftMs),
      decoding: formatDurationMillis(tx, span.generationMs),
    });
  const timing = [duration, segments].filter(value => value !== null).join(' · ');
  return [span.label, range, timing].filter(value => value !== null && value !== '').join('\n');
}

/**
 * The share of a Model block spent before its first output.
 *
 * Duration blocks place it from the native phase bridge; equal-width blocks
 * show the measured TTFT/decoding ratio. Without that evidence the block is
 * drawn as decoding alone rather than an invented split.
 */
function ttftFraction(span: TrajectorySpan, mode: TrajectoryTimelineMode): number | null {
  if (span.kind !== 'request') return null;
  if (mode === 'duration') {
    if (span.firstOutputAt === undefined || span.end <= span.start) return null;
    return clampFraction((span.firstOutputAt - span.start) / (span.end - span.start));
  }
  if (span.ttftMs === undefined || span.generationMs === undefined || span.ttftMs < 0 || span.generationMs < 0) return null;
  const total = span.ttftMs + span.generationMs;
  return total > 0 ? span.ttftMs / total : null;
}

function LaneLabels() {
  const tx = useTranslation();
  return (
    <div className={css.labels} aria-hidden="true">
      <span>{tx('trajectory:lane.Input')}</span>
      <span>{tx('trajectory:lane.Model')}</span>
      <span>{tx('trajectory:lane.Tools')}</span>
    </div>
  );
}

/**
 * The earlier-history marker at the track's left edge.
 *
 * Harness holds its own pending flag because its callback returns a promise;
 * rustX does not, because the Trace cache already owns loading and the
 * finite limit. Pointer events stop here so pressing the marker cannot also
 * start a drag on the track underneath it.
 */
function EarlierHistoryBoundary({ loading, enabled, onHover, onLoad }: {
  loading: boolean;
  enabled: boolean;
  onHover: () => void;
  onLoad: () => void;
}) {
  const tx = useTranslation();
  return (
    <Tooltip
      label={loading ? tx('trajectory:history.loading-earlier') : tx('trajectory:history.click-to-load-earlier')}
      side="right"
      delayMs={TIMELINE_TOOLTIP_DELAY_MS}
    >
      <button
        type="button"
        className={css.earlierHistory}
        data-earlier-history=""
        data-loading={loading || undefined}
        aria-label={loading ? tx('trajectory:history.loading-earlier') : tx('trajectory:history.load-earlier')}
        aria-disabled={loading || !enabled}
        onClick={() => { if (enabled && !loading) onLoad(); }}
        onPointerEnter={event => { event.stopPropagation(); onHover(); }}
        onPointerMove={event => { event.stopPropagation(); }}
        onPointerDown={event => { event.stopPropagation(); }}
        onPointerUp={event => { event.stopPropagation(); }}
      >
        …
      </button>
    </Tooltip>
  );
}

/** Props for the Trajectory timing overview. */
export interface TrajectoryTimelineProps {
  model: TrajectoryTimelineModel | null;
  mode: TrajectoryTimelineMode;
  range: TrajectoryTimeRange | null;
  selectedId: string | null;
  /** Span identities matching the active search, or null without a query. */
  searchMatches: ReadonlySet<string> | null;
  onRangeChange: (range: TrajectoryTimeRange | null) => void;
  /** Select a directly clicked block. */
  onSelect: (id: string) => void;
  /** Bring the record nearest a whitespace click into view without selecting it. */
  onReveal: (id: string) => void;
  /** True when the Trace cache reports an older page beyond this window. */
  hasEarlierRecords: boolean;
  /** True while that older page is already being fetched. */
  loadingEarlier: boolean;
  /**
   * True when another older page may still be requested.
   *
   * Paging has one owner. The overview never tracks cursors, pending loads
   * or the finite history limit itself; it renders the state the Trace cache
   * resolved and calls back into the same load the ledger uses.
   */
  canLoadEarlier: boolean;
  onLoadEarlier: () => void;
}

/**
 * Render the timing overview.
 * @param props - projected spans, focus range and selection callbacks.
 * @returns the overview element, or an explicit empty state.
 */
export function TrajectoryTimeline(props: TrajectoryTimelineProps) {
  // React commits the projection and its interaction owner atomically. A press
  // can finish only in the instance where it began; removed DOM/capture and
  // refs cannot deliver a P1 gesture to P2. Equivalent projections keep state.
  return <TimelineInteraction key={timelineProjectionRevision(props.model, props.mode)} {...props} />;
}

function TimelineInteraction({
  model,
  mode,
  range,
  selectedId,
  searchMatches,
  onRangeChange,
  onSelect,
  onReveal,
  hasEarlierRecords,
  loadingEarlier,
  canLoadEarlier,
  onLoadEarlier,
}: TrajectoryTimelineProps) {
  const tx = useTranslation();
  const rootRef = useRef<HTMLElement>(null);
  const trackRef = useRef<HTMLDivElement>(null);
  const dragRef = useRef<{ pointerId: number; anchorTime: number; anchorClientX: number; recordId: string | null } | null>(null);
  const panRef = useRef<PanGesture | null>(null);
  const [draft, setDraft] = useState<TrajectoryTimeRange | null>(null);
  const [hover, setHover] = useState<HoverPoint | null>(null);
  const [panning, setPanning] = useState(false);
  const [viewport, setViewport] = useState<TrajectoryTimeRange | null>(null);
  const [animateViewport, setAnimateViewport] = useState(false);
  // A selection made in the ledger scrolls a zoomed viewport to its block.
  useEffect(() => {
    if (model === null || selectedId === null) return;
    const selected = model.spans.find(span => span.id === selectedId);
    if (selected === undefined) return;
    setAnimateViewport(true);
    setViewport(current => {
      if (current === null || (selected.end > current.start && selected.start < current.end)) return current;
      const duration = Math.max(1, current.end - current.start);
      const desired = selected.end <= current.start ? selected.start : selected.end - duration;
      const start = Math.min(Math.max(desired, model.start), Math.max(model.start, model.end - duration));
      return start === current.start ? current : { start, end: start + duration };
    });
  }, [model, selectedId]);
  const fullDuration = Math.max(1, (model?.end ?? 0) - (model?.start ?? 0));
  const viewportDuration = Math.min(fullDuration, Math.max(1, (viewport?.end ?? 0) - (viewport?.start ?? 0)));
  const domainStart = model === null || viewport === null
    ? model?.start ?? 0
    : Math.min(Math.max(viewport.start, model.start), model.end - viewportDuration);
  const domainDuration = viewport === null ? fullDuration : viewportDuration;
  useEffect(() => {
    const root = rootRef.current;
    if (root === null) return;
    const onWheel = (event: WheelEvent) => {
      event.preventDefault();
      const track = trackRef.current;
      if (track === null || model === null) return;
      setAnimateViewport(false);
      const rect = track.getBoundingClientRect();
      const anchorFraction = clampFraction((event.clientX - rect.left) / Math.max(1, rect.width));
      const nextDuration = Math.min(
        fullDuration,
        Math.max(Math.min(mode === 'sequence' ? MINIMUM_ZOOM_OPERATIONS : 20, fullDuration), domainDuration * Math.exp(event.deltaY * 0.0015)),
      );
      if (nextDuration >= fullDuration * 0.999) { setViewport(null); return; }
      const anchorTime = domainStart + anchorFraction * domainDuration;
      const nextStart = Math.min(Math.max(anchorTime - anchorFraction * nextDuration, model.start), model.end - nextDuration);
      setViewport({ start: nextStart, end: nextStart + nextDuration });
    };
    root.addEventListener('wheel', onWheel, { passive: false });
    return () => { root.removeEventListener('wheel', onWheel); };
  }, [domainDuration, domainStart, fullDuration, mode, model]);

  // Native `TraceTiming.started_at` is mandatory, so every projected record
  // places a span and this branch means the loaded window holds no record at
  // all. A page with no records carries no cursor either, so there is no
  // earlier history to offer here.
  if (model === null) {
    return (
      <section ref={rootRef} className={css.root} aria-label={tx('trajectory:timeline.aria')}>
        <div className={css.plot}>
          <LaneLabels />
          <div className={css.track}>
            <span className={css.empty}>{tx('trajectory:timeline.no-timing-data')}</span>
          </div>
        </div>
      </section>
    );
  }

  const projectedDomainStyle = {
    '--trajectory-domain-left': `${-(domainStart - model.start) / domainDuration * 100}%`,
    '--trajectory-domain-width': `${fullDuration / domainDuration * 100}%`,
  } as CSSProperties;
  const visibleRange = draft !== null
    ? rangeFraction(draft, domainStart, domainDuration, model.start, model.end)
    : range === null ? null : rangeFraction(range, domainStart, domainDuration, model.start, model.end);
  const activeRange = draft ?? range;
  // Only at the earliest edge of the projection, exactly as the Harness
  // overview gates its boundary: zoomed away from the start, the marker would
  // point at history that is not adjacent to it.
  const showsEarlierBoundary = hasEarlierRecords && domainStart === model.start;
  const minimumSelectionDuration = Math.min(domainDuration, fullDuration / model.spans.length);

  const fractionAt = (event: PointerEvent<HTMLDivElement>) => {
    const rect = event.currentTarget.getBoundingClientRect();
    return clampFraction((event.clientX - rect.left) / Math.max(1, rect.width));
  };
  const recordAt = (event: PointerEvent<HTMLDivElement>) =>
    (event.target instanceof Element ? event.target.closest<HTMLElement>('[data-record-id]')?.dataset.recordId : undefined) ?? null;

  const onPointerDown = (event: PointerEvent<HTMLDivElement>) => {
    if (event.button === 2) {
      panRef.current = { anchorClientX: event.clientX, anchorStart: domainStart, moved: false, pannable: viewport !== null, pointerId: event.pointerId };
      if (viewport !== null) setAnimateViewport(false);
      setPanning(true);
      event.currentTarget.setPointerCapture(event.pointerId);
      return;
    }
    if (event.button !== 0) return;
    const anchor = fractionAt(event);
    const anchorTime = domainStart + anchor * domainDuration;
    const recordId = recordAt(event);
    setHover({ fraction: anchor, recordId });
    dragRef.current = { pointerId: event.pointerId, anchorTime, anchorClientX: event.clientX, recordId };
    event.currentTarget.setPointerCapture(event.pointerId);
    setDraft({ start: anchorTime, end: anchorTime });
  };

  const onPointerMove = (event: PointerEvent<HTMLDivElement>) => {
    const rect = event.currentTarget.getBoundingClientRect();
    const fraction = fractionAt(event);
    setHover({ fraction, recordId: recordAt(event) });
    const pan = panRef.current;
    if (pan !== null && pan.pointerId === event.pointerId) {
      if (Math.abs(event.clientX - pan.anchorClientX) >= MINIMUM_DRAG_PX) pan.moved = true;
      if (!pan.pannable) return;
      const delta = (event.clientX - pan.anchorClientX) / Math.max(1, rect.width);
      const start = Math.min(Math.max(pan.anchorStart - delta * domainDuration, model.start), model.end - domainDuration);
      setViewport({ start, end: start + domainDuration });
      return;
    }
    const drag = dragRef.current;
    if (drag === null || drag.pointerId !== event.pointerId) return;
    let nextDomainStart = domainStart;
    if (viewport !== null) {
      const localX = event.clientX - rect.left;
      const edgeWidth = Math.min(MAXIMUM_EDGE_PAN_PX, Math.max(1, rect.width * EDGE_PAN_ZONE_FRACTION));
      const direction = localX < edgeWidth ? -1 : localX > rect.width - edgeWidth ? 1 : 0;
      if (direction !== 0) {
        const edgeDistance = direction < 0 ? edgeWidth - localX : localX - (rect.width - edgeWidth);
        const strength = clampFraction(edgeDistance / edgeWidth);
        const desired = domainStart + direction * domainDuration * EDGE_PAN_STEP_FRACTION * Math.max(0.2, strength);
        nextDomainStart = Math.min(Math.max(desired, model.start), model.end - domainDuration);
        if (nextDomainStart !== domainStart) {
          setAnimateViewport(false);
          setViewport({ start: nextDomainStart, end: nextDomainStart + domainDuration });
        }
      }
    }
    setDraft(orderedRange(drag.anchorTime, nextDomainStart + fraction * domainDuration));
  };

  const onPointerEnd = (event: PointerEvent<HTMLDivElement>) => {
    const pan = panRef.current;
    if (pan !== null && pan.pointerId === event.pointerId) {
      const moved = pan.moved || Math.abs(event.clientX - pan.anchorClientX) >= MINIMUM_DRAG_PX;
      panRef.current = null;
      setPanning(false);
      // A right-button click with no travel clears the focus interval; a
      // right-button drag pans instead and must not also clear it.
      if (!moved) onRangeChange(null);
      return;
    }
    const drag = dragRef.current;
    if (drag === null || drag.pointerId !== event.pointerId) return;
    const pointFraction = fractionAt(event);
    const selected = orderedRange(drag.anchorTime, domainStart + pointFraction * domainDuration);
    setHover({ fraction: pointFraction, recordId: recordAt(event) });
    dragRef.current = null;
    setDraft(null);
    const click = Math.abs(event.clientX - drag.anchorClientX) < MINIMUM_DRAG_PX;
    const clicked = click && drag.recordId !== null ? model.spans.find(span => span.id === drag.recordId) : undefined;
    if (clicked !== undefined) {
      onRangeChange(null);
      onSelect(clicked.id);
      return;
    }
    onRangeChange(selected.end - selected.start < minimumSelectionDuration
      ? centeredRange(click ? selected.start : (selected.start + selected.end) / 2, minimumSelectionDuration, model.start, model.end)
      : selected);
    if (click) {
      const point = selected.start;
      const distance = (span: TrajectorySpan) => point < span.start ? span.start - point : point > span.end ? point - span.end : 0;
      const nearest = model.spans.reduce((candidate, span) => distance(span) < distance(candidate) ? span : candidate);
      onReveal(nearest.id);
    }
  };

  const onPointerCancel = () => {
    dragRef.current = null;
    panRef.current = null;
    setDraft(null);
    setHover(null);
    setPanning(false);
  };

  const onKeyDown = (event: KeyboardEvent<HTMLDivElement>) => {
    if (event.key !== 'Escape' || range === null) return;
    event.preventDefault();
    onRangeChange(null);
  };

  return (
    <section ref={rootRef} className={css.root} aria-label={tx('trajectory:timeline.aria')}>
      <div className={css.plot}>
        <LaneLabels />
        <div
          ref={trackRef}
          className={css.track}
          data-panning={panning || undefined}
          data-domain-start={domainStart}
          data-domain-end={domainStart + domainDuration}
          aria-label={tx('trajectory:timeline.overview-aria')}
          tabIndex={0}
          onKeyDown={onKeyDown}
          onPointerDown={onPointerDown}
          onPointerMove={onPointerMove}
          onPointerUp={onPointerEnd}
          onPointerCancel={onPointerCancel}
          onPointerLeave={() => { if (dragRef.current === null && panRef.current === null) setHover(null); }}
          onDoubleClick={event => { event.preventDefault(); onRangeChange(null); }}
          onContextMenu={event => { event.preventDefault(); }}
        >
          {showsEarlierBoundary && (
            <EarlierHistoryBoundary loading={loadingEarlier} enabled={canLoadEarlier} onHover={() => { setHover(null); }} onLoad={onLoadEarlier} />
          )}
          {hover !== null && hover.recordId === null && draft === null && (
            <div className={css.hoverLine} data-timeline-hover-line="" aria-hidden="true" style={{ '--trajectory-hover-left': `${hover.fraction * 100}%` } as CSSProperties} />
          )}
          {visibleRange !== null && (
            <>
              <div
                className={css.selection}
                data-focus-range=""
                data-dragging={draft === null ? undefined : 'true'}
                aria-hidden="true"
                style={{ '--trajectory-selection-left': `${visibleRange.start * 100}%`, '--trajectory-selection-width': `${(visibleRange.end - visibleRange.start) * 100}%` } as CSSProperties}
              />
              <div
                className={css.selectionEdges}
                data-dragging={draft === null ? undefined : 'true'}
                aria-hidden="true"
                style={{ '--trajectory-selection-left': `${visibleRange.start * 100}%`, '--trajectory-selection-width': `${(visibleRange.end - visibleRange.start) * 100}%` } as CSSProperties}
              />
            </>
          )}
          <div className={css.turnBoundaries} data-animate-viewport={animateViewport || undefined} aria-hidden="true" style={projectedDomainStyle}>
            {model.boundaries
              .filter(boundary => boundary.at > model.start && boundary.at >= domainStart && boundary.at <= domainStart + domainDuration)
              .map(boundary => (
                <span
                  key={boundary.nativeAttemptId}
                  className={css.turnBoundary}
                  style={{ '--trajectory-turn-left': `${(boundary.at - model.start) / fullDuration * 100}%` } as CSSProperties}
                />
              ))}
          </div>
          <div className={css.lanes} data-animate-viewport={animateViewport || undefined} data-timeline-domain="" style={projectedDomainStyle}>
            {model.spans
              .filter(span => span.id === selectedId || (span.end >= domainStart && span.start <= domainStart + domainDuration))
              .map(span => {
                const width = (span.end - span.start) / fullDuration * 100;
                const ttft = ttftFraction(span, mode);
                return (
                  <Tooltip key={span.id} label={() => tooltipLabel(tx, span)} side="bottom" delayMs={TIMELINE_TOOLTIP_DELAY_MS}>
                    <span
                      aria-hidden="true"
                      className={css.span}
                      data-timeline-span={span.kind}
                      data-record-id={span.id}
                      data-assistant-timing={ttft === null ? undefined : 'true'}
                      data-error={span.error || undefined}
                      data-current={span.id === selectedId || undefined}
                      data-hovered={hover?.recordId === span.id || undefined}
                      data-search-match={searchMatches === null ? undefined : searchMatches.has(span.id) ? 'true' : 'false'}
                      data-selected={activeRange === null ? undefined : span.start <= activeRange.end && span.end >= activeRange.start ? 'true' : 'false'}
                      style={{
                        '--trajectory-span-left': `${(span.start - model.start) / fullDuration * 100}%`,
                        '--trajectory-span-width': `${width}%`,
                        '--trajectory-span-gap': `min(${width * 0.08}%, 1px)`,
                        '--trajectory-span-lane': span.lane,
                        ...(ttft === null ? {} : { '--trajectory-assistant-ttft': `${ttft * 100}%` }),
                      } as CSSProperties}
                    />
                  </Tooltip>
                );
              })}
          </div>
        </div>
      </div>
    </section>
  );
}
