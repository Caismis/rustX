/* Copyright (c) 2026 DeepSeek. MIT. Adapted from pinned Harness semantic ledger; see PROVENANCE.md. */
import type { CSSProperties, KeyboardEvent } from 'react';
import { useTranslation } from '../../locale/react';
import { traceStateLabel } from '../../bindings/status-labels';
import { CellContent, CellIcon, cellNarrowLabel, SystemPromptCell } from './TrajectoryCell';
import type { FocusableDisplayItem, TrajectoryLedgerRow } from './layout';
import css from './Trajectory.module.css';

export function TrajectoryRow({ row, index, style, activeKey, activeTurn, folded, calls, focusedIds, select, toggleTurn, toggleCalls, onKeyDown }: {
  row: TrajectoryLedgerRow; index: number; style: CSSProperties; activeKey?: string; activeTurn?: string;
  folded: ReadonlySet<string>; calls: ReadonlySet<string>; focusedIds: ReadonlySet<string> | null;
  select: (item: FocusableDisplayItem) => void; toggleTurn: (id: string) => void; toggleCalls: (id: string) => void;
  onKeyDown: (event: KeyboardEvent<HTMLElement>, key: string) => void;
}) {
  const tx = useTranslation();
  const item = row.item;
  const record = item?.record;
  const shortLabel = record ? cellNarrowLabel(tx)[record.kind] : undefined;
  const truncated = item?.type === 'ContextRow' ? item.context.truncated || item.context.preview?.truncated
    : item?.type === 'SystemPromptCell' ? record?.request?.system_prompt.preview?.truncated
      : record?.truncated || record?.preview?.truncated || record?.tool?.arguments?.truncated || record?.tool?.detail?.truncated;
  const activate = () => { if (item) select(item); else if (row.turn) select(row.turn); };
  return <div data-display-key={row.display_key} data-owner={item?.owner_record_id} data-trace-id={record?.id}
    data-display-type={row.kind === 'structure' ? 'StructuralSeat' : row.kind === 'marker' ? 'MarkerSeat' : row.kind === 'summary' ? 'TurnSummary' : item?.type}
    data-kind={record?.kind} data-state={record?.state} data-attempt={row.turn?.attempt_id}
    data-selected={activeKey === row.display_key || undefined} data-turn-start={row.turnStart || undefined}
    data-timeline-focus={focusedIds && record ? focusedIds.has(record.id) ? 'inside' : 'outside' : undefined}
    role="row" aria-rowindex={index + 1} aria-selected={activeKey === row.display_key}
    aria-label={item ? `${item.label} ${item.preview || traceStateLabel(tx, item.record.state)}` : row.summary ?? row.turn?.label}
    tabIndex={item && item.type !== 'RequestBoundary' ? 0 : -1} className={css.record} style={style} onClick={activate} onKeyDown={event => { if (event.key === 'Escape' || (event.target === event.currentTarget && item)) onKeyDown(event, item?.display_key ?? ''); }}>
    <span role="cell" className={css.event}>
      {row.turn && row.turn.attempt_id === activeTurn && <span className={css.rail} aria-hidden="true" />}
      {row.turnStart && row.turn && <span className={css.turnChrome} data-active={row.turn.attempt_id === activeTurn || undefined}>
        <button data-display-key={`${row.turn.display_key}:fold`} className={css.foldToggle} aria-label={tx('trajectory:trajectory.value-value', { p0: folded.has(row.turn.attempt_id) ? tx('trajectory:trajectory.expand') : tx('trajectory:copy.fold'), p1: row.turn.label })} onClick={event => { event.stopPropagation(); toggleTurn(row.turn!.attempt_id); }}>{folded.has(row.turn.attempt_id) ? '▸' : '▾'}</button>
        <button data-display-key={row.turn.display_key} onKeyDown={event => onKeyDown(event, row.turn!.display_key)} data-structural="turn" aria-pressed={activeKey === row.turn.display_key} data-selected={activeKey === row.turn.display_key || undefined} aria-label={row.turn.label} title={row.turn.attempt_id} onClick={event => { event.stopPropagation(); select(row.turn!); }}><span className={css.turnLabelFull}>{row.turn.label}</span><span className={css.turnLabelCompact}>#{row.turn.ordinal}</span></button>
      </span>}
      {row.kind === 'semantic' && item && <span className={css.kindTag} data-kind={item.type === 'SystemPromptCell' ? 'system' : item.type === 'ContextRow' ? 'context' : record?.kind}>
        {item.type === 'RecordRow' && !shortLabel && <span className={css.kindIcon}><CellIcon kind={item.record.kind} /></span>}
        <span className={css.kindLabel}>{item.type === 'SystemPromptCell' ? tx('trajectory:trajectory.system') : item.type === 'ContextRow' ? tx('trajectory:trajectory.context') : item.label}</span>
        {shortLabel && <span className={css.kindShort}>{shortLabel}</span>}
      </span>}
    </span>
    <div role="cell" className={css.content}>
      {row.stepMarkers.map(step => <button key={step.display_key} className={css.stepChrome} data-display-key={step.display_key} onKeyDown={event => onKeyDown(event, step.display_key)} data-structural="step" aria-pressed={activeKey === step.display_key} data-step={step.step_id} data-selected={activeKey === step.display_key || undefined} aria-label={step.label} title={`${step.step_id} · ${step.native_record ? traceStateLabel(tx, step.native_record.state) : ''}`} onClick={event => { event.stopPropagation(); select(step); }}>{step.label}</button>)}
      {row.request && <button onKeyDown={event => onKeyDown(event, row.request!.display_key)} className={css.requestChrome} data-display-key={row.request.display_key} aria-pressed={activeKey === row.request.display_key} data-request-owner={row.request.owner_record_id} data-request-id={row.request.record.request?.request_id} data-selected={activeKey === row.request.display_key || undefined} data-status={row.request.record.state}
        aria-label={`${row.request.label} ${row.request.record.request?.request_id}`} title={`${row.request.record.request?.model} · ${row.request.record.request?.request_id} · ${traceStateLabel(tx, row.request.record.state)}`}
        onClick={event => { event.stopPropagation(); select(row.request!); }}>{(row.request.record.request?.retry_number ?? 0) > 0 ? row.request.record.request?.retry_number : ''}</button>}
      {row.kind === 'summary' ? <button className={css.collapsed} onClick={event => { event.stopPropagation(); if (row.turn) toggleTurn(row.turn.attempt_id); }}>{row.summary}</button>
        : item?.type === 'SystemPromptCell' ? <SystemPromptCell cell={item} />
          : item?.type === 'RecordRow' ? <CellContent record={item.record} />
            : item && row.kind === 'semantic' ? <span className={css.preview}>{item.type === 'ContextRow' ? `${item.label} · ${item.preview}` : item.preview}</span> : null}
      {item?.type === 'CollapsedCallSummary' && <button className={css.collapsed} onClick={event => { event.stopPropagation(); toggleCalls(item.record.id); }}>{tx('trajectory:trajectory.expand-calls')}</button>}
      {item?.type === 'RecordRow' && item.record.calls.length > 0 && <button className={css.collapsed} onClick={event => { event.stopPropagation(); toggleCalls(item.record.id); }}>{calls.has(item.record.id) ? tx('trajectory:trajectory.expand') : tx('trajectory:trajectory.collapse')} {tx('trajectory:trajectory.calls')}</button>}
      {record && row.kind !== 'marker' && item?.type === 'RecordRow' && record.state !== 'completed' && <span className={css.state}>{traceStateLabel(tx, record.state)}</span>}
      {(truncated || row.request?.record.request?.context_truncated) && <span className={css.state} title={tx('trajectory:trajectory.truncated')} aria-label={tx('trajectory:trajectory.truncated')}>…</span>}
    </div>
  </div>;
}
