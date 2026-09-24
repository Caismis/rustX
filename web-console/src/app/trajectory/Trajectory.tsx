/* Copyright (c) 2026 DeepSeek. MIT. Adapted from pinned Harness ui-trajectory/TrajectoryTable.tsx and TrajectoryToolbar.tsx; see PROVENANCE.md. */
import { useCallback, useLayoutEffect, useMemo, useRef, useState } from 'react';
import { useVirtualizer } from '@tanstack/react-virtual';
import { Group, Panel, Separator } from 'react-resizable-panels';
import { TRACE_LIMIT, type TraceCache } from '../../client/trace';
import { Button } from '../../presentation/primitives/Button';
import { Input } from '../../presentation/primitives/Input';
import { TrajectoryInspector } from './TrajectoryInspector';
import { CellContent, CellIcon } from './TrajectoryCell';
import { TrajectoryTimeline } from './TrajectoryTimeline';
import { trajectoryItems, visibleItems, preferredItem, selectionOf, type OwnedDisplayItem, type TrajectoryDisplayItem, type TrajectorySelection } from './layout';
import { searchItems } from './search';
import { timelineFocus, trajectoryTimeline, type TrajectoryTimeRange, type TrajectoryTimelineMode } from './timeline';
import css from './Trajectory.module.css';

const heightOf = (item: TrajectoryDisplayItem) => item.type === 'StepHeader' || item.type === 'CollapsedCallSummary' ? 20 : 30;
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
  const [attempts, setAttempts] = useState<ReadonlySet<string>>(new Set());
  const [calls, setCalls] = useState<ReadonlySet<string>>(new Set());
  const [selection, setSelection] = useState<TrajectorySelection | undefined>(() => {
    const item = cache.selection ? preferredItem(trajectoryItems(cache.page.records), cache.selection.id) : undefined;
    return item ? selectionOf(item) : undefined;
  });
  const [range, setRange] = useState<TrajectoryTimeRange | null>(null);
  const [width, setWidth] = useState(0);
  const [offTail, setOffTail] = useState(false);
  const root = useRef<HTMLElement>(null);
  const viewport = useRef<HTMLDivElement>(null);
  const followsTail = useRef(true);
  const focusedDisplay = useRef<string | undefined>(undefined);
  const pendingFocus = useRef<string | undefined>(undefined);
  const prepend = useRef<{ first: string | undefined; selection: TrajectorySelection; offset: number } | null>(null);
  const records = cache.page.records;
  const allItems = useMemo(() => trajectoryItems(records, cache.page.next_cursor), [records, cache.page.next_cursor]);
  const matches = useMemo(() => searchItems(allItems, query), [allItems, query]);
  const rows = useMemo(() => visibleItems(allItems, records, attempts, calls, matches), [allItems, records, attempts, calls, matches]);
  const matchingOwners = useMemo(() => matches ? new Set(allItems.filter((item): item is OwnedDisplayItem => item.type !== 'HistoryBoundary' && matches.has(item.display_key)).map(item => item.owner_record_id)) : null, [allItems, matches]);
  const boundaryLabel = useCallback((_record: unknown, index: number) => {
    const item = allItems.find(item => item.type === 'AttemptSectionHeader' && item.display_key === JSON.stringify(['attempt-section', records[index]?.location.attempt_id, records[index]?.id]));
    return item?.type === 'AttemptSectionHeader' ? item.label : undefined;
  }, [allItems, records]);
  const timelineModel = useMemo(() => trajectoryTimeline(records, mode, boundaryLabel), [records, mode, boundaryLabel]);
  const focusedIds = useMemo(() => timelineFocus(timelineModel, range), [timelineModel, range]);
  const virtualized = rows.length > 100;
  const virtualizer = useVirtualizer({
    count: virtualized ? rows.length : 0, enabled: virtualized,
    getScrollElement: () => viewport.current,
    estimateSize: index => heightOf(rows[index]!),
    getItemKey: index => rows[index]!.display_key,
    overscan: 12, initialRect: { width: 800, height: 500 },
    anchorTo: 'end', followOnAppend: 'auto', scrollEndThreshold: 2,
  });
  const selectedItem = selection ? preferredItem(allItems, selection.owner_record_id, selection) : undefined;
  const selected = selectedItem?.record ?? (cache.selection?.id === selection?.owner_record_id ? cache.selection : undefined);
  const selectedDetail = selection ? cache.details[selection.owner_record_id] : undefined;
  const select = useCallback((item?: OwnedDisplayItem) => {
    setSelection(item ? selectionOf(item) : undefined);
    onSelect(item?.owner_record_id);
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
    if (selection && selectedItem && selection.display_key !== selectedItem.display_key) {
      const active = document.activeElement as HTMLElement | null;
      if (focusedDisplay.current === selection.display_key && (active === document.body || active?.dataset.displayKey === selection.display_key)) pendingFocus.current = selectedItem.display_key;
      setSelection({ ...selection, display_key: selectedItem.display_key });
    }
  }, [selection, selectedItem]);
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
      const target = preferredItem(rows, anchor.selection.owner_record_id, anchor.selection);
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
      const node = Array.from(pane.querySelectorAll<HTMLElement>('[data-display-key]')).find(node => node.dataset.owner && node.getBoundingClientRect().bottom > bounds.top);
      const item = node ? rows.find((item): item is OwnedDisplayItem => item.type !== 'HistoryBoundary' && item.display_key === node.dataset.displayKey) : undefined;
      if (node && item) prepend.current = { first, selection: selectionOf(item), offset: node.getBoundingClientRect().top - bounds.top };
    }
    followsTail.current = false;
    loadEarlier();
  };
  const toggleAttempt = (id: string) => setAttempts(current => { const next = new Set(current); if (!next.delete(id)) next.add(id); return next; });
  const toggleCalls = (id: string) => setCalls(current => { const next = new Set(current); if (!next.delete(id)) next.add(id); return next; });
  const attemptIds = [...new Set(records.flatMap(record => record.location.attempt_id ? [record.location.attempt_id] : []))];
  const callOwners = records.filter(record => record.kind === 'assistant' && record.calls.length).map(record => record.id);
  const narrow = width < 720;
  const rendered = virtualized ? virtualizer.getVirtualItems().map(item => ({ row: rows[item.index]!, index: item.index, start: item.start })) : rows.map((row, index) => ({ row, index, start: 0 }));
  const close = () => { if (selection) pendingFocus.current = selection.display_key; select(); };
  return <section ref={root} className={css.root} aria-label="Trajectory" onFocusCapture={event => {
    focusedDisplay.current = (event.target as HTMLElement).closest<HTMLElement>('[data-display-key]')?.dataset.displayKey;
  }}>
    <div className={css.toolbar} role="toolbar" aria-label="Trajectory controls">
      <Button size="sm" aria-pressed={mode === 'duration' || mode === 'actual'} onClick={() => { setMode(mode === 'duration' ? 'sequence' : mode === 'actual' ? 'time' : mode === 'time' ? 'actual' : 'duration'); setRange(null); }}>Duration</Button>
      <Button size="sm" aria-pressed={mode === 'time' || mode === 'actual'} onClick={() => { setMode(mode === 'actual' ? 'duration' : mode === 'time' ? 'sequence' : mode === 'duration' ? 'actual' : 'time'); setRange(null); }}>Actual time</Button>
      <Button size="sm" aria-label={attempts.size ? 'Expand Attempts' : 'Fold Attempts'} aria-pressed={attempts.size > 0} onClick={() => setAttempts(attempts.size ? new Set() : new Set(attemptIds))}>Attempts</Button>
      <Button size="sm" aria-label={calls.size ? 'Expand Calls' : 'Collapse Calls'} aria-pressed={calls.size > 0} onClick={() => setCalls(calls.size ? new Set() : new Set(callOwners))}>Calls</Button>
      <Input className={css.search} aria-label="Search loaded Trace" value={query} onChange={event => setQuery(event.target.value)} placeholder="Search loaded history" />
      {offTail && <Button size="sm" onClick={() => { followsTail.current = true; setOffTail(false); latest(); if (virtualized) virtualizer.scrollToIndex(rows.length - 1, { align: 'end' }); else if (viewport.current) viewport.current.scrollTop = viewport.current.scrollHeight; }}>Jump to latest</Button>}
    </div>
    {cache.error && <p role="alert" className={css.error}>{cache.error}</p>}
    <TrajectoryTimeline records={records} mode={mode} range={range} selectedId={selection?.owner_record_id ?? null} searchMatches={matchingOwners} onRangeChange={setRange} boundaryLabel={boundaryLabel}
      hasEarlierRecords={Boolean(cache.page.next_cursor)} canLoadEarlier={canLoadEarlier} loadingEarlier={cache.loading === true} onLoadEarlier={requestOlder}
      onSelect={id => { const item = preferredItem(allItems, id); if (!item) return; setAttempts(new Set()); setCalls(new Set()); if (matches && !matches.has(item.display_key)) setQuery(''); followsTail.current = false; pendingFocus.current = item.display_key; select(item); }} />
    <Group className={css.split} orientation={narrow ? 'vertical' : 'horizontal'}>
      <Panel id="ledger" minSize={narrow ? '160px' : '340px'} className={css.ledgerPanel}>
        <div className={css.columns} aria-hidden="true"><span>Event</span><span>Content</span></div>
        <div ref={viewport} className={css.ledger} data-trajectory-scroll="" role="table" aria-label="Trace ledger" aria-rowcount={rows.length} style={{ overflowAnchor: 'none' }} onScroll={event => {
          const pane = event.currentTarget;
          followsTail.current = pane.scrollHeight - pane.clientHeight - pane.scrollTop <= 2;
          setOffTail(!followsTail.current);
        }}>
          <div style={virtualized ? { height: virtualizer.getTotalSize(), position: 'relative' } : { position: 'relative' }}>
            {rendered.map(({ row, index, start }) => {
              const style = { height: heightOf(row), ...(virtualized ? { position: 'absolute' as const, top: 0, left: 0, width: '100%', transform: `translateY(${start}px)` } : {}) };
              if (row.type === 'HistoryBoundary') return <div key={row.display_key} data-display-key={row.display_key} className={css.loadRow} style={style}><Button size="sm" disabled={!canLoadEarlier} onClick={requestOlder}>{cache.loading ? 'Loading earlier records…' : 'Load earlier records'}</Button></div>;
              const record = row.record;
              const structural = row.type === 'AttemptSectionHeader' || row.type === 'StepHeader';
              const warning = record.state !== 'completed' && (row.type === 'RecordRow' || row.type === 'RequestBoundary' || (row.type === 'AttemptSectionHeader' && record.kind === 'attempt'));
              const truncated = row.type === 'ContextRow' ? row.context.truncated || row.context.preview?.truncated : row.type === 'SystemRow' ? record.request?.system_prompt.preview?.truncated : record.truncated || record.preview?.truncated;
              return <div key={row.display_key} data-display-key={row.display_key} data-owner={row.owner_record_id} data-trace-id={record.id} data-display-type={row.type} data-kind={record.kind} data-state={record.state} data-structural={structural || undefined} data-selected={selection?.display_key === row.display_key || undefined} data-timeline-focus={focusedIds ? focusedIds.has(record.id) ? 'inside' : 'outside' : undefined}
                role="row" aria-rowindex={index + 1} aria-selected={selection?.display_key === row.display_key} aria-label={`${row.label} · ${row.preview || record.state}`} tabIndex={0} className={css.record} style={style} onClick={() => select(row)} onKeyDown={event => {
                  if (event.target !== event.currentTarget) return;
                  if (event.key === 'Enter' || event.key === ' ') { event.preventDefault(); select(row); }
                  if (event.key === 'Escape') close();
                  if (event.key === 'ArrowDown' || event.key === 'ArrowUp') { event.preventDefault(); const next = rows[index + (event.key === 'ArrowDown' ? 1 : -1)]; if (next && next.type !== 'HistoryBoundary') { pendingFocus.current = next.display_key; select(next); } }
                }}>
                <span role="cell" className={css.event}>
                  {row.type === 'AttemptSectionHeader' && <button className={css.foldToggle} aria-label={`${attempts.has(record.location.attempt_id!) ? 'Expand' : 'Fold'} ${row.label}`} onClick={event => { event.stopPropagation(); toggleAttempt(record.location.attempt_id!); }}>{attempts.has(record.location.attempt_id!) ? '▸' : '▾'}</button>}
                  <span className={css.kindTag} data-kind={row.type === 'SystemRow' ? 'system' : row.type === 'ContextRow' ? 'context' : record.kind}>
                    {row.type === 'RecordRow' && <span className={css.kindIcon}><CellIcon kind={record.kind} /></span>}
                    <span>{row.type === 'SystemRow' ? 'SYSTEM' : row.type === 'ContextRow' ? 'CONTEXT' : row.label}</span>
                  </span>
                </span>
                <div role="cell" className={css.content}>
                  {row.type === 'RecordRow' ? <CellContent record={record} /> : <span className={css.preview}>{row.type === 'SystemRow' || row.type === 'ContextRow' ? `${row.label} · ${row.preview || 'Empty'}` : row.preview}</span>}
                  {row.type === 'RequestBoundary' && <span className={css.relation}>retry / recovery {record.request?.retry_number}</span>}
                  {row.type === 'CollapsedCallSummary' && <button className={css.collapsed} onClick={event => { event.stopPropagation(); toggleCalls(record.id); }}>Expand Calls</button>}
                  {row.type === 'RecordRow' && record.calls.length > 0 && <button className={css.collapsed} onClick={event => { event.stopPropagation(); toggleCalls(record.id); }}>{calls.has(record.id) ? 'Expand' : 'Collapse'} Calls</button>}
                  {row.type === 'AttemptSectionHeader' && attempts.has(record.location.attempt_id!) && <span className={css.preview}>{records.filter(r => r.location.attempt_id === record.location.attempt_id && r.state !== 'completed').map(r => r.state).join(' · ')}</span>}
                  {warning && <span className={css.state}>{record.state}</span>}
                  {truncated && <span className={css.state}>Truncated</span>}
                  {row.type === 'RequestBoundary' && record.request?.context_truncated && <span className={css.state}>Context history truncated</span>}
                </div>
              </div>;
            })}
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
