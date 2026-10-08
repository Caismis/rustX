import { useTranslation } from '../../locale/react';
/* Copyright (c) 2026 DeepSeek. MIT. Adapted from pinned Harness ui-trajectory/TrajectoryTable.tsx and TrajectoryToolbar.tsx; see PROVENANCE.md. */
import { useCallback, useLayoutEffect, useMemo, useRef, useState, type CSSProperties } from 'react';
import { TRACE_LIMIT, type TraceCache } from '../../client/trace';
import { IconSearchOutline16 } from '../../presentation/primitives/icons';
import { TrajectoryInspector, inspectorTabs } from './TrajectoryInspector';
import { TrajectoryLedger, type LedgerHandle } from './TrajectoryLedger';
import type { TrajectoryRowActions } from './TrajectoryRow';
import { TrajectoryTimeline } from './TrajectoryTimeline';
import { projectTrajectory, trajectoryItems, matchingCalls, matchedRecordIds, visibleItems, displayUniverse, preferredItem, selectionOf, ledgerRows, rowOwnsKey, isInspectable, type InspectableDisplayItem, type StructuralDisplayItem, type TrajectoryFacet, type TrajectorySelection } from './layout';
import { searchItems } from './search';
import { timelineFocus, timelineProjectionRevision, trajectoryTimeline, type TrajectoryTimeRange, type TrajectoryTimelineMode } from './timeline';
import css from './Trajectory.module.css';

const DETAILS_MIN_WIDTH = 320;
const DETAILS_MAX_WIDTH = 720;
const TABLE_MIN_WIDTH = 280;
const DETAILS_RESIZE_STEP = 16;
const TOOL_REQUEST_SHARE = 0.58;
const TOOL_REQUEST_MIN_WIDTH = 180;
const TOOL_REQUEST_MAX_WIDTH = 480;
const DEFAULT_TOOL_REQUEST_SHARE = 0.36;
const DEFAULT_TOOL_REQUEST_OFFSET = 56;

function clampDetailsWidth(width: number, splitWidth: number): number {
  const maxWidth = Math.max(DETAILS_MIN_WIDTH, Math.min(DETAILS_MAX_WIDTH, splitWidth - TABLE_MIN_WIDTH));
  return Math.round(Math.min(Math.max(width, DETAILS_MIN_WIDTH), maxWidth));
}

function defaultToolRequestWidth(splitWidth: number): number {
  return Math.min(Math.max(splitWidth * DEFAULT_TOOL_REQUEST_SHARE - DEFAULT_TOOL_REQUEST_OFFSET, TOOL_REQUEST_MIN_WIDTH), TOOL_REQUEST_MAX_WIDTH);
}

export interface TrajectoryProps {
  cache: TraceCache;
  loadEarlier: () => void;
  onSelect: (id?: string) => void;
  onLoadDetail: (id: string) => void;
}

/** Native owner selection stays in the read cache; display/facet lives locally. */
export function Trajectory({ cache, loadEarlier, onSelect, onLoadDetail }: TrajectoryProps) {
  const tx = useTranslation();
  const [query, setQuery] = useState('');
  const [mode, setMode] = useState<TrajectoryTimelineMode>('sequence');
  const [collapsedTurns, setCollapsedTurns] = useState<ReadonlySet<string>>(new Set());
  const [calls, setCalls] = useState<ReadonlySet<string>>(new Set());
  const [selection, setSelection] = useState<TrajectorySelection | undefined>(() => {
    const item = cache.selection ? preferredItem(trajectoryItems(tx, projectTrajectory(tx, cache.page.records)), cache.selection.id) : undefined;
    return item ? selectionOf(item) : undefined;
  });
  const [focus, setFocus] = useState<{ epoch: number; ids: ReadonlySet<string>; range: TrajectoryTimeRange; revision: string } | null>(null);
  const [detailsWidth, setDetailsWidth] = useState<number | null>(null);
  const [toolRequestOffset, setToolRequestOffset] = useState<number | null>(null);
  const resize = useRef<{ pointerId: number; startX: number; startWidth: number; splitWidth: number; startOffset: number } | null>(null);
  // As in Harness, a newly selected record reopens the most recently used tab it has.
  const tabHistory = useRef<TrajectoryFacet[]>(['overview']);
  const ledger = useRef<LedgerHandle>(null);
  const focusedDisplay = useRef<string | undefined>(undefined);
  const [pendingFocus, setPendingFocus] = useState<string | undefined>();
  const records = cache.page.records;
  const projection = useMemo(() => projectTrajectory(tx, records), [tx, records]);
  const allItems = useMemo(() => trajectoryItems(tx, projection, cache.page.next_cursor), [tx, projection, cache.page.next_cursor]);
  const matches = useMemo(() => searchItems(projection, query), [projection, query]);
  const visible = useMemo(() => visibleItems(tx, allItems, records, calls, matches), [tx, allItems, records, calls, matches]);
  const rows = useMemo(() => ledgerRows(tx, projection, visible, collapsedTurns, matches !== null), [tx, projection, visible, collapsedTurns, matches]);
  const selectionItems = useMemo(() => displayUniverse(allItems, visible), [allItems, visible]);
  const matchingOwners = useMemo(() => matchedRecordIds(allItems, matches), [allItems, matches]);
  const timelineModel = useMemo(() => trajectoryTimeline(tx, projection, mode), [tx, projection, mode]);
  const executions = useMemo(() => matchingCalls(records), [records]);
  // Loaded-window ordinals, like the Turn labels: identity stays native.
  const requestNumbers = useMemo(() => new Map(records.filter(record => record.kind === 'request').map((record, index) => [record.id, index + 1])), [records]);
  const foldableTurns = useMemo(() => new Set(projection.sections.flatMap(section => section.kind === 'turn'
    && allItems.filter(item => isInspectable(item) && item.type !== 'SystemPromptCell' && item.type !== 'RequestBoundary' && item.record.location.attempt_id === section.nativeAttemptId).length > 1
    ? [section.nativeAttemptId] : [])), [projection, allItems]);
  // Timeline focus is valid only within the Trace read domain that created it.
  // Within one epoch (prepend, lifecycle refresh) it persists as native record
  // identities, so renumbered Turns and moved coordinates keep its ownership.
  // A rebase onto a new epoch retires it: no stale set can dim the new domain.
  const focusedIds = focus?.epoch === cache.epoch ? focus.ids : null;
  const revision = timelineProjectionRevision(timelineModel, mode);
  const range = useMemo<TrajectoryTimeRange | null>(() => {
    if (focus?.epoch === cache.epoch && focus.revision === revision) return focus.range;
    const spans = timelineModel?.spans.filter(span => focusedIds?.has(span.ownerId ?? span.id)) ?? [];
    return spans.length ? { start: Math.min(...spans.map(span => span.start)), end: Math.max(...spans.map(span => span.end)) } : null;
  }, [timelineModel, focusedIds, focus, cache.epoch, revision]);
  const setRange = (range: TrajectoryTimeRange | null) => {
    const ids = timelineFocus(timelineModel, range);
    setFocus(ids && range ? { epoch: cache.epoch, ids, range, revision } : null);
  };
  const selectedItem = selection ? preferredItem(selectionItems, selection.owner_record_id, selection) : undefined;
  const selected = selectedItem?.record ?? (cache.selection?.id === selection?.owner_record_id ? cache.selection : undefined);
  const selectedDetail = selection ? cache.details[selection.owner_record_id] : undefined;
  const select = useCallback((item?: InspectableDisplayItem, facet?: TrajectoryFacet) => {
    if (item) {
      const tabs = inspectorTabs(item);
      const recent = facet ?? [...tabHistory.current].reverse().find(tab => tabs.includes(tab));
      setSelection({ ...selectionOf(item), facet: recent ?? (tabs.includes(item.facet) ? item.facet : tabs[0]!) });
      setPendingFocus(item.display_key);
    } else setSelection(undefined);
    onSelect(item?.owner_record_id);
  }, [onSelect]);
  const setFacet = (facet: TrajectoryFacet) => {
    tabHistory.current = [...tabHistory.current.filter(tab => tab !== facet), facet];
    setSelection(current => current ? { ...current, facet } : current);
  };

  // Selection migration only follows semantic regrouping, never a detail reply.
  useLayoutEffect(() => {
    if (selection && selectedItem) {
      const active = document.activeElement as HTMLElement | null;
      if (focusedDisplay.current === selection.display_key && (active === document.body || active?.dataset.displayKey === selection.display_key)) setPendingFocus(selectedItem.display_key);
      if (selection.display_key !== selectedItem.display_key) setSelection({ ...selection, display_key: selectedItem.display_key });
    }
  }, [selection, selectedItem]);
  const activeKey = selection?.display_key;
  const canLoadEarlier = Boolean(cache.page.next_cursor) && !cache.loading && records.length < TRACE_LIMIT;
  const requestOlder = () => ledger.current?.loadEarlier();
  const toggleTurn = (id: string) => setCollapsedTurns(current => { const next = new Set(current); if (!next.delete(id)) next.add(id); return next; });
  const toggleCalls = (id: string) => setCalls(current => { const next = new Set(current); if (!next.delete(id)) next.add(id); return next; });
  const callOwners = useMemo(() => new Set([...executions].flatMap(([owner, tools]) => tools.length ? [owner] : [])), [executions]);
  const close = () => {
    if (selection) setPendingFocus(rows.find(row => rowOwnsKey(row, selection.display_key))?.display_key);
    select();
  };
  // Reveal a record from the inspector or the overview: unfold its Turn and
  // its Assistant's calls, and drop a search or focus that would hide it.
  const open = (item: InspectableDisplayItem, facet?: TrajectoryFacet) => {
    const attempt = item.record.location.attempt_id;
    if (attempt) setCollapsedTurns(current => { const next = new Set(current); next.delete(attempt); return next; });
    setCalls(current => new Set([...current].filter(owner => !executions.get(owner)?.some(record => record.id === item.owner_record_id))));
    if (matches && !matches.has(item.display_key)) setQuery('');
    if (focusedIds && !focusedIds.has(item.owner_record_id)) setRange(null);
    select(item, facet);
  };
  const openRecord = (id: string, facet?: TrajectoryFacet) => {
    const item = allItems.find((candidate): candidate is InspectableDisplayItem => isInspectable(candidate) && candidate.owner_record_id === id
      && (candidate.type === 'RequestBoundary' || candidate.type === 'RecordRow'));
    if (item) open(item, facet);
  };
  const actions: TrajectoryRowActions = { select, toggleTurn, toggleCalls, foldableTurns, callOwners, requestNumbers };
  // Harness locates a detail by its section and group: "Turn 2 · Step 1".
  const location = (() => {
    const attempt = selected?.location.attempt_id;
    if (!attempt) return undefined;
    const step = selected.location.step_id ?? undefined;
    const turn = allItems.find((item): item is StructuralDisplayItem => item.type === 'TurnHeader' && item.attempt_id === attempt);
    const group = allItems.find((item): item is StructuralDisplayItem => item.type === 'GroupHeader' && item.attempt_id === attempt && item.step_id === step);
    return { turn: turn?.label, group: group?.label };
  })();
  const inspector = selected && selection && selectedItem
    ? <TrajectoryInspector key={selection.display_key} item={selectedItem} detail={selectedDetail?.detail} loading={selectedDetail?.loading} error={selectedDetail?.error}
      facet={selection.facet} onFacet={setFacet} onLoadDetail={onLoadDetail} onClose={close} location={location}
      records={records} executions={executions} requestNumbers={requestNumbers} completeHistory={!cache.page.next_cursor} onOpen={openRecord} />
    : null;
  const splitStyle = toolRequestOffset === null ? undefined : { '--trajectory-tool-request-width': `calc(58cqw - ${toolRequestOffset}px)` } as CSSProperties;
  const turnIds = projection.sections.flatMap(section => section.kind === 'turn' ? [section.nativeAttemptId] : []);
  const allTurnsCollapsed = foldableTurns.size > 0 && [...foldableTurns].every(id => collapsedTurns.has(id));
  const allCallsCollapsed = callOwners.size > 0 && [...callOwners].every(id => calls.has(id));
  return <section className={css.root} data-conversation-composer-overlay="" aria-label={tx('trajectory:view.trajectory')} onFocusCapture={event => {
    focusedDisplay.current = (event.target as HTMLElement).closest<HTMLElement>('[data-display-key]')?.dataset.displayKey;
  }}>
    <div className={css.toolbar} role="toolbar" aria-label={tx('trajectory:toolbar.aria')}>
      <div className={css.toolbarActions}>
        <button type="button" className={css.toggle} aria-label={tx('trajectory:toolbar.use-actual-duration')} aria-pressed={mode === 'duration'}
          title={mode === 'duration' ? tx('trajectory:toolbar.use-equal-width') : tx('trajectory:toolbar.use-actual-duration')}
          onClick={() => { setMode(mode === 'duration' ? 'sequence' : 'duration'); setRange(null); }}>
          <svg className={css.toggleIcon} viewBox="0 0 16 16" fill="none" aria-hidden="true"><circle cx="8" cy="8" r="5.25" /><path d="M8 4.75V8l2.25 1.5" /></svg>
          {tx('trajectory:toolbar.duration')}
        </button>
        <button type="button" className={css.action} aria-label={allTurnsCollapsed ? tx('trajectory:toolbar.expand-turns') : tx('trajectory:toolbar.collapse-turns')} aria-pressed={allTurnsCollapsed}
          title={allTurnsCollapsed ? tx('trajectory:toolbar.expand-turns') : tx('trajectory:toolbar.collapse-turns')}
          onClick={() => setCollapsedTurns(allTurnsCollapsed ? new Set() : new Set(turnIds))}>
          <span className={css.actionIcon} aria-hidden="true">{allTurnsCollapsed ? '⊞' : '⊟'}</span>{tx('trajectory:toolbar.turns')}
        </button>
        <button type="button" className={css.action} aria-label={allCallsCollapsed ? tx('trajectory:toolbar.expand-calls') : tx('trajectory:toolbar.collapse-calls')} aria-pressed={allCallsCollapsed}
          title={allCallsCollapsed ? tx('trajectory:toolbar.expand-calls') : tx('trajectory:toolbar.collapse-calls')}
          onClick={() => setCalls(allCallsCollapsed ? new Set() : new Set(callOwners))}>
          <span className={css.actionIcon} aria-hidden="true">{allCallsCollapsed ? '⊞' : '⊟'}</span>{tx('trajectory:toolbar.calls')}
        </button>
      </div>
      <div className={css.search}>
        <IconSearchOutline16 size={11} className={css.searchIcon} />
        <input type="search" className={css.searchInput} aria-label={tx('trajectory:toolbar.search')} value={query} onChange={event => setQuery(event.target.value)} placeholder={tx('trajectory:toolbar.search-placeholder')} />
      </div>
    </div>
    {cache.error && <p role="alert" className={css.error}>{cache.error}</p>}
    {/* Epoch retires read-domain ownership; the Timeline separately fences
        coordinate interactions by its semantic projection revision. */}
    <TrajectoryTimeline key={cache.epoch} model={timelineModel} mode={mode} range={range}
      selectedId={timelineModel?.spans.find(span => span.displayKey === selection?.display_key)?.id ?? null}
      searchMatches={matches === null ? null : new Set(timelineModel?.spans.filter(span => span.displayKey ? matches.has(span.displayKey) : matchingOwners?.has(span.ownerId ?? span.id)).map(span => span.id))}
      onRangeChange={setRange}
      hasEarlierRecords={Boolean(cache.page.next_cursor)} canLoadEarlier={canLoadEarlier} loadingEarlier={cache.loading === true} onLoadEarlier={requestOlder}
      onSelect={id => {
        const span = timelineModel?.spans.find(span => span.id === id);
        const item = span?.displayKey ? allItems.find((item): item is InspectableDisplayItem => item.display_key === span.displayKey && isInspectable(item)) : preferredItem(allItems, id);
        if (item) open(item);
      }}
      onReveal={id => {
        const span = timelineModel?.spans.find(span => span.id === id);
        const key = span?.displayKey ?? preferredItem(allItems, id)?.display_key;
        if (key) ledger.current?.reveal(key);
      }} />
    <div className={css.split} style={splitStyle}>
      <div className={css.ledgerPane}>
        <TrajectoryLedger ref={ledger} rows={rows} first={records[0]?.id} activeKey={activeKey} focusKey={pendingFocus} onFocused={() => setPendingFocus(undefined)}
          folded={collapsedTurns} focusedIds={focusedIds} actions={actions} close={close}
          loadEarlier={loadEarlier} canLoadEarlier={canLoadEarlier} loading={cache.loading === true} searching={matches !== null} />
      </div>
      {inspector && <aside className={css.details} aria-label={tx('trajectory:details.event')} style={detailsWidth === null ? undefined : { width: detailsWidth }}>
        <div className={css.detailsResizeHandle} role="separator" aria-label={tx('trajectory:details.resize')} aria-orientation="vertical" tabIndex={0}
          title={tx('trajectory:details.resize-title')}
          onDoubleClick={() => { setDetailsWidth(null); setToolRequestOffset(null); }}
          onPointerDown={event => {
            if (event.button !== 0) return;
            const details = event.currentTarget.parentElement;
            const split = details?.parentElement;
            if (!details || !split) return;
            const splitWidth = split.getBoundingClientRect().width;
            resize.current = { pointerId: event.pointerId, startX: event.clientX, startWidth: details.getBoundingClientRect().width, splitWidth,
              startOffset: toolRequestOffset ?? splitWidth * TOOL_REQUEST_SHARE - defaultToolRequestWidth(splitWidth) };
            event.currentTarget.setPointerCapture(event.pointerId);
            event.preventDefault();
          }}
          onPointerMove={event => {
            const drag = resize.current;
            if (!drag || drag.pointerId !== event.pointerId) return;
            const width = clampDetailsWidth(drag.startWidth + drag.startX - event.clientX, drag.splitWidth);
            setDetailsWidth(width);
            setToolRequestOffset(drag.startOffset + (width - drag.startWidth) * TOOL_REQUEST_SHARE);
          }}
          onPointerUp={event => {
            if (resize.current?.pointerId !== event.pointerId) return;
            resize.current = null;
            event.currentTarget.releasePointerCapture(event.pointerId);
          }}
          onPointerCancel={() => { resize.current = null; }}
          onKeyDown={event => {
            if (event.key !== 'ArrowLeft' && event.key !== 'ArrowRight') return;
            const details = event.currentTarget.parentElement;
            const split = details?.parentElement;
            if (!details || !split) return;
            const current = details.getBoundingClientRect().width;
            const splitWidth = split.getBoundingClientRect().width;
            const width = clampDetailsWidth(current + (event.key === 'ArrowLeft' ? 1 : -1) * DETAILS_RESIZE_STEP, splitWidth);
            setDetailsWidth(width);
            setToolRequestOffset((toolRequestOffset ?? splitWidth * TOOL_REQUEST_SHARE - defaultToolRequestWidth(splitWidth)) + (width - current) * TOOL_REQUEST_SHARE);
            event.preventDefault();
          }} />
        {inspector}
      </aside>}
    </div>
  </section>;
}
