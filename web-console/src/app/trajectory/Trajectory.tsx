/* Copyright (c) 2026 DeepSeek. MIT. Adapted from pinned Harness ui-trajectory/TrajectoryTable.tsx and TrajectoryToolbar.tsx; see PROVENANCE.md. */
/**
 * The Trajectory view: a dense chronological ledger over native Trace
 * records, with a timing overview above it and a record inspector beside it.
 *
 * The browser owns presentation only. It renders, folds, searches, selects,
 * pages and anchors scrolling; it never folds Journal events, never infers a
 * retry from timestamps, never derives a duration, and never turns a
 * proposed ToolCall into an execution. Every displayed fact is one the
 * server resolved from native authority.
 */
import {
  useCallback,
  useEffect,
  useLayoutEffect,
  useMemo,
  useRef,
  useState,
  type CSSProperties,
} from 'react';
import { useVirtualizer } from '@tanstack/react-virtual';
import type { TraceRecord } from '../../../../protocol/app-server/v12';
import type { TraceCache } from '../../client/trace';
import { TRACE_LIMIT } from '../../client/trace';
import { Button } from '../../presentation/primitives/Button';
import { Input } from '../../presentation/primitives/Input';
import { Tooltip } from '../../presentation/primitives/Tooltip';
import { TrajectoryInspector } from './TrajectoryInspector';
import { CellContent, CellIcon, cellLabel, cellNarrowLabel, previewOf } from './TrajectoryCell';
import { TrajectoryTimeline } from './TrajectoryTimeline';
import {
  foldRows,
  foldableKeys,
  sectionKeyOf,
  sectionLabel,
  sectionOrdinals,
  stepFoldKey,
  trajectoryRows,
} from './layout';
import { TrajectorySearchIndex } from './search';
import { timelineFocus, trajectoryTimeline, type TrajectoryTimeRange, type TrajectoryTimelineMode } from './timeline';
import { formatDuration } from './timeline';
import css from './Trajectory.module.css';

/** Row height, fixed so a prepend preserves the reader's visible anchor. */
const ROW_HEIGHT = 30;
/** Folded summary rows are shorter, exactly as the Harness ledger makes them. */
const SUMMARY_ROW_HEIGHT = 20;
/** Below this many rows the ledger renders directly; above it, virtualized. */
const VIRTUALIZATION_THRESHOLD = 100;
const OVERSCAN_ROWS = 12;
/** Distance from the bottom within which the reader counts as "at the tail". */
const TAIL_THRESHOLD_PX = 2;

function durationOf(record: TraceRecord): string {
  return record.timing.duration_ms == null
    ? '—'
    : formatDuration(Number(record.timing.duration_ms));
}

/** Props for the Trajectory view. */
export interface TrajectoryProps {
  cache: TraceCache;
  loadEarlier: () => void;
  latest: () => void;
  onSelect: (id?: string) => void;
  onLoadDetail: (id: string) => void;
}

/**
 * Render the Trajectory ledger, overview and inspector.
 * @param props - the loaded Trace window and its read controls.
 * @returns the Trajectory view.
 */
export function Trajectory({ cache, loadEarlier, latest, onSelect, onLoadDetail }: TrajectoryProps) {
  const [query, setQuery] = useState('');
  const [mode, setMode] = useState<TrajectoryTimelineMode>('sequence');
  const [foldedAttempts, setFoldedAttempts] = useState<ReadonlySet<string>>(new Set());
  const [foldedSteps, setFoldedSteps] = useState<ReadonlySet<string>>(new Set());
  const [selectedId, setSelectedId] = useState<string | undefined>(cache.selection?.id);
  const [focusRange, setFocusRange] = useState<TrajectoryTimeRange | null>(null);
  const [searchIndex] = useState(() => new TrajectorySearchIndex());
  const [indexRevision, setIndexRevision] = useState(0);

  const records = cache.page.records;
  const allRows = useMemo(() => trajectoryRows(records), [records]);
  const ordinals = useMemo(() => sectionOrdinals(allRows), [allRows]);

  useEffect(() => {
    if (searchIndex.update(allRows)) setIndexRevision(revision => revision + 1);
  }, [allRows, searchIndex]);
  const searchMatches = useMemo(
    () => searchIndex.search(query),
    // The revision participates so a rebuilt index re-runs the same query.
    [searchIndex, indexRevision, query],
  );

  const boundaryLabel = useCallback(
    (_record: TraceRecord, index: number) => {
      const row = allRows[index];
      if (row === undefined || !row.sectionStart) return undefined;
      return sectionLabel(row.attempt, ordinals.get(row.section) ?? 0);
    },
    [allRows, ordinals],
  );
  const timelineModel = useMemo(
    () => trajectoryTimeline(records, mode, boundaryLabel),
    [records, mode, boundaryLabel],
  );
  const focusedIds = useMemo(
    () => timelineFocus(timelineModel, focusRange),
    [timelineModel, focusRange],
  );

  /**
   * The rows actually rendered.
   *
   * Search replaces folding while a query is active, exactly as Harness
   * does: a folded section must not hide a match the reader asked for.
   */
  const rows = useMemo(() => {
    if (searchMatches !== null) {
      return allRows.filter(row => searchMatches.has(row.record.id));
    }
    const byAttempt = foldRows(allRows, foldedAttempts, 'attempt', row => sectionKeyOf(row.record));
    return foldRows(byAttempt, foldedSteps, 'step', row => stepFoldKey(row.record));
  }, [allRows, foldedAttempts, foldedSteps, searchMatches]);

  const selected =
    (cache.selection?.id === selectedId ? cache.selection : undefined) ??
    records.find(record => record.id === selectedId);
  const selectedDetail = selectedId === undefined ? undefined : cache.details[selectedId];
  const select = useCallback(
    (id?: string) => {
      setSelectedId(id);
      onSelect(id);
    },
    [onSelect],
  );

  const viewport = useRef<HTMLDivElement>(null);
  const pendingFocus = useRef<string | undefined>(undefined);
  const followsTail = useRef(true);
  const mounted = useRef(false);
  const prependAnchor = useRef<{ first: string | undefined; scrollHeight: number; scrollTop: number; virtualized: boolean } | null>(null);
  const virtualized = rows.length > VIRTUALIZATION_THRESHOLD;
  const rowKey = useCallback(
    (index: number) => {
      const row = rows[index]!;
      return row.collapsedSummary === undefined ? row.record.id : `${row.record.id}:summary`;
    },
    [rows],
  );
  const rowSize = useCallback(
    (index: number) => (rows[index]?.collapsedSummary === undefined ? ROW_HEIGHT : SUMMARY_ROW_HEIGHT),
    [rows],
  );
  const virtualizer = useVirtualizer({
    count: virtualized ? rows.length : 0,
    enabled: virtualized,
    getScrollElement: () => viewport.current,
    estimateSize: rowSize,
    getItemKey: rowKey,
    overscan: OVERSCAN_ROWS,
    initialRect: { width: 800, height: 500 },
    anchorTo: 'end',
    followOnAppend: 'auto',
    scrollEndThreshold: TAIL_THRESHOLD_PX,
  });

  useLayoutEffect(() => {
    const id = pendingFocus.current;
    if (!id) return;
    const index = rows.findIndex(row => !row.collapsedSummary && row.record.id === id);
    if (index < 0) return;
    pendingFocus.current = undefined;
    followsTail.current = false;
    if (virtualized) virtualizer.scrollToIndex(index, { align: 'auto' });
    else {
      const node = Array.from(viewport.current?.querySelectorAll<HTMLElement>('[data-trace-id]') ?? []).find(node => node.dataset.traceId === id);
      if (node && viewport.current) {
        const pane = viewport.current;
        const rowBounds = node.getBoundingClientRect();
        const paneBounds = pane.getBoundingClientRect();
        if (rowBounds.top < paneBounds.top) pane.scrollTop += rowBounds.top - paneBounds.top;
        else if (rowBounds.bottom > paneBounds.bottom) pane.scrollTop += rowBounds.bottom - paneBounds.bottom;
      }
    }
  }, [rows, selectedId, virtualized, virtualizer]);

  const firstId = records[0]?.id;
  useLayoutEffect(() => {
    const pane = viewport.current;
    if (pane === null) return;
    const anchor = prependAnchor.current;
    // A prepended older page must not move what the reader is looking at:
    // restore the previous distance from the bottom of the content.
    if (anchor !== null && anchor.first !== firstId) {
      if (!virtualized) pane.scrollTop = anchor.scrollTop + pane.scrollHeight - anchor.scrollHeight;
      else if (!anchor.virtualized) {
        // The virtualizer has no prior keyed anchor on its first enabled
        // render. Transfer the existing reader offset across that boundary.
        virtualizer.scrollToOffset(anchor.scrollTop + pane.scrollHeight - anchor.scrollHeight);
      }
      prependAnchor.current = null;
      followsTail.current = false;
      return;
    }
    if (!mounted.current && rows.length > 0) {
      mounted.current = true;
      followsTail.current = true;
      if (virtualized) virtualizer.scrollToIndex(rows.length - 1, { align: 'end' });
      else pane.scrollTop = pane.scrollHeight;
      return;
    }
    // New records follow the tail only while the reader is still at it.
    if (followsTail.current && !virtualized) pane.scrollTop = pane.scrollHeight;
  }, [firstId, rows.length, virtualized, virtualizer]);

  // One paging authority. The toolbar button, the ledger's history row and
  // the overview's earlier-history marker all read these and call the same
  // `requestOlder`; none of them tracks a cursor or a pending load itself.
  const hasEarlierRecords = Boolean(cache.page.next_cursor);
  const canLoadEarlier = hasEarlierRecords && !cache.loading && records.length < TRACE_LIMIT;

  const requestOlder = () => {
    if (!canLoadEarlier) return;
    const pane = viewport.current;
    if (pane !== null) {
      prependAnchor.current = { first: firstId, scrollHeight: pane.scrollHeight, scrollTop: pane.scrollTop, virtualized };
    }
    loadEarlier();
  };

  const toggleAttempt = (key: string) =>
    setFoldedAttempts(current => {
      const next = new Set(current);
      if (!next.delete(key)) next.add(key);
      return next;
    });
  const toggleStep = (key: string) =>
    setFoldedSteps(current => {
      const next = new Set(current);
      if (!next.delete(key)) next.add(key);
      return next;
    });
  const attemptKeys = useMemo(() => foldableKeys(allRows.filter(row => row.attempt !== null), row => sectionKeyOf(row.record)), [allRows]);
  const stepKeys = useMemo(() => foldableKeys(allRows.filter(row => row.record.location.step_id != null), row => stepFoldKey(row.record)), [allRows]);
  const allAttemptsFolded = attemptKeys.length > 0 && attemptKeys.every(key => foldedAttempts.has(key));
  const allStepsFolded = stepKeys.length > 0 && stepKeys.every(key => foldedSteps.has(key));

  const rendered = virtualized
    ? virtualizer.getVirtualItems().map(item => ({ row: rows[item.index]!, item }))
    : rows.map((row, index) => ({ row, item: { index, key: rowKey(index), start: 0, size: rowSize(index) } }));

  const selectionOutsideWindow = selected !== undefined && !records.some(record => record.id === selected.id);
  const selectionRemoved = selectedId !== undefined && selected === undefined;

  return (
    <section className={css.root} aria-label="Trajectory">
      <div className={css.toolbar} role="toolbar" aria-label="Trajectory controls">
        <Button
          size="sm"
          aria-pressed={mode === 'duration'}
          title={mode === 'duration' ? 'Use equal-width blocks' : 'Use recorded durations'}
          onClick={() => {
            setMode(current => (current === 'duration' ? 'sequence' : 'duration'));
            setFocusRange(null);
          }}
        >
          Duration
        </Button>
        <Button
          size="sm"
          aria-pressed={allAttemptsFolded}
          onClick={() =>
            setFoldedAttempts(allAttemptsFolded ? new Set() : new Set(attemptKeys))
          }
        >
          {allAttemptsFolded ? 'Expand Attempts' : 'Fold Attempts'}
        </Button>
        <Button size="sm" aria-pressed={allStepsFolded} onClick={() => setFoldedSteps(allStepsFolded ? new Set() : new Set(stepKeys))}>
          {allStepsFolded ? 'Expand Steps' : 'Fold Steps'}
        </Button>
        <Input
          aria-label="Search loaded Trace"
          value={query}
          onChange={event => setQuery(event.target.value)}
          placeholder="Search loaded records"
        />
        <Button size="sm" disabled={!canLoadEarlier} onClick={requestOlder}>
          {cache.loading ? 'Loading…' : 'Load older'}
        </Button>
        <Button size="sm" onClick={latest}>
          Latest
        </Button>
        <small className={css.count}>
          {records.length} loaded
          {searchMatches === null ? '' : ` · ${rows.length} matching`}
        </small>
      </div>
      {cache.error && <p role="alert" className={css.error}>{cache.error}</p>}

      <TrajectoryTimeline
        records={records}
        mode={mode}
        range={focusRange}
        selectedId={selectedId ?? null}
        searchMatches={searchMatches}
        onRangeChange={setFocusRange}
        onSelect={id => {
          pendingFocus.current = id;
          const record = records.find(record => record.id === id);
          if (record) {
            setFoldedAttempts(current => { const next = new Set(current); next.delete(sectionKeyOf(record)); return next; });
            setFoldedSteps(current => { const next = new Set(current); next.delete(stepFoldKey(record)); return next; });
          }
          // Picking a record in the overview is an explicit request to see
          // that record. An active query that excludes it would leave it
          // selected but absent from the ledger, so the query yields to the
          // selection. A query that already matches it is left alone.
          if (searchMatches !== null && !searchMatches.has(id)) setQuery('');
          select(id);
        }}
        boundaryLabel={boundaryLabel}
        hasEarlierRecords={hasEarlierRecords}
        loadingEarlier={cache.loading === true}
        canLoadEarlier={canLoadEarlier}
        onLoadEarlier={requestOlder}
      />

      {selectionOutsideWindow && (
        <p role="status" className={css.note}>
          The selected record is retained outside the loaded history window.
        </p>
      )}
      {selectionRemoved && (
        <p role="status" className={css.note}>
          The selected record left the loaded window.{' '}
          <Button size="sm" onClick={() => select(undefined)}>
            Dismiss selection
          </Button>
        </p>
      )}

      <div className={css.split}>
        <div
          ref={viewport}
          className={css.ledger}
          data-trajectory-scroll=""
          role="table"
          aria-label="Trace ledger"
          aria-rowcount={rows.length}
          style={{ overflowAnchor: 'none' }}
          onScroll={event => {
            const pane = event.currentTarget;
            followsTail.current =
              pane.scrollHeight - pane.clientHeight - pane.scrollTop <= TAIL_THRESHOLD_PX;
          }}
        >
          <div className={css.loadRow}>
            {hasEarlierRecords ? (
              <Button size="sm" disabled={!canLoadEarlier} onClick={requestOlder}>
                {cache.loading ? 'Loading earlier records…' : 'Load earlier records'}
              </Button>
            ) : <span>Beginning of loaded history</span>}
          </div>
          <div
            style={
              virtualized
                ? { height: virtualizer.getTotalSize(), position: 'relative' }
                : { position: 'relative' }
            }
          >
            {rendered.map(({ row, item }) => {
              const record = row.record;
              const folded = row.collapsedSummary !== undefined;
              const structural = record.kind === 'attempt' || record.kind === 'step';
              const attemptFolded = foldedAttempts.has(sectionKeyOf(record));
              const stepFolded = foldedSteps.has(stepFoldKey(record));
              const style: CSSProperties = virtualized
                ? {
                    position: 'absolute',
                    top: 0,
                    left: 0,
                    width: '100%',
                    height: item.size,
                    transform: `translateY(${item.start}px)`,
                  }
                : { height: item.size };
              return (
                <div
                  key={item.key}
                  role="row"
                  tabIndex={folded ? -1 : 0}
                  aria-label={`${cellLabel[record.kind]} · ${previewOf(record) || record.state}`}
                  onKeyDown={event => {
                    if (event.target !== event.currentTarget) return;
                    if (event.key === 'Enter' || event.key === ' ') { event.preventDefault(); select(record.id); }
                    if (event.key === 'Escape') select(undefined);
                  }}
                  aria-rowindex={item.index + 1}
                  aria-selected={!folded && selectedId === record.id}
                  className={css.record}
                  data-trace-id={record.id}
                  data-kind={record.kind}
                  data-state={record.state}
                  data-structural={structural || undefined}
                  data-attempt={record.location.attempt_id ?? undefined}
                  data-step={record.location.step_id ?? undefined}
                  data-section-start={row.sectionStart || undefined}
                  data-section-end={row.sectionEnd || undefined}
                  data-group-start={row.groupStart || undefined}
                  data-collapsed-summary={row.collapsedKind}
                  data-selected={(!folded && selectedId === record.id) || undefined}
                  data-timeline-focus={
                    focusedIds === null ? undefined : focusedIds.has(record.id) ? 'inside' : 'outside'
                  }
                  style={style}
                  onClick={() => { if (!folded) select(record.id); }}
                  onDoubleClick={event => {
                    if (folded) return;
                    event.preventDefault();
                    if (record.location.step_id != null) toggleStep(stepFoldKey(record));
                    else if (row.attempt !== null) toggleAttempt(sectionKeyOf(record));
                  }}
                >
                  <span role="cell" className={css.event}>
                    {row.sectionStart && !folded && (
                      <span className={css.sectionLabel} title={row.attempt === null ? 'Outside an Attempt' : `Native Attempt ${row.attempt}`} data-active={row.section === allRows.find(candidate => candidate.record.id === selectedId)?.section || undefined}>
                        {row.attempt === null ? 'Unscoped' : sectionLabel(row.attempt, ordinals.get(row.section) ?? 0)}
                      </span>
                    )}
                    {row.attempt !== null && <span className={css.rail} aria-hidden="true" />}
                    {!folded && selectedId === record.id && (
                      <span className={css.selectionRail} aria-hidden="true" />
                    )}
                    {row.requestNumber !== undefined && !folded && (
                      <Tooltip label={`Request ${row.requestNumber} in the loaded window`} side="right">
                        <span
                          className={css.requestMarker}
                          data-status={record.state}
                          role="img"
                          aria-label={`Request ${row.requestNumber} in the loaded window`}
                        />
                      </Tooltip>
                    )}
                    {!folded && (
                      <>
                        {row.groupStart && row.attempt !== null && (
                          <button
                            type="button"
                            className={css.foldToggle}
                            aria-label={`${
                              record.location.step_id == null
                                ? attemptFolded
                                  ? 'Expand'
                                  : 'Fold'
                                : stepFolded
                                  ? 'Expand'
                                  : 'Fold'
                            } ${row.groupLabel}`}
                            aria-expanded={
                              record.location.step_id == null ? !attemptFolded : !stepFolded
                            }
                            onClick={event => {
                              event.stopPropagation();
                              if (record.location.step_id != null) toggleStep(stepFoldKey(record));
                              else toggleAttempt(sectionKeyOf(record));
                            }}
                          >
                            {(record.location.step_id == null ? attemptFolded : stepFolded) ? '▸' : '▾'}
                          </button>
                        )}
                        <Tooltip label={cellLabel[record.kind]} side="right">
                          <span className={css.kindTag} data-kind={record.kind}>
                            <span className={css.kindIcon} aria-hidden="true"><CellIcon kind={record.kind} /></span>
                            <span className={css.kindLabel}>{structural ? row.groupLabel : cellLabel[record.kind]}</span>
                            {cellNarrowLabel[record.kind] !== undefined && (
                              // The row's own aria-label already names the
                              // kind, so this narrow-width discriminator is
                              // visual only and must not be announced twice.
                              <span className={css.kindShort} aria-hidden="true">
                                {cellNarrowLabel[record.kind]}
                              </span>
                            )}
                          </span>
                        </Tooltip>
                      </>
                    )}
                  </span>
                  <div role="cell" className={css.content} title={previewOf(record)}>
                    {folded ? (
                      <button
                        type="button"
                        className={css.collapsed}
                        onClick={event => {
                          event.stopPropagation();
                          if (row.collapsedKind === 'step') toggleStep(stepFoldKey(record));
                          else toggleAttempt(sectionKeyOf(record));
                        }}
                      >
                        <span aria-hidden="true">…</span> {row.collapsedSummary}
                      </button>
                    ) : (
                      <>
                        <CellContent record={record} />
                      </>
                    )}
                  </div>
                  <span role="cell" className={css.trailing}>
                    {!folded && <>
                      <span className={css.state} title={record.state} aria-label={`State: ${record.state}`}>
                        {record.state === 'completed' ? '✓' : record.state === 'running' ? '◌' : record.state}
                      </span>
                      <span className={css.duration} title="Recorded Journal duration">{durationOf(record)}</span>
                    </>}
                  </span>
                </div>
              );
            })}
          </div>
          {rows.length === 0 && (
            <p className={css.note}>
              {searchMatches === null
                ? 'No records in the loaded window.'
                : 'No loaded record matches this search. Load earlier records to search further back.'}
            </p>
          )}
        </div>
        {selected && (
          <TrajectoryInspector
            key={selected.id}
            record={selected}
            detail={selectedDetail?.detail}
            loading={selectedDetail?.loading}
            error={selectedDetail?.error}
            onLoadDetail={onLoadDetail}
            onClose={() => select(undefined)}
          />
        )}
      </div>
    </section>
  );
}
