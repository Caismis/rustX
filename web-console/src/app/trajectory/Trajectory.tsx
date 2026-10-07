import { useTranslation } from '../../locale/react';
/* Copyright (c) 2026 DeepSeek. MIT. Adapted from pinned Harness ui-trajectory/TrajectoryTable.tsx and TrajectoryToolbar.tsx; see PROVENANCE.md. */
import { useCallback, useLayoutEffect, useMemo, useRef, useState } from 'react';
import { Group, Panel, Separator } from 'react-resizable-panels';
import { TRACE_LIMIT, type TraceCache } from '../../client/trace';
import { IconSearchOutline16 } from '../../presentation/primitives/icons';
import { TrajectoryInspector, TrajectoryStructureInspector } from './TrajectoryInspector';
import { TrajectoryLedger, type LedgerHandle } from './TrajectoryLedger';
import { TrajectoryTimeline } from './TrajectoryTimeline';
import { projectTrajectory, trajectoryItems, matchingCalls, matchedRecordIds, visibleItems, displayUniverse, preferredItem, selectionOf, isInspectable, preferredStructure, ledgerRows, rowOwnsKey, type FocusableDisplayItem, type StructuralDisplayItem, type TrajectorySelection, type TurnStructure } from './layout';
import { searchItems } from './search';
import { timelineFocus, timelineProjectionRevision, trajectoryTimeline, type TrajectoryTimeRange, type TrajectoryTimelineMode } from './timeline';
import css from './Trajectory.module.css';

export interface TrajectoryProps {
  cache: TraceCache;
  loadEarlier: () => void;
  latest: () => void;
  onSelect: (id?: string) => void;
  onLoadDetail: (id: string) => void;
}

/** Native owner selection stays in the read cache; display/facet lives locally. */
export function Trajectory({ cache, loadEarlier, latest, onSelect, onLoadDetail }: TrajectoryProps) {
  const tx = useTranslation();
  const [query, setQuery] = useState('');
  const [mode, setMode] = useState<TrajectoryTimelineMode>('sequence');
  const [collapsedTurns, setCollapsedTurns] = useState<ReadonlySet<string>>(new Set());
  const [calls, setCalls] = useState<ReadonlySet<string>>(new Set());
  const [selection, setSelection] = useState<TrajectorySelection | undefined>(() => {
    const item = cache.selection ? preferredItem(trajectoryItems(tx, projectTrajectory(tx, cache.page.records)), cache.selection.id) : undefined;
    return item ? selectionOf(item) : undefined;
  });
  const [structure, setStructure] = useState<TurnStructure | undefined>(undefined);
  const [focus, setFocus] = useState<{ epoch: number; ids: ReadonlySet<string>; range: TrajectoryTimeRange; revision: string } | null>(null);
  const [width, setWidth] = useState(0);
  const [offTail, setOffTail] = useState(false);
  const root = useRef<HTMLElement>(null);
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
  const select = useCallback((item?: FocusableDisplayItem) => {
    const inspectable = item && isInspectable(item) ? item : undefined;
    setStructure(item && !isInspectable(item) ? item : undefined);
    setSelection(inspectable ? selectionOf(inspectable) : undefined);
    onSelect(inspectable?.owner_record_id);
    if (item) setPendingFocus(item.display_key);
  }, [onSelect]);
  useLayoutEffect(() => {
    const element = root.current;
    if (!element) return;
    const observer = new ResizeObserver(entries => setWidth(entries[0]?.contentRect.width ?? 0));
    observer.observe(element);
    return () => observer.disconnect();
  }, []);

  // Selection migration only follows semantic regrouping, never a detail reply.
  useLayoutEffect(() => {
    if (selection && selectedItem) {
      const active = document.activeElement as HTMLElement | null;
      if (focusedDisplay.current === selection.display_key && (active === document.body || active?.dataset.displayKey === selection.display_key)) setPendingFocus(selectedItem.display_key);
      if (selection.display_key !== selectedItem.display_key) setSelection({ ...selection, display_key: selectedItem.display_key });
    }
  }, [selection, selectedItem]);
  // Structural regrouping stays within the containing native Attempt.
  // Neither a late detail response nor a new loaded anchor supplies an owner.
  useLayoutEffect(() => {
    if (!structure) return;
    const next = preferredStructure(allItems, structure);
    const active = document.activeElement as HTMLElement | null;
    if (next?.display_key === structure.display_key && active !== document.body) return;
    if (next && focusedDisplay.current === structure.display_key && (active === document.body || active?.closest<HTMLElement>('[data-display-key]')?.dataset.displayKey === structure.display_key)) setPendingFocus(next.display_key);
    setStructure(next);
  }, [allItems, structure]);
  const activeKey = structure?.display_key ?? selection?.display_key;
  // Structural evidence is read from the current projection, so a lifecycle
  // refresh or renumbering of the same native Attempt/Step is never stale.
  const structureItem = structure ? preferredStructure(allItems, structure) : undefined;
  const canLoadEarlier = Boolean(cache.page.next_cursor) && !cache.loading && records.length < TRACE_LIMIT;
  const requestOlder = () => ledger.current?.loadEarlier();
  const toggleTurn = (id: string) => {
    setCollapsedTurns(current => { const next = new Set(current); if (!next.delete(id)) next.add(id); return next; });
    const turn = allItems.find(item => item.type === 'TurnHeader' && item.attempt_id === id);
    if (turn) setPendingFocus(`${turn.display_key}:fold`);
  };
  const toggleCalls = (id: string) => setCalls(current => { const next = new Set(current); if (!next.delete(id)) next.add(id); return next; });
  const turnIds = projection.sections.flatMap(section => section.kind === 'turn' ? [section.nativeAttemptId] : []);
  const callOwners = records.filter(record => record.kind === 'assistant' && record.calls.length).map(record => record.id);
  const narrow = width < 720;
  const close = () => {
    const key = structure?.display_key ?? selection?.display_key;
    if (key) {
      const attempt = structure ? structure.attempt_id : selected?.location.attempt_id;
      const target = rows.find(row => rowOwnsKey(row, key))
        ?? rows.find(row => row.turnStart && row.turn?.attempt_id === attempt);
      setPendingFocus(target && rowOwnsKey(target, key) ? key : target?.turn?.display_key);
    }
    select();
  };
  // Harness locates a detail by its section and group: "Turn 2 · Step 1".
  const location = (() => {
    const attempt = selected?.location.attempt_id;
    if (!attempt) return undefined;
    const step = selected.location.step_id ?? undefined;
    const turn = allItems.find((item): item is StructuralDisplayItem => item.type === 'TurnHeader' && item.attempt_id === attempt);
    const group = allItems.find((item): item is StructuralDisplayItem => item.type === 'GroupHeader' && item.attempt_id === attempt && item.step_id === step);
    return [turn?.label, group?.label].filter(Boolean).join(' · ') || undefined;
  })();
  const inspector = structureItem
    ? <TrajectoryStructureInspector item={structureItem} onClose={close} />
    : selected && selection
      ? <TrajectoryInspector record={selected} detail={selectedDetail?.detail} loading={selectedDetail?.loading} error={selectedDetail?.error} selection={selection} onFacet={facet => setSelection(current => current ? { ...current, facet } : current)} onLoadDetail={onLoadDetail} onClose={close} location={location} />
      : null;
  return <section ref={root} className={css.root} aria-label={tx('trajectory:trajectory.trajectory')} onFocusCapture={event => {
    focusedDisplay.current = (event.target as HTMLElement).closest<HTMLElement>('[data-display-key]')?.dataset.displayKey;
  }}>
    <div className={css.toolbar} role="toolbar" aria-label={tx('trajectory:trajectory.trajectory-controls')}>
      <div className={css.toolbarActions}>
        <button type="button" className={css.toggle} aria-pressed={mode === 'duration' || mode === 'actual'} onClick={() => { setMode(mode === 'duration' ? 'sequence' : mode === 'actual' ? 'time' : mode === 'time' ? 'actual' : 'duration'); setRange(null); }}>
          <svg className={css.toggleIcon} viewBox="0 0 16 16" fill="none" aria-hidden="true"><circle cx="8" cy="8" r="5.25" /><path d="M8 4.75V8l2.25 1.5" /></svg>
          {tx('trajectory:trajectory.duration')}
        </button>
        <button type="button" className={css.control} aria-pressed={mode === 'time' || mode === 'actual'} onClick={() => { setMode(mode === 'actual' ? 'duration' : mode === 'time' ? 'sequence' : mode === 'duration' ? 'actual' : 'time'); setRange(null); }}>
          <span>{tx('trajectory:trajectory.actual-time')}</span>
          <span className={css.controlTrack} data-on={mode === 'time' || mode === 'actual' || undefined} aria-hidden="true"><span className={css.controlThumb} /></span>
        </button>
        <button type="button" className={css.action} aria-label={collapsedTurns.size ? tx('trajectory:copy.expand-turns') : tx('trajectory:copy.fold-turns')} aria-pressed={collapsedTurns.size > 0} onClick={() => setCollapsedTurns(collapsedTurns.size ? new Set() : new Set(turnIds))}>
          <span className={css.actionIcon} aria-hidden="true">{collapsedTurns.size ? '⊞' : '⊟'}</span>{tx('trajectory:copy.turns')}
        </button>
        <button type="button" className={css.action} aria-label={calls.size ? tx('trajectory:trajectory.expand-calls') : tx('trajectory:trajectory.collapse-calls')} aria-pressed={calls.size > 0} onClick={() => setCalls(calls.size ? new Set() : new Set(callOwners))}>
          <span className={css.actionIcon} aria-hidden="true">{calls.size ? '⊞' : '⊟'}</span>{tx('trajectory:trajectory.calls')}
        </button>
        {offTail && <button type="button" className={css.action} onClick={() => { latest(); ledger.current?.latest(); }}>{tx('trajectory:trajectory.jump-to-latest')}</button>}
      </div>
      <div className={css.search}>
        <IconSearchOutline16 size={11} className={css.searchIcon} />
        <input type="search" className={css.searchInput} aria-label={tx('trajectory:trajectory.search-loaded-trace')} value={query} onChange={event => setQuery(event.target.value)} placeholder={tx('trajectory:trajectory.search')} />
      </div>
    </div>
    {cache.error && <p role="alert" className={css.error}>{cache.error}</p>}
    {/* Epoch retires read-domain ownership; the Timeline separately fences
        coordinate interactions by its semantic projection revision. */}
    <TrajectoryTimeline key={cache.epoch} model={timelineModel} mode={mode} range={range} selectedId={timelineModel?.spans.find(span => span.displayKey === selection?.display_key)?.id ?? null} searchMatches={matches === null ? null : new Set(timelineModel?.spans.filter(span => span.displayKey ? matches.has(span.displayKey) : matchingOwners?.has(span.ownerId ?? span.id)).map(span => span.id))} onRangeChange={setRange}
      hasEarlierRecords={Boolean(cache.page.next_cursor)} canLoadEarlier={canLoadEarlier} loadingEarlier={cache.loading === true} onLoadEarlier={requestOlder}
      onSelect={id => { const span = timelineModel?.spans.find(span => span.id === id); const item = span?.displayKey ? allItems.find(item => item.display_key === span.displayKey && isInspectable(item)) : preferredItem(allItems, id); if (!item || !isInspectable(item)) return; setCollapsedTurns(current => { const next = new Set(current); if (item.record.location.attempt_id) next.delete(item.record.location.attempt_id); return next; }); setCalls(current => { const matching = matchingCalls(records); return new Set([...current].filter(owner => !matching.get(owner)?.some(record => record.id === item.owner_record_id))); }); if (matches && !matches.has(item.display_key)) setQuery(''); select(item); }} />
    <Group className={css.split} orientation={narrow ? 'vertical' : 'horizontal'}>
      <Panel id="ledger" minSize={narrow ? '160px' : '340px'} className={css.ledgerPanel}>
        <TrajectoryLedger ref={ledger} rows={rows} first={records[0]?.id} activeKey={activeKey} focusKey={pendingFocus} onFocused={() => setPendingFocus(undefined)}
          folded={collapsedTurns} calls={calls} focusedIds={focusedIds} select={select} toggleTurn={toggleTurn} toggleCalls={toggleCalls} close={close}
          loadEarlier={loadEarlier} canLoadEarlier={canLoadEarlier} loading={cache.loading === true} searching={matches !== null} onOffTail={setOffTail} />
      </Panel>
      {inspector && <><Separator className={css.separator} /><Panel id="inspector" minSize={narrow ? '180px' : '320px'} defaultSize={narrow ? '48%' : `${Math.min(440, Math.max(320, width * .38))}px`} maxSize={narrow ? '70%' : `${Math.max(320, width - 346)}px`}>
        {inspector}
      </Panel></>}
    </Group>
  </section>;
}
