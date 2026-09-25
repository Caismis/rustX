/* Copyright (c) 2026 DeepSeek. MIT. Adapted from pinned Harness ui-trajectory/TrajectoryTable.tsx and TrajectoryToolbar.tsx; see PROVENANCE.md. */
import { useCallback, useLayoutEffect, useMemo, useRef, useState } from 'react';
import { defaultRangeExtractor, useVirtualizer } from '@tanstack/react-virtual';
import { Group, Panel, Separator } from 'react-resizable-panels';
import { TRACE_LIMIT, type TraceCache } from '../../client/trace';
import { Button } from '../../presentation/primitives/Button';
import { Input } from '../../presentation/primitives/Input';
import { TrajectoryInspector } from './TrajectoryInspector';
import { CellContent, CellIcon, SystemPromptCell } from './TrajectoryCell';
import { TrajectoryTimeline } from './TrajectoryTimeline';
import { projectTrajectory, trajectoryItems, matchingCalls, matchedRecordIds, visibleItems, displayUniverse, preferredItem, selectionOf, isInspectable, preferredStructure, preferredDisplayItem, type FocusableDisplayItem, type StructuralDisplayItem, type TrajectoryDisplayItem, type TrajectorySelection } from './layout';
import { searchItems } from './search';
import { timelineFocus, trajectoryTimeline, type TrajectoryTimeRange, type TrajectoryTimelineMode } from './timeline';
import css from './Trajectory.module.css';

const heightOf = (item: TrajectoryDisplayItem) => item.type === 'RequestBoundary' || item.type === 'CollapsedCallSummary' ? 20 : 30;
export interface TrajectoryProps {
  cache: TraceCache;
  loadEarlier: () => void;
  latest: () => void;
  onSelect: (id?: string) => void;
  onLoadDetail: (id: string) => void;
}

/** Native owner selection stays in the read cache; display/facet lives locally. */
export function Trajectory({ cache, loadEarlier, latest, onSelect, onLoadDetail }: TrajectoryProps) {
  const [query, setQuery] = useState('');
  const [mode, setMode] = useState<TrajectoryTimelineMode>('sequence');
  const [collapsedTurns, setCollapsedTurns] = useState<ReadonlySet<string>>(new Set());
  const [calls, setCalls] = useState<ReadonlySet<string>>(new Set());
  const [selection, setSelection] = useState<TrajectorySelection | undefined>(() => {
    const item = cache.selection ? preferredItem(trajectoryItems(projectTrajectory(cache.page.records)), cache.selection.id) : undefined;
    return item ? selectionOf(item) : undefined;
  });
  const [structure, setStructure] = useState<StructuralDisplayItem | undefined>(undefined);
  const [focusIds, setFocusIds] = useState<ReadonlySet<string> | null>(null);
  const [width, setWidth] = useState(0);
  const [offTail, setOffTail] = useState(false);
  const root = useRef<HTMLElement>(null);
  const viewport = useRef<HTMLDivElement>(null);
  const followsTail = useRef(true);
  const focusedDisplay = useRef<string | undefined>(undefined);
  const pendingFocus = useRef<string | undefined>(undefined);
  const prepend = useRef<{ first: string | undefined; item: FocusableDisplayItem; offset: number } | null>(null);
  const records = cache.page.records;
  const projection = useMemo(() => projectTrajectory(records), [records]);
  const allItems = useMemo(() => trajectoryItems(projection, cache.page.next_cursor), [projection, cache.page.next_cursor]);
  const matches = useMemo(() => searchItems(projection, query), [projection, query]);
  const rows = useMemo(() => visibleItems(allItems, records, collapsedTurns, calls, matches), [allItems, records, collapsedTurns, calls, matches]);
  const selectionItems = useMemo(() => displayUniverse(allItems, rows), [allItems, rows]);
  const matchingOwners = useMemo(() => matchedRecordIds(allItems, matches), [allItems, matches]);
  const timelineModel = useMemo(() => trajectoryTimeline(projection, mode), [projection, mode]);
  // Drag selection persists as native record identities, not sequence positions.
  // Prepending history changes the displayed range without moving its ownership.
  const focusedIds = focusIds;
  const range = useMemo<TrajectoryTimeRange | null>(() => {
    const spans = timelineModel?.spans.filter(span => focusIds?.has(span.id)) ?? [];
    return spans.length ? { start: Math.min(...spans.map(span => span.start)), end: Math.max(...spans.map(span => span.end)) } : null;
  }, [timelineModel, focusIds]);
  const setRange = (range: TrajectoryTimeRange | null) => setFocusIds(timelineFocus(timelineModel, range));
  const virtualized = rows.length > 100;
  const activeStickyIndex = useRef<number | undefined>(undefined);
  const stickyIndexes = useMemo(() => rows.flatMap((row, index) => row.type === 'TurnHeader' ? [index] : []), [rows]);
  const virtualizer = useVirtualizer({
    count: virtualized ? rows.length : 0, enabled: virtualized,
    getScrollElement: () => viewport.current,
    estimateSize: index => heightOf(rows[index]!),
    getItemKey: index => rows[index]!.display_key,
    rangeExtractor: range => {
      const row = rows[range.startIndex];
      const attempt = row && row.type !== 'HistoryBoundary' ? isInspectable(row) ? row.record.location.attempt_id : row.attempt_id : undefined;
      activeStickyIndex.current = stickyIndexes.find(index => {
        const header = rows[index];
        return header?.type === 'TurnHeader' && header.attempt_id === attempt;
      });
      return [...new Set([...(activeStickyIndex.current === undefined ? [] : [activeStickyIndex.current]), ...defaultRangeExtractor(range)])].sort((a, b) => a - b);
    },
    overscan: 12, initialRect: { width: 800, height: 500 },
    anchorTo: 'end', followOnAppend: 'auto', scrollEndThreshold: 2,
  });
  const selectedItem = selection ? preferredItem(selectionItems, selection.owner_record_id, selection) : undefined;
  const selected = selectedItem?.record ?? (cache.selection?.id === selection?.owner_record_id ? cache.selection : undefined);
  const selectedDetail = selection ? cache.details[selection.owner_record_id] : undefined;
  const select = useCallback((item?: FocusableDisplayItem) => {
    const inspectable = item && isInspectable(item) ? item : undefined;
    setStructure(item && !isInspectable(item) ? item : undefined);
    setSelection(inspectable ? selectionOf(inspectable) : undefined);
    onSelect(inspectable?.owner_record_id);
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
      if (focusedDisplay.current === selection.display_key && (active === document.body || active?.dataset.displayKey === selection.display_key)) pendingFocus.current = selectedItem.display_key;
      if (selection.display_key !== selectedItem.display_key) setSelection({ ...selection, display_key: selectedItem.display_key });
    }
  }, [selection, selectedItem, virtualized]);
  // Structural regrouping stays within the containing native Attempt/Step.
  // Neither a late detail response nor a new loaded anchor supplies an owner.
  useLayoutEffect(() => {
    if (!structure) return;
    const next = preferredStructure(allItems, structure);
    const active = document.activeElement as HTMLElement | null;
    if (next?.display_key === structure.display_key && active !== document.body) return;
    if (next && focusedDisplay.current === structure.display_key && (active === document.body || active?.closest<HTMLElement>('[data-display-key]')?.dataset.displayKey === structure.display_key)) pendingFocus.current = next.display_key;
    setStructure(next);
  }, [allItems, structure, virtualized]);
  const activeKey = structure?.display_key ?? selection?.display_key;
  useLayoutEffect(() => {
    const key = pendingFocus.current;
    if (!key) return;
    const index = rows.findIndex(item => item.display_key === key);
    if (index < 0) return;
    if (virtualized) virtualizer.scrollToIndex(index, { align: 'auto' });
    const node = Array.from(viewport.current?.querySelectorAll<HTMLElement>('[data-display-key]') ?? []).find(node => node.dataset.displayKey === key);
    if (node) { node.focus({ preventScroll: virtualized }); pendingFocus.current = undefined; }
  });
  const first = records[0]?.id;
  useLayoutEffect(() => {
    const pane = viewport.current;
    if (!pane) return;
    const anchor = prepend.current;
    if (anchor && anchor.first !== first) {
      const target = preferredDisplayItem(rows, anchor.item);
      if (target) {
        const index = rows.indexOf(target);
        const start = rows.slice(0, index).reduce((sum, item) => sum + heightOf(item), 0);
        // Transfer one semantic anchor across threshold/header changes; normal
        // virtual scroll mechanics and keyed measurement remain TanStack's.
        if (virtualized) virtualizer.scrollToOffset(start - anchor.offset);
        else pane.scrollTop = start - anchor.offset;
      }
      prepend.current = null;
      followsTail.current = false;
    } else if (followsTail.current && rows.length) {
      if (virtualized) virtualizer.scrollToIndex(rows.length - 1, { align: 'end' });
      else pane.scrollTop = pane.scrollHeight;
    }
  }, [first, rows.length, virtualized, virtualizer]);

  const canLoadEarlier = Boolean(cache.page.next_cursor) && !cache.loading && records.length < TRACE_LIMIT;
  const requestOlder = () => {
    if (!canLoadEarlier) return;
    const pane = viewport.current;
    if (pane) {
      const bounds = pane.getBoundingClientRect();
      const node = Array.from(pane.querySelectorAll<HTMLElement>('[data-display-key]')).find(node => node.dataset.displayType && node.dataset.displayType !== 'TurnHeader' && node.getBoundingClientRect().bottom > bounds.top);
      const item = node ? rows.find((item): item is FocusableDisplayItem => item.type !== 'HistoryBoundary' && item.display_key === node.dataset.displayKey) : undefined;
      if (node && item) prepend.current = { first, item, offset: node.getBoundingClientRect().top - bounds.top };
    }
    followsTail.current = false;
    loadEarlier();
  };
  const toggleTurn = (id: string) => setCollapsedTurns(current => { const next = new Set(current); if (!next.delete(id)) next.add(id); return next; });
  const toggleCalls = (id: string) => setCalls(current => { const next = new Set(current); if (!next.delete(id)) next.add(id); return next; });
  const turnIds = projection.sections.flatMap(section => section.kind === 'turn' ? [section.nativeAttemptId] : []);
  const callOwners = records.filter(record => record.kind === 'assistant' && record.calls.length).map(record => record.id);
  const narrow = width < 720;
  const rendered = virtualized ? virtualizer.getVirtualItems().map(item => ({ row: rows[item.index]!, index: item.index, start: item.start })) : rows.map((row, index) => ({ row, index, start: 0 }));
  // Nonvirtual Turns need a containing block so sticky headers stop at the
  // outside-record boundary. Virtual rows share one measured canvas instead.
  const renderedGroups = virtualized ? [rendered] : rendered.reduce<(typeof rendered)[]>((groups, entry) => {
    const outside = entry.row.type === 'HistoryBoundary' || (isInspectable(entry.row) && entry.row.record.location.attempt_id == null);
    if (!groups.length || entry.row.type === 'TurnHeader' || outside) groups.push([]);
    groups.at(-1)!.push(entry);
    return groups;
  }, []);
  const close = () => {
    if (selection) {
      const target = rows.find(row => row.display_key === selection.display_key)
        ?? rows.find(row => row.type === 'TurnHeader' && row.attempt_id === selected?.location.attempt_id);
      pendingFocus.current = target?.display_key;
    }
    select();
  };
  return <section ref={root} className={css.root} aria-label="Trajectory" onFocusCapture={event => {
    focusedDisplay.current = (event.target as HTMLElement).closest<HTMLElement>('[data-display-key]')?.dataset.displayKey;
  }}>
    <div className={css.toolbar} role="toolbar" aria-label="Trajectory controls">
      <Button size="sm" aria-pressed={mode === 'duration' || mode === 'actual'} onClick={() => { setMode(mode === 'duration' ? 'sequence' : mode === 'actual' ? 'time' : mode === 'time' ? 'actual' : 'duration'); setRange(null); }}>Duration</Button>
      <Button size="sm" aria-pressed={mode === 'time' || mode === 'actual'} onClick={() => { setMode(mode === 'actual' ? 'duration' : mode === 'time' ? 'sequence' : mode === 'duration' ? 'actual' : 'time'); setRange(null); }}>Actual time</Button>
      <Button size="sm" aria-label={collapsedTurns.size ? 'Expand Turns' : 'Fold Turns'} aria-pressed={collapsedTurns.size > 0} onClick={() => setCollapsedTurns(collapsedTurns.size ? new Set() : new Set(turnIds))}>Turns</Button>
      <Button size="sm" aria-label={calls.size ? 'Expand Calls' : 'Collapse Calls'} aria-pressed={calls.size > 0} onClick={() => setCalls(calls.size ? new Set() : new Set(callOwners))}>Calls</Button>
      <Input className={css.search} aria-label="Search loaded Trace" value={query} onChange={event => setQuery(event.target.value)} placeholder="Search loaded history" />
      {offTail && <Button size="sm" onClick={() => { followsTail.current = true; setOffTail(false); latest(); if (virtualized) virtualizer.scrollToIndex(rows.length - 1, { align: 'end' }); else if (viewport.current) viewport.current.scrollTop = viewport.current.scrollHeight; }}>Jump to latest</Button>}
    </div>
    {cache.error && <p role="alert" className={css.error}>{cache.error}</p>}
    <TrajectoryTimeline projection={projection} mode={mode} range={range} selectedId={selection?.owner_record_id ?? null} searchMatches={matchingOwners} onRangeChange={setRange}
      hasEarlierRecords={Boolean(cache.page.next_cursor)} canLoadEarlier={canLoadEarlier} loadingEarlier={cache.loading === true} onLoadEarlier={requestOlder}
      onSelect={id => { const item = preferredItem(allItems, id); if (!item) return; setCollapsedTurns(current => { const next = new Set(current); if (item.record.location.attempt_id) next.delete(item.record.location.attempt_id); return next; }); setCalls(current => { const matching = matchingCalls(records); return new Set([...current].filter(owner => !matching.get(owner)?.some(record => record.id === item.owner_record_id))); }); if (matches && !matches.has(item.display_key)) setQuery(''); followsTail.current = false; pendingFocus.current = item.display_key; select(item); }} />
    <Group className={css.split} orientation={narrow ? 'vertical' : 'horizontal'}>
      <Panel id="ledger" minSize={narrow ? '160px' : '340px'} className={css.ledgerPanel}>
        <div className={css.columns} aria-hidden="true"><span>Event</span><span>Content</span></div>
        <div ref={viewport} className={css.ledger} data-trajectory-scroll="" role="table" aria-label="Trace ledger" aria-rowcount={rows.length} style={{ overflowAnchor: 'none' }} onScroll={event => {
          const pane = event.currentTarget;
          followsTail.current = pane.scrollHeight - pane.clientHeight - pane.scrollTop <= 2;
          setOffTail(!followsTail.current);
        }}>
          <div style={virtualized ? { height: virtualizer.getTotalSize(), position: 'relative' } : { position: 'relative' }}>
            {renderedGroups.map(group => <div key={virtualized ? 'virtual-canvas' : group[0]?.row.display_key} style={virtualized ? { height: '100%' } : undefined}>{group.map(({ row, index, start }) => {
              const style = { height: heightOf(row), ...(virtualized ? { position: index === activeStickyIndex.current ? 'sticky' as const : 'absolute' as const, top: 0, left: 0, width: '100%', ...(index === activeStickyIndex.current ? {} : { transform: `translateY(${start}px)` }) } : {}) };
              if (row.type === 'HistoryBoundary') return <div key={row.display_key} data-display-key={row.display_key} className={css.loadRow} style={style}><Button size="sm" disabled={!canLoadEarlier} onClick={requestOlder}>{cache.loading ? 'Loading earlier records…' : 'Load earlier records'}</Button></div>;
              const activate = () => select(row);
              const onKeyDown = (event: React.KeyboardEvent<HTMLDivElement>) => {
                if (event.target !== event.currentTarget) return;
                if (event.key === 'Enter' || event.key === ' ') { event.preventDefault(); activate(); }
                if (event.key === 'Escape') close();
                if (event.key === 'ArrowDown' || event.key === 'ArrowUp') { event.preventDefault(); const next = rows[index + (event.key === 'ArrowDown' ? 1 : -1)]; if (next && next.type !== 'HistoryBoundary') { pendingFocus.current = next.display_key; select(next); } }
              };
              if (!isInspectable(row)) return <div key={row.display_key} data-display-key={row.display_key} data-display-type={row.type} data-anchor={row.anchor_record_id} data-attempt={row.attempt_id} data-step={row.type === 'GroupHeader' ? row.step_id : undefined} data-structural="true" data-selected={activeKey === row.display_key || undefined}
                role="row" aria-rowindex={index + 1} aria-selected={activeKey === row.display_key} aria-label={row.label} tabIndex={0} className={css.record} style={style} onClick={activate} onFocus={event => { if (event.target === event.currentTarget && structure?.display_key !== row.display_key) activate(); }} onKeyDown={onKeyDown} title={row.type === 'GroupHeader' ? row.label : `${row.label} · loaded-window ordinal`}>
                <span role="cell" className={css.event}>
                  {row.type === 'TurnHeader' && <button className={css.foldToggle} aria-label={`${collapsedTurns.has(row.attempt_id) ? 'Expand' : 'Fold'} ${row.label}`} onClick={event => { event.stopPropagation(); toggleTurn(row.attempt_id); }}>{collapsedTurns.has(row.attempt_id) ? '▸' : '▾'}</button>}
                  <span className={css.kindTag} data-kind={row.type === 'GroupHeader' ? 'step' : 'attempt'}>{row.label}</span>
                </span>
                <div role="cell" className={css.content}>
                  {row.type === 'TurnHeader' && collapsedTurns.has(row.attempt_id) && <span className={css.preview}>{records.filter(r => r.location.attempt_id === row.attempt_id && r.state !== 'completed').map(r => r.state).join(' · ')}</span>}
                  {row.type === 'TurnHeader' && row.native_record && row.native_record.state !== 'completed' && <span className={css.state}>{row.native_record.state}</span>}
                </div>
              </div>;
              const record = row.record;
              const warning = record.state !== 'completed' && (row.type === 'RecordRow' || row.type === 'RequestBoundary');
              const truncated = row.type === 'ContextRow' ? row.context.truncated || row.context.preview?.truncated : row.type === 'SystemPromptCell' ? record.request?.system_prompt.preview?.truncated : record.truncated || record.preview?.truncated;
              return <div key={row.display_key} data-display-key={row.display_key} data-owner={row.owner_record_id} data-trace-id={record.id} data-display-type={row.type} data-kind={record.kind} data-state={record.state} data-selected={activeKey === row.display_key || undefined} data-timeline-focus={focusedIds ? focusedIds.has(record.id) ? 'inside' : 'outside' : undefined}
                role="row" aria-rowindex={index + 1} aria-selected={activeKey === row.display_key} aria-label={`${row.label} · ${row.preview || record.state}`} tabIndex={0} className={css.record} style={style} onClick={activate} onKeyDown={onKeyDown}>
                <span role="cell" className={css.event}>
                  <span className={css.kindTag} data-kind={row.type === 'SystemPromptCell' ? 'system' : row.type === 'ContextRow' ? 'context' : record.kind}>
                    {row.type === 'RecordRow' && <span className={css.kindIcon}><CellIcon kind={record.kind} /></span>}
                    <span>{row.type === 'SystemPromptCell' ? 'SYSTEM' : row.type === 'ContextRow' ? 'CONTEXT' : row.label}</span>
                  </span>
                </span>
                <div role="cell" className={css.content}>
                  {row.type === 'SystemPromptCell' ? <SystemPromptCell cell={row} /> : row.type === 'RecordRow' ? <CellContent record={record} /> : <span className={css.preview}>{row.type === 'ContextRow' ? `${row.label} · ${row.preview || 'Empty'}` : row.preview}</span>}
                  {row.type === 'RequestBoundary' && (record.request?.retry_number ?? 0) > 0 && <span className={css.relation}>retry / recovery {record.request?.retry_number}</span>}
                  {row.type === 'CollapsedCallSummary' && <button className={css.collapsed} onClick={event => { event.stopPropagation(); toggleCalls(record.id); }}>Expand Calls</button>}
                  {row.type === 'RecordRow' && record.calls.length > 0 && <button className={css.collapsed} onClick={event => { event.stopPropagation(); toggleCalls(record.id); }}>{calls.has(record.id) ? 'Expand' : 'Collapse'} Calls</button>}
                  {warning && <span className={css.state}>{record.state}</span>}
                  {truncated && <span className={css.state}>Truncated</span>}
                  {row.type === 'RequestBoundary' && record.request?.context_truncated && <span className={css.state}>Context history truncated</span>}
                </div>
              </div>;
            })}</div>)}
          </div>
          {!rows.length && <p className={css.note}>{matches ? 'No loaded item matches this search.' : 'No records in the loaded window.'}</p>}
        </div>
      </Panel>
      {selected && selection && <><Separator className={css.separator} /><Panel id="inspector" minSize={narrow ? '180px' : '320px'} defaultSize={narrow ? '48%' : `${Math.min(440, Math.max(320, width * .38))}px`} maxSize={narrow ? '70%' : `${Math.max(320, width - 346)}px`}>
        <TrajectoryInspector record={selected} detail={selectedDetail?.detail} loading={selectedDetail?.loading} error={selectedDetail?.error} selection={selection} onFacet={facet => setSelection(current => current ? { ...current, facet } : current)} onLoadDetail={onLoadDetail} onClose={close} />
      </Panel></>}
    </Group>
  </section>;
}
