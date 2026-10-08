/* Copyright (c) 2026 DeepSeek. MIT. Adapted from pinned Harness virtual ledger; see PROVENANCE.md. */
import { useMemo, useImperativeHandle, useLayoutEffect, useEffect, useRef, type Ref } from 'react';
import { useVirtualizer } from '@tanstack/react-virtual';
import { useTranslation } from '../../locale/react';
import { StateDot } from '../../presentation/primitives/StateDot';
import { TrajectoryRow, type TrajectoryRowActions } from './TrajectoryRow';
import { ledgerFocusTargets, rowOwnsKey, type TrajectoryLedgerRow } from './layout';
import css from './Trajectory.module.css';

export interface LedgerHandle {
  loadEarlier: () => void;
  /** Scroll a record's row into view without moving focus or selection. */
  reveal: (key: string) => void;
}
export function TrajectoryLedger({ ref, rows, first, activeKey, focusKey, onFocused, folded, focusedIds, actions, close, loadEarlier, canLoadEarlier, loading, searching }: {
  ref: Ref<LedgerHandle>; rows: TrajectoryLedgerRow[]; first?: string; activeKey?: string; focusKey?: string; onFocused: () => void;
  folded: ReadonlySet<string>; focusedIds: ReadonlySet<string> | null;
  actions: TrajectoryRowActions; close: () => void;
  loadEarlier: () => void; canLoadEarlier: boolean; loading: boolean; searching: boolean;
}) {
  const tx = useTranslation();
  const targets = useMemo(() => ledgerFocusTargets(rows), [rows]);
  // As in Harness, only the Turn that owns the current selection draws its rail.
  const activeTurn = useMemo(() => activeKey === undefined ? undefined : rows.find(row => rowOwnsKey(row, activeKey))?.turn?.attempt_id, [rows, activeKey]);
  const viewport = useRef<HTMLDivElement>(null);
  const followsTail = useRef(true);
  const prepend = useRef<{ first?: string; key: string; offset: number } | null>(null);
  const virtualized = rows.length > 100;
  const virtualizer = useVirtualizer({ count: virtualized ? rows.length : 0, enabled: virtualized,
    getScrollElement: () => viewport.current, estimateSize: index => rows[index]!.height,
    getItemKey: index => rows[index]!.display_key, overscan: 12, initialRect: { width: 800, height: 500 },
    anchorTo: 'end', followOnAppend: 'auto', scrollEndThreshold: 2,
  });
  const reveal = (key: string) => {
    const index = rows.findIndex(row => rowOwnsKey(row, key));
    if (index < 0) return;
    followsTail.current = false;
    if (virtualized) { virtualizer.scrollToIndex(index, { align: 'center', behavior: 'smooth' }); return; }
    Array.from(viewport.current?.querySelectorAll<HTMLElement>('[role="row"][data-display-key]') ?? [])
      .find(node => rowOwnsKey(rows[index]!, node.dataset.displayKey!))?.scrollIntoView({ block: 'center', behavior: 'smooth' });
  };
  const requestOlder = () => {
    if (!canLoadEarlier) return;
    const pane = viewport.current;
    if (pane) {
      const bounds = pane.getBoundingClientRect();
      // Keep the first fully visible row still: a partly hidden seat above it
      // may shrink when its Turn chrome moves to a prepended row.
      const visible = Array.from(pane.querySelectorAll<HTMLElement>('[role="row"][data-display-key]'));
      const node = visible.find(node => node.getBoundingClientRect().top >= bounds.top) ?? visible.find(node => node.getBoundingClientRect().bottom > bounds.top);
      if (node) prepend.current = { first, key: node.dataset.displayKey!, offset: node.getBoundingClientRect().top - bounds.top };
    }
    followsTail.current = false; loadEarlier();
  };
  useImperativeHandle(ref, () => ({ loadEarlier: requestOlder, reveal }));
  useLayoutEffect(() => {
    const pane = viewport.current;
    if (!pane) return;
    const anchor = prepend.current;
    if (anchor && anchor.first !== first) {
      const index = rows.findIndex(row => rowOwnsKey(row, anchor.key));
      if (index >= 0) {
        const node = Array.from(pane.querySelectorAll<HTMLElement>('[data-display-key]')).find(node => node.dataset.displayKey === anchor.key);
        const start = node ? node.getBoundingClientRect().top - pane.getBoundingClientRect().top + pane.scrollTop : rows.slice(0, index).reduce((sum, row) => sum + row.height, 0);
        if (virtualized) virtualizer.scrollToOffset(start - anchor.offset); else pane.scrollTop = start - anchor.offset;
      }
      prepend.current = null; followsTail.current = false;
    } else if (followsTail.current && rows.length) {
      // The native scroll extent includes the composer's CSS clearance.
      pane.scrollTop = pane.scrollHeight;
    }
  }, [first, rows, virtualized, virtualizer]);
  useLayoutEffect(() => {
    const pane = viewport.current;
    if (!pane) return;
    // Observe the content box: composer clearance changes its height even
    // though the full-height ledger's border box stays fixed.
    const observer = new ResizeObserver(() => {
      if (followsTail.current) pane.scrollTop = pane.scrollHeight;
    });
    observer.observe(pane);
    return () => observer.disconnect();
  }, []);
  useEffect(() => {
    if (!focusKey) return;
    const index = rows.findIndex(row => rowOwnsKey(row, focusKey));
    if (index < 0) return;
    followsTail.current = false;
    const node = Array.from(viewport.current?.querySelectorAll<HTMLElement>('[data-display-key]') ?? []).find(node => node.dataset.displayKey === focusKey);
    const bounds = viewport.current?.getBoundingClientRect();
    const box = node?.getBoundingClientRect();
    // scrollTop rounds fractional control offsets; do not replace an exact
    // prepend anchor with row alignment for a subpixel edge difference.
    if (virtualized && (!box || !bounds || box.top < bounds.top - 1 || box.bottom > bounds.bottom + 1)) virtualizer.scrollToIndex(index, { align: 'auto' });
    if (node) { node.focus({ preventScroll: virtualized }); onFocused(); }
  });
  const rendered = virtualized ? virtualizer.getVirtualItems().map(item => ({ row: rows[item.index]!, index: item.index, start: item.start })) : rows.map((row, index) => ({ row, index, start: 0 }));
  return <div ref={viewport} className={css.ledger} data-trajectory-scroll="" role="table" aria-label={tx('trajectory:trajectory.trace-ledger')} aria-rowcount={rows.length} style={{ overflowAnchor: 'none' }} onScroll={event => {
    const pane = event.currentTarget; followsTail.current = pane.scrollHeight - pane.clientHeight - pane.scrollTop <= 2;
  }} onClick={event => { if (event.target === event.currentTarget) close(); }}>
    <div style={{ position: 'relative', ...(virtualized ? { height: virtualizer.getTotalSize() } : {}) }}>
      {rendered.map(({ row, index, start }) => {
        const style = { height: row.height, ...(virtualized ? { position: 'absolute' as const, top: 0, left: 0, width: '100%', transform: `translateY(${start}px)` } : {}) };
        if (row.kind === 'history') return <div key={row.display_key} data-display-key={row.display_key} data-history-load="" className={css.loadRow} style={style}>
          <button type="button" className={css.historyLoadButton} disabled={loading || !canLoadEarlier} aria-label={loading ? tx('trajectory:history.loading-earlier') : tx('trajectory:history.load-earlier')} onClick={requestOlder}>
            {loading && <StateDot state="ongoing" />}
            <span aria-hidden="true">{loading ? tx('trajectory:history.loading-earlier') : tx('trajectory:history.load-earlier')}</span>
          </button>
        </div>;
        return <TrajectoryRow key={row.display_key} {...{ row, index, style, activeKey, activeTurn, folded, focusedIds, actions }} onKeyDown={(event, key) => {
          if (event.key === 'Escape') { event.stopPropagation(); close(); return; }
          const current = targets.findIndex(target => target.display_key === key);
          if (current < 0) return;
          if (event.key === 'Enter' || event.key === ' ') { event.preventDefault(); event.stopPropagation(); actions.select(targets[current]!.item); }
          if (event.key === 'ArrowDown' || event.key === 'ArrowUp') {
            event.preventDefault(); event.stopPropagation();
            const next = targets[current + (event.key === 'ArrowDown' ? 1 : -1)];
            if (next) actions.select(next.item);
          }
        }} />;
      })}
    </div>
    {!rows.length && <p className={css.note}>{searching ? tx('trajectory:trajectory.no-loaded-item-matches-this-search') : tx('trajectory:trajectory.no-records-in-the-loaded-window')}</p>}
  </div>;
}
