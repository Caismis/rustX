/* Copyright (c) 2026 DeepSeek. MIT. Adapted from pinned Harness TrajectoryTable.tsx ledger rows; see PROVENANCE.md. */
import type { CSSProperties, KeyboardEvent, MouseEvent } from 'react';
import { useTranslation } from '../../locale/react';
import { CellContent, KindTag, cellKind, cellLabel, isErrorRecord, listText } from './TrajectoryCell';
import type { InspectableDisplayItem, TrajectoryLedgerRow } from './layout';
import css from './Trajectory.module.css';
import type { TraceRecord } from '../../../../protocol/app-server/v71';
import { IconChevronRightOutline14 } from '../../presentation/primitives/icons';

export interface TrajectoryRowActions {
  workflows?: ReadonlyMap<string, TraceRecord>;
  openedWorkflows?: ReadonlySet<string>;
  toggleWorkflow?: (id: string) => void;
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
  const workflow = item && item.type !== 'WorkflowNode' ? actions.workflows?.get(item.owner_record_id) : undefined;
  const workflowOpen = !!workflow && actions.openedWorkflows?.has(workflow.id) === true;
  const node = item?.type === 'WorkflowNode' ? item.workflowNode : undefined;
  const summary = row.kind === 'summary';
  // A Request is selected on its dot, never as a row, as in Harness.
  const selected = item !== undefined && item.type !== 'RequestBoundary' && activeKey === item.display_key;
  const turn = row.turn;
  // As in Harness, the promoted initial prompt precedes the Turn's rail.
  const initialSystem = item?.type === 'SystemPromptCell' && record?.request?.system_prompt.state === 'initial';
  const running = node ? ['running', 'waiting', 'draining'].includes(node.state.type) : record?.state === 'running';
  const focusId = item?.type === 'WorkflowNode' ? item.parent_record_id : record?.id;
  const error = node ? node.state.type === 'settled' && node.state.outcome === 'failed' : record !== undefined && item?.type === 'RecordRow' && isErrorRecord(record);
  const toggleSummary = () => {
    if (row.callsOwner) actions.toggleCalls(row.callsOwner);
    else if (turn) actions.toggleTurn(turn.ownerKey);
  };
  const onDoubleClick = (event: MouseEvent) => {
    if (workflow) { event.preventDefault(); actions.toggleWorkflow?.(workflow.id); return; }
    if (summary || !item || item.type === 'RequestBoundary' || !turn || row.betweenTurns) return;
    if (folded.has(turn.ownerKey) && actions.foldableTurns.has(turn.ownerKey)) { event.preventDefault(); actions.toggleTurn(turn.ownerKey); return; }
    if (item.type === 'RecordRow' && actions.callOwners.has(item.owner_record_id)) { event.preventDefault(); actions.toggleCalls(item.owner_record_id); return; }
    if (row.turnStart && actions.foldableTurns.has(turn.ownerKey)) { event.preventDefault(); actions.toggleTurn(turn.ownerKey); }
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
    data-workflow-node={node?.id} data-workflow-instance={node?.instance.node} data-workflow-entry={workflow?.id}
    data-workflow-open={workflowOpen || undefined} data-workflow-last={row.workflowLast || undefined}
    data-display-type={row.kind === 'structure' ? 'StructuralSeat' : row.kind === 'marker' ? 'MarkerSeat' : summary ? (row.callsOwner ? 'CallsSummary' : 'TurnSummary') : item?.type}
    data-kind={item ? cellKind(item) : undefined} data-state={node ? node.state.type === 'settled' ? node.state.outcome : node.state.type : record?.state} data-attempt={turn?.attempt_id}
    data-error={error || undefined} data-running={running || undefined}
    data-between-turns={row.betweenTurns || undefined} data-selected={selected || undefined} data-turn-start={row.turnStart || undefined}
    data-collapsed-summary={summary ? (row.callsOwner ? 'assistant' : 'turn') : undefined}
    data-timeline-focus={focusedIds && focusId ? focusedIds.has(focusId) ? 'inside' : 'outside' : undefined}
    role="row" aria-rowindex={index + 1} aria-selected={selected} aria-label={label}
    tabIndex={(item && item.type !== 'RequestBoundary') || summary ? 0 : -1} className={css.record} style={style}
    onClick={() => { if (summary) toggleSummary(); else if (item && item.type !== 'RequestBoundary') actions.select(item); }}
    onDoubleClick={onDoubleClick}
    onKeyDown={event => {
      if (event.target !== event.currentTarget) return;
      if (workflow && (event.key === 'ArrowRight' && !workflowOpen || event.key === 'ArrowLeft' && workflowOpen)) { event.preventDefault(); actions.toggleWorkflow?.(workflow.id); return; }
      if (summary && (event.key === 'Enter' || event.key === ' ')) { event.preventDefault(); toggleSummary(); return; }
      if (event.key === 'Escape' || item) onKeyDown(event, item?.display_key ?? '');
    }}>
    <span role="cell" className={css.event}>
      {request && <button type="button" onKeyDown={event => onKeyDown(event, request.display_key)} className={css.requestChrome} data-display-key={request.display_key}
        aria-label={requestLabel} aria-pressed={activeKey === request.display_key} data-label={requestLabel}
        data-state={request.record.state} data-request-owner={request.owner_record_id} data-request-id={request.record.request?.request_id}
        data-selected={activeKey === request.display_key || undefined} data-status={isErrorRecord(request.record) ? 'error' : undefined}
        style={row.requestRun ? { '--request-boundary-offset': `${row.requestRun * 8}px` } as CSSProperties : undefined}
        onClick={event => { event.stopPropagation(); actions.select(request); }}
        onDoubleClick={event => { event.stopPropagation(); }} />}
      {turn && turn.ownerKey === activeTurn && !initialSystem && !row.betweenTurns && <span className={css.turnRail} aria-hidden="true" />}
      {selected && <span className={css.selectionRail} aria-hidden="true" />}
      {row.betweenTurns && <span className={css.turnLabel} aria-label={tx('trajectory:section.between-turns')}>{tx('trajectory:section.between-turns')}</span>}
      {row.turnStart && !row.betweenTurns && turn && <span className={turn.ownerKey === activeTurn ? `${css.turnLabel} ${css.turnLabelActive}` : css.turnLabel} aria-label={turn.label}>
        <span className={css.turnLabelFull} aria-hidden="true">{turn.label}</span>
        <span className={css.turnLabelCompact} aria-hidden="true">#{turn.ordinal}</span>
      </span>}
      {row.kind === 'semantic' && item && !node && <KindTag kind={cellKind(item)} />}
    </span>
    <div role="cell" className={css.content}>
      {workflow && <button type="button" className={css.workflowToggle} aria-expanded={workflowOpen} aria-label={tx(workflowOpen ? 'trajectory:workflow.collapse' : 'trajectory:workflow.expand')}
        onClick={event => { event.stopPropagation(); actions.toggleWorkflow?.(workflow.id); }} onDoubleClick={event => event.stopPropagation()}>
        <IconChevronRightOutline14/>
      </button>}
      {summary
        ? <span className={css.collapsedTurnContent} title={row.summary}>
          <span className={css.collapsedTurnEllipsis}>…</span>
          <span className={css.collapsedTurnText}>{row.summary}</span>
        </span>
        : item && row.kind === 'semantic' ? <CellContent item={item} /> : null}
    </div>
  </div>;
}
