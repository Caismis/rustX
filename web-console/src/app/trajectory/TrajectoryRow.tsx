/* Copyright (c) 2026 DeepSeek. MIT. Adapted from pinned Harness TrajectoryTable.tsx ledger rows; see PROVENANCE.md. */
import type { CSSProperties, KeyboardEvent, MouseEvent } from 'react';
import { useTranslation } from '../../locale/react';
import { CellContent, KindTag, cellKind, cellLabel, isErrorRecord, listText } from './TrajectoryCell';
import type { InspectableDisplayItem, TrajectoryLedgerRow } from './layout';
import css from './Trajectory.module.css';

export interface TrajectoryRowActions {
  select: (item: InspectableDisplayItem) => void;
  toggleTurn: (id: string) => void;
  toggleCalls: (id: string) => void;
  /** Turns with more than one content row, the only ones Harness folds. */
  foldableTurns: ReadonlySet<string>;
  /** Assistant records with loaded Tool calls to fold. */
  callOwners: ReadonlySet<string>;
  /** Loaded-window Request ordinals, as the dot names them. */
  requestNumbers: ReadonlyMap<string, number>;
}

export function TrajectoryRow({ row, index, style, activeKey, activeTurn, folded, focusedIds, actions, onKeyDown }: {
  row: TrajectoryLedgerRow; index: number; style: CSSProperties; activeKey?: string; activeTurn?: string;
  folded: ReadonlySet<string>; focusedIds: ReadonlySet<string> | null;
  actions: TrajectoryRowActions;
  onKeyDown: (event: KeyboardEvent<HTMLElement>, key: string) => void;
}) {
  const tx = useTranslation();
  const item = row.item;
  const record = item?.record;
  const summary = row.kind === 'summary';
  // A Request is selected on its dot, never as a row, as in Harness.
  const selected = item !== undefined && item.type !== 'RequestBoundary' && activeKey === item.display_key;
  const turn = row.turn;
  // As in Harness, the promoted initial prompt precedes the Turn's rail.
  const initialSystem = item?.type === 'SystemPromptCell' && record?.request?.system_prompt.state === 'initial';
  const error = record !== undefined && item?.type === 'RecordRow' && isErrorRecord(record);
  const toggleSummary = () => {
    if (row.callsOwner) actions.toggleCalls(row.callsOwner);
    else if (turn) actions.toggleTurn(turn.attempt_id);
  };
  const onDoubleClick = (event: MouseEvent) => {
    if (summary || !item || item.type === 'RequestBoundary' || !turn) return;
    if (folded.has(turn.attempt_id) && actions.foldableTurns.has(turn.attempt_id)) { event.preventDefault(); actions.toggleTurn(turn.attempt_id); return; }
    if (item.type === 'RecordRow' && actions.callOwners.has(item.owner_record_id)) { event.preventDefault(); actions.toggleCalls(item.owner_record_id); return; }
    if (row.turnStart && actions.foldableTurns.has(turn.attempt_id)) { event.preventDefault(); actions.toggleTurn(turn.attempt_id); }
  };
  const request = row.request;
  const requestNumber = request ? actions.requestNumbers.get(request.owner_record_id) : undefined;
  const requestLabel = tx('trajectory:request.label', { request: requestNumber ?? '—' });
  const label = summary
    ? tx('trajectory:request.collapsed-summary', { kind: tx(row.callsOwner ? 'trajectory:request.collapsed-assistant' : 'trajectory:request.collapsed-turn'), summary: row.summary ?? '' })
    : item && item.type !== 'RequestBoundary'
      ? tx('trajectory:request.row-aria', { request: requestNumber === undefined ? '' : tx('trajectory:request.row-prefix', { request: requestNumber }), kind: cellLabel(tx, cellKind(item)), content: listText(tx, item) || tx('trajectory:request.no-content') })
      : turn?.label;
  return <div data-display-key={row.display_key} data-owner={item?.owner_record_id} data-trace-id={record?.id}
    data-display-type={row.kind === 'structure' ? 'StructuralSeat' : row.kind === 'marker' ? 'MarkerSeat' : summary ? (row.callsOwner ? 'CallsSummary' : 'TurnSummary') : item?.type}
    data-kind={item ? cellKind(item) : undefined} data-state={record?.state} data-attempt={turn?.attempt_id}
    data-error={error || undefined} data-running={record?.state === 'running' || undefined}
    data-selected={selected || undefined} data-turn-start={row.turnStart || undefined}
    data-collapsed-summary={summary ? (row.callsOwner ? 'assistant' : 'turn') : undefined}
    data-timeline-focus={focusedIds && record ? focusedIds.has(record.id) ? 'inside' : 'outside' : undefined}
    role="row" aria-rowindex={index + 1} aria-selected={selected} aria-label={label}
    tabIndex={(item && item.type !== 'RequestBoundary') || summary ? 0 : -1} className={css.record} style={style}
    onClick={() => { if (summary) toggleSummary(); else if (item && item.type !== 'RequestBoundary') actions.select(item); }}
    onDoubleClick={onDoubleClick}
    onKeyDown={event => {
      if (event.target !== event.currentTarget) return;
      if (summary && (event.key === 'Enter' || event.key === ' ')) { event.preventDefault(); toggleSummary(); return; }
      if (event.key === 'Escape' || item) onKeyDown(event, item?.display_key ?? '');
    }}>
    <span role="cell" className={css.event}>
      {request && <button type="button" onKeyDown={event => onKeyDown(event, request.display_key)} className={css.requestChrome} data-display-key={request.display_key}
        aria-label={requestLabel} aria-pressed={activeKey === request.display_key} data-label={requestLabel}
        data-request-owner={request.owner_record_id} data-request-id={request.record.request?.request_id}
        data-selected={activeKey === request.display_key || undefined} data-status={isErrorRecord(request.record) ? 'error' : undefined}
        style={row.requestRun ? { '--request-boundary-offset': `${row.requestRun * 8}px` } as CSSProperties : undefined}
        onClick={event => { event.stopPropagation(); actions.select(request); }}
        onDoubleClick={event => { event.stopPropagation(); }} />}
      {turn && turn.attempt_id === activeTurn && !initialSystem && <span className={css.turnRail} aria-hidden="true" />}
      {selected && <span className={css.selectionRail} aria-hidden="true" />}
      {row.turnStart && turn && <span className={turn.attempt_id === activeTurn ? `${css.turnLabel} ${css.turnLabelActive}` : css.turnLabel} aria-label={turn.label}>
        <span className={css.turnLabelFull} aria-hidden="true">{turn.label}</span>
        <span className={css.turnLabelCompact} aria-hidden="true">#{turn.ordinal}</span>
      </span>}
      {row.kind === 'semantic' && item && <KindTag kind={cellKind(item)} />}
    </span>
    <div role="cell" className={css.content}>
      {summary
        ? <span className={css.collapsedTurnContent} title={row.summary}>
          <span className={css.collapsedTurnEllipsis}>…</span>
          <span className={css.collapsedTurnText}>{row.summary}</span>
        </span>
        : item && row.kind === 'semantic' ? <CellContent item={item} /> : null}
    </div>
  </div>;
}
