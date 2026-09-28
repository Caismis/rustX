/* Copyright (c) 2026 DeepSeek. MIT. Adapted from pinned Harness virtual ledger; see PROVENANCE.md. */
import { useImperativeHandle, useLayoutEffect, useEffect, useRef, type Ref } from 'react';
import { useVirtualizer } from '@tanstack/react-virtual';
import { useTranslation } from '../../locale/react';
import { Button } from '../../presentation/primitives/Button';
import { TrajectoryRow } from './TrajectoryRow';
import { rowOwnsKey, type FocusableDisplayItem, type TrajectoryLedgerRow } from './layout';
import css from './Trajectory.module.css';

export interface LedgerHandle { loadEarlier: () => void; latest: () => void }
export function TrajectoryLedger({ ref, rows, first, activeKey, focusKey, onFocused, folded, calls, focusedIds, select, toggleTurn, toggleCalls, close, loadEarlier, canLoadEarlier, loading, searching, onOffTail }: {
  ref: Ref<LedgerHandle>; rows: TrajectoryLedgerRow[]; first?: string; activeKey?: string; focusKey?: string; onFocused: () => void;
  folded: ReadonlySet<string>; calls: ReadonlySet<string>; focusedIds: ReadonlySet<string> | null;
  select: (item: FocusableDisplayItem) => void; toggleTurn: (id: string) => void; toggleCalls: (id: string) => void; close: () => void;
  loadEarlier: () => void; canLoadEarlier: boolean; loading: boolean; searching: boolean; onOffTail: (off: boolean) => void;
}) {
  const tx = useTranslation();
  const viewport = useRef<HTMLDivElement>(null);
  const followsTail = useRef(true);
  const prepend = useRef<{ first?: string; key: string; offset: number } | null>(null);
  const virtualized = rows.length > 100;
  const virtualizer = useVirtualizer({ count: virtualized ? rows.length : 0, enabled: virtualized,
    getScrollElement: () => viewport.current, estimateSize: index => rows[index]!.height,
    getItemKey: index => rows[index]!.display_key, overscan: 12, initialRect: { width: 800, height: 500 },
    anchorTo: 'end', followOnAppend: 'auto', scrollEndThreshold: 2,
  });
  const latest = () => {
    followsTail.current = true; onOffTail(false);
    if (virtualized && rows.length) virtualizer.scrollToIndex(rows.length - 1, { align: 'end' });
    else if (viewport.current) viewport.current.scrollTop = viewport.current.scrollHeight;
  };
  const requestOlder = () => {
    if (!canLoadEarlier) return;
    const pane = viewport.current;
    if (pane) {
      const bounds = pane.getBoundingClientRect();
      const focused = document.activeElement instanceof HTMLElement && pane.contains(document.activeElement) && document.activeElement.matches('[data-structural]') ? document.activeElement : null;
      const node = focused ?? Array.from(pane.querySelectorAll<HTMLElement>('[role="row"][data-display-key]')).find(node => node.getBoundingClientRect().bottom > bounds.top);
      if (node) prepend.current = { first, key: node.dataset.displayKey!, offset: node.getBoundingClientRect().top - bounds.top };
    }
    followsTail.current = false; loadEarlier();
  };
  useImperativeHandle(ref, () => ({ loadEarlier: requestOlder, latest }));
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
      if (virtualized) virtualizer.scrollToIndex(rows.length - 1, { align: 'end' }); else pane.scrollTop = pane.scrollHeight;
    }
  }, [first, rows, virtualized, virtualizer]);
  useEffect(() => {
    if (!focusKey) return;
    const index = rows.findIndex(row => rowOwnsKey(row, focusKey));
    if (index < 0) return;
    followsTail.current = false;
    const node = Array.from(viewport.current?.querySelectorAll<HTMLElement>('[data-display-key]') ?? []).find(node => node.dataset.displayKey === focusKey);
    const bounds = viewport.current?.getBoundingClientRect();
    const box = node?.getBoundingClientRect();
    if (virtualized && (!box || !bounds || box.top < bounds.top || box.bottom > bounds.bottom)) virtualizer.scrollToIndex(index, { align: 'auto' });
    if (node) { node.focus({ preventScroll: virtualized }); onFocused(); }
  });
  const rendered = virtualized ? virtualizer.getVirtualItems().map(item => ({ row: rows[item.index]!, index: item.index, start: item.start })) : rows.map((row, index) => ({ row, index, start: 0 }));
  return <div ref={viewport} className={css.ledger} data-trajectory-scroll="" role="table" aria-label={tx('trajectory:trajectory.trace-ledger')} aria-rowcount={rows.length} style={{ overflowAnchor: 'none' }} onScroll={event => {
    const pane = event.currentTarget; followsTail.current = pane.scrollHeight - pane.clientHeight - pane.scrollTop <= 2; onOffTail(!followsTail.current);
  }}>
    <div style={{ position: 'relative', ...(virtualized ? { height: virtualizer.getTotalSize() } : {}) }}>
      {rendered.map(({ row, index, start }) => {
        const style = { height: row.height, ...(virtualized ? { position: 'absolute' as const, top: 0, left: 0, width: '100%', transform: `translateY(${start}px)` } : {}) };
        if (row.kind === 'history') return <div key={row.display_key} data-display-key={row.display_key} className={css.loadRow} style={style}><Button size="sm" disabled={!canLoadEarlier} onClick={requestOlder}>{loading ? tx('trajectory:trajectory.loading-earlier-records') : tx('trajectory:trajectory.load-earlier-records')}</Button></div>;
        return <TrajectoryRow key={row.display_key} {...{ row, index, style, activeKey, folded, calls, focusedIds, select, toggleTurn, toggleCalls }} onKeyDown={event => {
          if (event.key === 'Escape') { close(); return; }
          if (event.target !== event.currentTarget) return;
          if (event.key === 'Enter' || event.key === ' ') { event.preventDefault(); const target = row.item ?? row.turn; if (target) select(target); }
          if (event.key === 'ArrowDown' || event.key === 'ArrowUp') { event.preventDefault(); const next = rows[index + (event.key === 'ArrowDown' ? 1 : -1)]; const target = next?.item ?? next?.turn; if (target) select(target); }
        }} />;
      })}
    </div>
    {!rows.length && <p className={css.note}>{searching ? tx('trajectory:trajectory.no-loaded-item-matches-this-search') : tx('trajectory:trajectory.no-records-in-the-loaded-window')}</p>}
  </div>;
}
