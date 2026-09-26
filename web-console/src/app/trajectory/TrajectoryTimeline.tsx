/* Copyright (c) 2026 DeepSeek. MIT. Adapted from pinned Harness ui-trajectory/TrajectoryTimeline.tsx; see PROVENANCE.md. */
/**
 * The fixed timing overview above the ledger.
 *
 * Adapted from the Harness overview: lanes projected left to right, drag to
 * focus an interval, wheel to zoom the time domain, a right-button click to
 * clear the interval and a right-button drag to pan a zoomed viewport. An
 * Assistant request's span shows the recorded split between waiting for the
 * first output and decoding, so the division is evidence rather than decoration.
 *
 * A timed span requires endpoints in its rendered domain. Request Model spans
 * use provider evidence; Journal terminal timing cannot replace a missing bridge.
 */
import { useEffect, useMemo, useRef, useState, type CSSProperties, type PointerEvent } from 'react';
import type { TraceRecord } from '../../../../protocol/app-server/v23';
import {
  TRAJECTORY_LANES,
  formatDuration,
  formatInstant,
  trajectoryTimeline,
  type TrajectoryTimeRange,
  type TrajectoryTimelineMode,
} from './timeline';
import { Button } from '../../presentation/primitives/Button';
import { Tooltip } from '../../presentation/primitives/Tooltip';
import css from './TrajectoryTimeline.module.css';

/** Pointer travel below which a drag is treated as a click. */
const MINIMUM_DRAG_PX = 3;
/** Smallest zoomed domain, in projected units, so a viewport stays usable. */
const MINIMUM_ZOOM_SPAN = 4;

interface Drag {
  pointerId: number;
  recordId?: string;
  clientX: number;
  anchor: number;
  current: number;
  moved: boolean;
}

interface Pan {
  pointerId: number;
  clientX: number;
  start: number;
  moved: boolean;
}

/**
 * The earlier-history marker at the overview's left edge.
 *
 * Adapted from the pinned Harness `EarlierHistoryBoundary`. Harness holds
 * its own pending flag because its callback returns a promise; rustX does
 * not, because the Trace cache already owns loading and the finite limit.
 * Pointer events stop here so pressing the marker cannot also start a drag
 * on the canvas underneath it.
 */
function EarlierHistoryBoundary({
  loading,
  enabled,
  onLoad,
}: {
  loading: boolean;
  enabled: boolean;
  onLoad: () => void;
}) {
  const actionable = enabled && !loading;
  return (
    <Tooltip
      label={loading ? 'Loading earlier records…' : 'Load earlier records'}
      side="right"
    >
      <button
        type="button"
        className={css.earlierHistory}
        data-earlier-history=""
        data-loading={loading || undefined}
        aria-label={
          loading ? 'Loading earlier records' : 'Load earlier records into the overview'
        }
        aria-disabled={!actionable}
        onClick={event => {
          event.stopPropagation();
          if (actionable) onLoad();
        }}
        onPointerDown={event => event.stopPropagation()}
        onPointerMove={event => event.stopPropagation()}
        onPointerUp={event => event.stopPropagation()}
        onContextMenu={event => event.stopPropagation()}
      >
        …
      </button>
    </Tooltip>
  );
}

/** Props for the Trajectory timing overview. */
export interface TrajectoryTimelineProps {
  records: readonly TraceRecord[];
  mode: TrajectoryTimelineMode;
  range: TrajectoryTimeRange | null;
  selectedId: string | null;
  /** Record identities matching the active search, or null without a query. */
  searchMatches: ReadonlySet<string> | null;
  onRangeChange: (range: TrajectoryTimeRange | null) => void;
  onSelect: (id: string) => void;
  /** Section boundary label for a record that opens one, else undefined. */
  boundaryLabel: (record: TraceRecord, index: number) => string | undefined;
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
 * @param props - loaded records, projection mode, and selection callbacks.
 * @returns the overview element, or an explicit empty state.
 */
export function TrajectoryTimeline({
  records,
  mode,
  range,
  selectedId,
  searchMatches,
  onRangeChange,
  onSelect,
  boundaryLabel,
  hasEarlierRecords,
  loadingEarlier,
  canLoadEarlier,
  onLoadEarlier,
}: TrajectoryTimelineProps) {
  const rootRef = useRef<HTMLDivElement>(null);
  const dragRef = useRef<Drag | null>(null);
  const panRef = useRef<Pan | null>(null);
  const [draft, setDraft] = useState<TrajectoryTimeRange | null>(null);
  const [viewport, setViewport] = useState<TrajectoryTimeRange | null>(null);
  const [hover, setHover] = useState<string | null>(null);
  const model = useMemo(
    () => trajectoryTimeline(records, mode, boundaryLabel),
    [records, mode, boundaryLabel],
  );

  // A rebuilt domain invalidates a viewport expressed in the old one.
  const domainKey = model === null ? '' : `${model.start}:${model.end}`;
  useEffect(() => { setViewport(null); }, [domainKey, mode]);

  const domain = viewport ?? (model === null ? null : { start: model.start, end: model.end });
  const span = domain === null ? 0 : Math.max(1e-6, domain.end - domain.start);

  useEffect(() => {
    const root = rootRef.current;
    if (root === null || model === null) return;
    const onWheel = (event: WheelEvent) => {
      event.preventDefault();
      const rect = root.getBoundingClientRect();
      const current = viewport ?? { start: model.start, end: model.end };
      const width = Math.max(1e-6, current.end - current.start);
      const at = current.start + ((event.clientX - rect.left) / Math.max(1, rect.width)) * width;
      const factor = event.deltaY > 0 ? 1.25 : 0.8;
      const next = Math.min(model.end - model.start, Math.max(MINIMUM_ZOOM_SPAN, width * factor));
      if (next >= model.end - model.start) { setViewport(null); return; }
      const ratio = (at - current.start) / width;
      const start = Math.max(model.start, Math.min(model.end - next, at - ratio * next));
      setViewport({ start, end: start + next });
    };
    root.addEventListener('wheel', onWheel, { passive: false });
    return () => { root.removeEventListener('wheel', onWheel); };
  }, [model, viewport]);

  // Native `TraceTiming.started_at` is mandatory, so every projected record
  // places a span and this branch means the loaded window holds no record at
  // all. A page with no records carries no cursor either, so there is no
  // earlier history to offer here: that affordance lives on the model-backed
  // path below, inside the positioned canvas.
  if (model === null || domain === null) {
    return (
      <section className={css.root} aria-label="Timing overview">
        <p className={css.empty}>No recorded timing in the loaded window</p>
      </section>
    );
  }

  const pointAt = (clientX: number) => {
    const rect = rootRef.current?.getBoundingClientRect();
    if (rect === undefined) return domain.start;
    const ratio = Math.min(1, Math.max(0, (clientX - rect.left) / Math.max(1, rect.width)));
    return domain.start + ratio * span;
  };
  const percent = (value: number) => ((value - domain.start) / span) * 100;
  // Only at the earliest edge of the projection, exactly as the pinned
  // Harness overview gates its own boundary: panned or zoomed away from the
  // start, the marker would point at history that is not adjacent to it.
  const showsEarlierBoundary = hasEarlierRecords && domain.start === model.start;

  const onPointerDown = (event: PointerEvent<HTMLDivElement>) => {
    if (event.button === 2) {
      panRef.current = { pointerId: event.pointerId, clientX: event.clientX, start: domain.start, moved: false };
      event.currentTarget.setPointerCapture(event.pointerId);
      return;
    }
    if (event.button !== 0) return;
    const at = pointAt(event.clientX);
    dragRef.current = { pointerId: event.pointerId, clientX: event.clientX, recordId: (event.target as HTMLElement).closest<HTMLElement>('[data-record-id]')?.dataset.recordId, anchor: at, current: at, moved: false };
    event.currentTarget.setPointerCapture(event.pointerId);
  };

  const onPointerMove = (event: PointerEvent<HTMLDivElement>) => {
    const pan = panRef.current;
    if (pan !== null && pan.pointerId === event.pointerId) {
      if (Math.abs(event.clientX - pan.clientX) >= MINIMUM_DRAG_PX) pan.moved = true;
      if (viewport === null) return;
      const rect = rootRef.current?.getBoundingClientRect();
      const delta = ((event.clientX - pan.clientX) / Math.max(1, rect?.width ?? 1)) * span;
      const start = Math.max(model.start, Math.min(model.end - span, pan.start - delta));
      setViewport({ start, end: start + span });
      return;
    }
    const drag = dragRef.current;
    if (drag === null || drag.pointerId !== event.pointerId) return;
    drag.current = pointAt(event.clientX);
    if (Math.abs(event.clientX - drag.clientX) >= MINIMUM_DRAG_PX) drag.moved = true;
    setDraft({ start: Math.min(drag.anchor, drag.current), end: Math.max(drag.anchor, drag.current) });
  };

  const onPointerUp = (event: PointerEvent<HTMLDivElement>) => {
    const pan = panRef.current;
    if (pan !== null && pan.pointerId === event.pointerId) {
      // A right-button click with no travel clears the focus interval; a
      // right-button drag pans instead and must not also clear it.
      if (!pan.moved) onRangeChange(null);
      panRef.current = null;
      return;
    }
    const drag = dragRef.current;
    if (drag === null || drag.pointerId !== event.pointerId) return;
    dragRef.current = null;
    setDraft(null);
    if (!drag.moved) {
      if (drag.recordId !== undefined) onSelect(drag.recordId);
      return;
    }
    onRangeChange({ start: Math.min(drag.anchor, drag.current), end: Math.max(drag.anchor, drag.current) });
  };

  const focus = draft ?? range;
  const labelledBoundaries = new Set<number>();
  let lastLabel = -15;
  for (const boundary of model.boundaries) {
    const at = percent(boundary.at);
    if (at >= 0 && at <= 90 && at - lastLabel >= 15) {
      labelledBoundaries.add(boundary.at);
      lastLabel = at;
    }
  }

  return (
    <section className={css.root} aria-label="Timing overview">
      <div className={css.legend}>
        <span>Overview</span>
        <small>Loaded window · drag to focus · wheel to zoom</small>
        <div className={css.controls}>
          <Button size="sm" aria-label="Zoom timeline in" onClick={() => {
            const next = Math.max(MINIMUM_ZOOM_SPAN, span * .8);
            if (next < span) setViewport({ start: domain.start, end: domain.start + next });
          }}>+</Button>
          <Button size="sm" aria-label="Reset timeline" onClick={() => { setViewport(null); onRangeChange(null); }}>Reset</Button>
        </div>
      </div>
      <div
        ref={rootRef}
        className={css.canvas}
        tabIndex={0}
        aria-label="Timeline navigation: arrow keys pan, Escape clears focus"
        data-domain-start={domain.start}
        data-domain-end={domain.end}
        onKeyDown={event => {
          if (event.key === 'Escape') { onRangeChange(null); return; }
          if (event.target !== event.currentTarget || !['ArrowLeft', 'ArrowRight'].includes(event.key)) return;
          event.preventDefault();
          const start = Math.max(model.start, Math.min(model.end - span, domain.start + span * (event.key === 'ArrowLeft' ? -.1 : .1)));
          setViewport({ start, end: start + span });
        }}
        onPointerDown={onPointerDown}
        onPointerMove={onPointerMove}
        onPointerUp={onPointerUp}
        onPointerCancel={() => { dragRef.current = null; panRef.current = null; setDraft(null); }}
        onContextMenu={event => event.preventDefault()}
      >
        {showsEarlierBoundary && (
          <EarlierHistoryBoundary
            loading={loadingEarlier}
            enabled={canLoadEarlier}
            onLoad={onLoadEarlier}
          />
        )}
        {focus !== null && (
          <div
            className={css.focus}
            aria-hidden="true"
            style={
              {
                left: `${percent(focus.start)}%`,
                width: `${Math.max(0.2, percent(focus.end) - percent(focus.start))}%`,
              } as CSSProperties
            }
          />
        )}
        {model.boundaries.map(boundary => (
          <div
            key={`${boundary.label}:${boundary.at}`}
            className={css.boundary}
            aria-hidden="true"
            style={{ left: `${percent(boundary.at)}%` } as CSSProperties}
          >
            {labelledBoundaries.has(boundary.at) && <span>{boundary.label}</span>}
          </div>
        ))}
        {TRAJECTORY_LANES.map((lane, index) => (
          <div className={css.lane} key={lane}>
            <span className={css.laneLabel}>{lane}</span>
            <div className={css.laneTrack}>
              {model.spans
                .filter(candidate => candidate.lane === index)
                .map(candidate => {
                  const left = percent(candidate.start);
                  const width = percent(candidate.end) - left;
                  const marker = mode !== 'sequence' && candidate.end === candidate.start;
                  const phasePercent = (at: number | undefined) =>
                    at === undefined || candidate.end <= candidate.start
                      ? undefined
                      : `${100 * (at - candidate.start) / (candidate.end - candidate.start)}%`;
                  const detail = [
                    candidate.label,
                    `Started ${formatInstant(
                      candidate.startedAt === undefined ? undefined : new Date(candidate.startedAt).toISOString(),
                    )}`,
                    candidate.durationMs === undefined
                      ? 'Journal duration unavailable'
                      : `Journal duration ${formatDuration(candidate.durationMs)}`,
                    candidate.ttftMs === undefined || candidate.generationMs === undefined
                      ? undefined
                      : `Dispatch → first output ${formatDuration(candidate.ttftMs)} · first output → provider terminal ${formatDuration(candidate.generationMs)}`,
                  ]
                    .filter(value => value !== undefined)
                    .join('\n');
                  return (
                    <button
                      key={candidate.id}
                      type="button"
                      className={css.span}
                      data-kind={candidate.kind}
                      data-record-id={candidate.id}
                      aria-pressed={candidate.id === selectedId}
                      data-error={candidate.error || undefined}
                      data-selected={candidate.id === selectedId || undefined}
                      data-marker={marker || undefined}
                      data-dimmed={
                        searchMatches !== null && !searchMatches.has(candidate.id) ? '' : undefined
                      }
                      aria-label={`Inspect ${candidate.label}`}
                      title={detail}
                      onFocus={() => setHover(candidate.id)}
                      onBlur={() => setHover(null)}
                      onPointerEnter={() => setHover(candidate.id)}
                      onPointerLeave={() => setHover(current => (current === candidate.id ? null : current))}
                      onClick={event => { event.stopPropagation(); onSelect(candidate.id); }}
                      style={
                        {
                          left: `${left}%`,
                          width: marker ? undefined : `${width}%`,
                          '--trajectory-dispatch': phasePercent(candidate.dispatchAt),
                          '--trajectory-first-output': phasePercent(candidate.firstOutputAt),
                        } as CSSProperties
                      }
                    />
                  );
                })}
            </div>
          </div>
        ))}
      </div>
      {/* A pointer hint, not an announcement: it changes on every hover, so
          giving it a live region would make the ledger unusable with a
          screen reader. Exact timing lives in the span's own accessible
          label and in the inspector's Timing section. */}
      <p className={css.hoverDetail} aria-hidden="true">
        {hover === null
          ? viewport === null
            ? ''
            : 'Zoomed · wheel out or right-drag to pan'
          : model.spans.find(candidate => candidate.id === hover)?.label ?? ''}
      </p>
    </section>
  );
}
