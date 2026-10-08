import { traceStateLabel } from '../../bindings/status-labels';
import type { Translate } from '../../locale/translation';
/* Copyright (c) 2026 DeepSeek. MIT. Adapted from pinned Harness ui-trajectory/layout.ts; see PROVENANCE.md. */
import type { TraceContextKind, TraceContextPresentation, TraceRecord } from '../../../../protocol/app-server/v38';

/** The Harness detail tab identities. Labels follow the locale; ids never do. */
export type TrajectoryFacet = 'overview' | 'rendered' | 'raw' | 'source' | 'input' | 'output' | 'schema' | 'timing' | 'options' | 'usage' | 'system-prompt' | 'tools' | 'diff';
export interface TrajectorySelection {
  display_key: string;
  owner_record_id: string;
  facet: TrajectoryFacet;
  cell_type?: 'SystemPromptCell' | 'ContextRow';
  context_message_id?: string;
}
interface Origin extends TrajectorySelection {
  record: TraceRecord;
  label: string;
  preview: string;
}
/** A loaded anchor controls placement, never detail ownership. Headers never
 * own a detail read; an exact loaded native structural record is exposed only
 * as its own bounded summary evidence, never borrowed from a member. */
interface Structure {
  display_key: string;
  attempt_id: string;
  anchor_record_id: string;
  record_ids: string[];
  native_record?: TraceRecord;
  label: string;
  preview: string;
}
export type StructuralDisplayItem =
  | (Structure & { type: 'GroupHeader'; kind: 'message' | 'step'; step_id?: string })
  | (Structure & { type: 'TurnHeader'; ordinal: number });
/** Only this closed union owns inspectable native records. */
export type InspectableDisplayItem =
  | (Origin & { type: 'RecordRow' })
  | (Origin & { type: 'SystemPromptCell' })
  | (Origin & { type: 'ContextRow'; context: TraceContextPresentation })
  | (Origin & { type: 'RequestBoundary' });
/** As in Harness, Turns and Steps are context for records, never selections. */
export type TurnStructure = Extract<StructuralDisplayItem, { type: 'TurnHeader' }>;
/** The folded Tool calls of one Assistant record: a toggle, not a record. */
export interface CallsSummaryItem { type: 'CallsSummary'; display_key: string; owner_record_id: string; summary: string }
export type TrajectoryDisplayItem = InspectableDisplayItem | StructuralDisplayItem | CallsSummaryItem | { type: 'HistoryBoundary'; display_key: string; cursor: string };
export function isInspectable(item: TrajectoryDisplayItem): item is InspectableDisplayItem {
  return item.type !== 'HistoryBoundary' && item.type !== 'GroupHeader' && item.type !== 'TurnHeader' && item.type !== 'CallsSummary';
}
export const displayKey = (...parts: (string | number | null | undefined)[]) => JSON.stringify(parts);
/** The only visible names of these closed domains. The native context kind and
 * the facet identity stay untranslated; only their labels follow the locale. */
export const contextKindLabel = (tx: Translate, kind: TraceContextKind) => tx(`trajectory:context.${kind}`);

function origin(record: TraceRecord, tag: string, label: string, preview = '', facet: TrajectoryFacet = 'overview', ...parts: string[]): Origin {
  return { record, owner_record_id: record.id, display_key: displayKey(tag, ...parts), facet, label, preview };
}

/** Project each native dimension independently. Previews and neighboring requests
 * cannot establish a relationship or erase a fact from the other dimension. */
export function systemPresentation(tx: Translate, record: TraceRecord): { label: string; facet: TrajectoryFacet } | undefined {
  const request = record.request;
  if (!request) return;
  const prompt = request.system_prompt.state;
  const tools = request.tool_catalog;
  const promptLabel = {
    initial: tx('trajectory:layout.initial-system-prompt'), changed: tx('trajectory:layout.system-prompt-updated'),
    unchanged: '', previous_unavailable: tx('trajectory:copy.previous-system-prompt-unavailable'),
  }[prompt];
  const toolsLabel = {
    initial: tx('trajectory:copy.initial-tools'), changed: tx('trajectory:layout.tools-updated'),
    unchanged: '', previous_unavailable: tx('trajectory:copy.previous-tool-catalog-unavailable'),
  }[tools];
  // These compact names retain the established presentation for complete facts.
  const label = prompt === 'initial' && tools === 'initial' ? tx('trajectory:layout.initial-system-prompt')
    : prompt === 'changed' && tools === 'changed' ? tx('trajectory:layout.system-prompt-and-tools-updated')
    : [promptLabel, toolsLabel].filter(Boolean).join(' · ');
  if (!label) return;
  const facet = prompt === 'changed' ? 'diff' : prompt === 'initial' ? 'system-prompt'
    : tools === 'changed' || tools === 'initial' ? 'tools' : 'system-prompt';
  return { label, facet };
}

export function recordLabel(tx: Translate, record: TraceRecord): string {
  if (record.kind === 'compaction') return record.state === 'completed' ? tx('trajectory:compacted') : record.state === 'running' ? tx('trajectory:copy.compacting') : tx('trajectory:copy.compaction-value', { p0: traceStateLabel(tx, record.state) });
  return tx(`trajectory:kind.${record.kind}`);
}

/** One native Step, or attempt-owned material with no Step. No proximity inference. */
export interface TrajectoryGroupModel {
  kind: 'message' | 'step';
  nativeStepId?: string;
  label: string;
  records: TraceRecord[];
  cells: InspectableDisplayItem[];
}
export interface TrajectoryTurnModel {
  kind: 'turn';
  nativeAttemptId: string;
  displayOrdinal: number;
  records: TraceRecord[];
  groups: TrajectoryGroupModel[];
}
export type TrajectorySection = TrajectoryTurnModel | {
  kind: 'outside'; record: TraceRecord; cells: InspectableDisplayItem[];
};
export interface TrajectoryProjection {
  sections: TrajectorySection[];
}

function cellsOf(tx: Translate, record: TraceRecord): InspectableDisplayItem[] {
  if (record.kind === 'attempt' || record.kind === 'step') return [];
  if (record.kind !== 'request' || !record.request) return [{ ...origin(record, 'record', recordLabel(tx, record), record.preview?.text ?? '', 'overview', record.id), type: 'RecordRow' }];
  const request = record.request;
  const cells: InspectableDisplayItem[] = [];
  const change = systemPresentation(tx, record);
  if (change) cells.push({ ...origin(record, 'system', change.label, request.system_prompt.preview?.text ?? (change.facet === 'tools' ? (request.tool_catalog === 'changed' ? tx('trajectory:copy.frozen-tool-catalog-changed') : tx('trajectory:copy.initial-frozen-tool-catalog')) : tx('trajectory:copy.prompt-preview-unavailable')), change.facet, record.id, request.request_id), type: 'SystemPromptCell' });
  for (const context of request.context_additions) {
    cells.push({ ...origin(record, 'context', contextKindLabel(tx, context.context_kind), context.preview?.text ?? tx('trajectory:trajectory-inspector.content-unavailable'), 'overview', record.id, request.request_id, context.message_id), type: 'ContextRow', context, context_message_id: context.message_id });
  }
  cells.push({ ...origin(record, 'request-boundary', tx('trajectory:copy.request'), request.model, 'overview', record.id, request.request_id), type: 'RequestBoundary' });
  return cells;
}

/** Exactly one finite, native-owned Turn projection for ledger and overview.
 * Durable input order orders Turns and Steps, but never establishes ownership.
 * Interleaved records with the same exact location join the same group. Unscoped
 * records remain standalone sections, never members of a nearby Turn. */
export function projectTrajectory(tx: Translate, records: readonly TraceRecord[]): TrajectoryProjection {
  const sections: TrajectorySection[] = [];
  const turns = new Map<string, TrajectoryTurnModel>();
  const groups = new Map<string, TrajectoryGroupModel>();
  for (const record of records) {
    const { attempt_id: attempt, step_id: step } = record.location;
    if (attempt == null) {
      sections.push({ kind: 'outside', record, cells: cellsOf(tx, record) });
      continue;
    }
    let turn = turns.get(attempt);
    if (!turn) {
      turn = { kind: 'turn', nativeAttemptId: attempt, displayOrdinal: turns.size + 1, records: [], groups: [] };
      turns.set(attempt, turn);
      sections.push(turn);
    }
    turn.records.push(record);
    if (record.kind === 'attempt') continue;
    const key = displayKey(attempt, step);
    let group = groups.get(key);
    if (!group) {
      group = step == null
        ? { kind: 'message', label: tx('trajectory:group.message'), records: [], cells: [] }
        : { kind: 'step', nativeStepId: step, label: tx('trajectory:group.step', { n: turn.groups.filter(group => group.kind === 'step').length + 1 }), records: [], cells: [] };
      groups.set(key, group);
      // Attempt-only inputs form the Message group; request-owned prompt/context
      // cells retain their exact Step, including an initial prompt.
      if (group.kind === 'message') turn.groups.unshift(group);
      else turn.groups.push(group);
    }
    group.records.push(record);
    group.cells.push(...cellsOf(tx, record));
  }
  return { sections };
}

/** Flatten only the shared projection, never reconstruct ownership in a renderer. */
export function trajectoryItems(tx: Translate, projection: TrajectoryProjection, cursor?: string | null): TrajectoryDisplayItem[] {
  const items: TrajectoryDisplayItem[] = [];
  if (cursor) items.push({ type: 'HistoryBoundary', display_key: displayKey('history-boundary', cursor), cursor });
  for (const section of projection.sections) {
    if (section.kind === 'outside') { items.push(...section.cells); continue; }
    const attempt = section.nativeAttemptId;
    const native = section.records.find(record => record.kind === 'attempt');
    items.push({ type: 'TurnHeader', display_key: displayKey('turn', attempt), attempt_id: attempt,
      anchor_record_id: section.records[0]!.id, record_ids: section.records.map(record => record.id),
      ordinal: section.displayOrdinal, label: tx('trajectory:copy.turn-value', { p0: section.displayOrdinal }), preview: '', ...(native ? { native_record: native } : {}) });
    for (const group of section.groups) {
      const native = group.records.find(record => record.kind === 'step');
      items.push({ type: 'GroupHeader', kind: group.kind, display_key: displayKey('group', attempt, group.nativeStepId), attempt_id: attempt,
        ...(group.nativeStepId === undefined ? {} : { step_id: group.nativeStepId }),
        anchor_record_id: group.records[0]!.id, record_ids: group.records.map(record => record.id),
        label: group.label, preview: '', ...(native ? { native_record: native } : {}) });
      items.push(...group.cells);
    }
  }
  return items;
}

/** Never correlate incomplete scope, or ambiguous proposals. Session/Conversation
 * isolation is supplied by the native Trace cache, not reconstructed from IDs. */
function callScope(record: TraceRecord, call: { call_id: string; tool_id: string }): string | undefined {
  const { attempt_id, step_id } = record.location;
  if (attempt_id == null || step_id == null) return;
  return displayKey(attempt_id, step_id, call.call_id, call.tool_id);
}
export function matchingCalls(records: readonly TraceRecord[]): Map<string, TraceRecord[]> {
  const owners = new Map<string, string[]>();
  for (const record of records) {
    if (record.kind !== 'assistant' || !record.message_id) continue;
    for (const call of record.calls) {
      const scope = callScope(record, call);
      if (scope) owners.set(scope, [...(owners.get(scope) ?? []), record.id]);
    }
  }
  const matches = new Map<string, TraceRecord[]>();
  for (const record of records) {
    if (record.kind !== 'tool' || !record.tool) continue;
    const scope = callScope(record, record.tool);
    const candidates = scope ? owners.get(scope) : undefined;
    if (candidates?.length !== 1) continue;
    const owner = candidates[0]!;
    matches.set(owner, [...(matches.get(owner) ?? []), record]);
  }
  return matches;
}

/** Harness folded-calls copy: the count, then the distinct Tool names. */
export function callsSummary(tx: Translate, executions: readonly TraceRecord[]): string {
  const names = [...new Set(executions.map(record => record.tool?.name ?? record.tool?.tool_id ?? ''))].filter(Boolean);
  const count = tx(executions.length === 1 ? 'trajectory:summary.tool-calls.one' : 'trajectory:summary.tool-calls.other', { count: executions.length });
  return names.length ? `${count} · ${names.join(', ')}` : count;
}

/** Search visibility and timeline dimming share the projection's exact membership.
 * Structural labels are evidence, never identities used to reconstruct ownership. */
export function matchedRecordIds(items: readonly TrajectoryDisplayItem[], matches: ReadonlySet<string> | null): ReadonlySet<string> | null {
  if (matches === null) return null;
  const owners = new Set<string>();
  for (const item of items) {
    if (isInspectable(item) && matches.has(item.display_key)) owners.add(item.owner_record_id);
  }
  return owners;
}

/** Search bypasses Tool-call folding; ledgerRows independently bypasses Turn
 * folding for the same query. Neither projection performs a read. */
export function visibleItems(tx: Translate, items: readonly TrajectoryDisplayItem[], records: readonly TraceRecord[], calls: ReadonlySet<string>, matches: ReadonlySet<string> | null): TrajectoryDisplayItem[] {
  if (matches) {
    const owners = matchedRecordIds(items, matches)!;
    return items.filter(item => isInspectable(item) ? matches.has(item.display_key)
      : (item.type === 'TurnHeader' || item.type === 'GroupHeader') && item.record_ids.some(id => owners.has(id)));
  }
  const matching = matchingCalls(records);
  const hidden = new Set<string>();
  for (const owner of calls) for (const record of matching.get(owner) ?? []) hidden.add(record.id);
  return items.flatMap<TrajectoryDisplayItem>(item => {
    if (item.type === 'HistoryBoundary') return [item];
    // Turn folding belongs to the semantic ledger projection, where System
    // cells and the first main row remain visible with their native chrome.
    if (item.type === 'RecordRow' && hidden.has(item.owner_record_id)) return [];
    const executions = item.type === 'RecordRow' && calls.has(item.owner_record_id) ? matching.get(item.owner_record_id) ?? [] : [];
    if (item.type === 'RecordRow' && executions.length) {
      const summary: CallsSummaryItem = { type: 'CallsSummary', display_key: displayKey('collapsed-calls', item.owner_record_id), owner_record_id: item.owner_record_id, summary: callsSummary(tx, executions) };
      return [item, summary];
    }
    return [item];
  });
}

/** Base items retain owner/facet targets even when filtered. Current policy items
 * contribute their own identities only while that policy produces them. */
export function displayUniverse(base: readonly TrajectoryDisplayItem[], current: readonly TrajectoryDisplayItem[]): TrajectoryDisplayItem[] {
  return [...new Map([...base, ...current].map(item => [item.display_key, item])).values()];
}

/** Same semantic facet first, then same native owner, never a numeric position. */
export function preferredItem(items: readonly TrajectoryDisplayItem[], owner: string, selection?: TrajectorySelection): InspectableDisplayItem | undefined {
  const candidates = items.filter((item): item is InspectableDisplayItem => isInspectable(item) && item.owner_record_id === owner);
  return candidates.find(item => item.display_key === selection?.display_key)
    ?? candidates.find(item => item.facet === selection?.facet && item.context_message_id === selection?.context_message_id)
    ?? candidates.find(item => item.type === 'RequestBoundary' || item.type === 'RecordRow')
    ?? candidates[0];
}
export function selectionOf(item: InspectableDisplayItem): TrajectorySelection {
  return { display_key: item.display_key, owner_record_id: item.owner_record_id, facet: item.facet, ...(item.type === 'SystemPromptCell' || item.type === 'ContextRow' ? { cell_type: item.type } : {}), ...(item.context_message_id ? { context_message_id: item.context_message_id } : {}) };
}

/** A measurable ledger seat. Turn and Step structure is metadata on content
 * rows, never a row of its own. All relationships are resolved here, before rendering. */
export interface TrajectoryLedgerRow {
  display_key: string;
  kind: 'semantic' | 'marker' | 'structure' | 'summary' | 'history';
  height: 30 | 20 | 10;
  item?: InspectableDisplayItem;
  turn?: TurnStructure;
  request?: Extract<InspectableDisplayItem, { type: 'RequestBoundary' }>;
  /** Marker seats immediately before this Request's dot, as in Harness's
   * request-boundary runs: consecutive dots step right instead of stacking. */
  requestRun?: number;
  turnStart: boolean;
  summary?: string;
  /** The Assistant record whose folded Tool calls a summary row expands. */
  callsOwner?: string;
}

export function ledgerRows(tx: Translate, projection: TrajectoryProjection, items: readonly TrajectoryDisplayItem[], folded: ReadonlySet<string>, searching: boolean): TrajectoryLedgerRow[] {
  const turns = new Map<string, TurnStructure>();
  const requests = new Map<string, Extract<InspectableDisplayItem, { type: 'RequestBoundary' }>>();
  const owners = new Map<string, TraceRecord>();
  for (const item of items) {
    if (item.type === 'TurnHeader') turns.set(item.attempt_id, item);
    if (isInspectable(item)) owners.set(item.owner_record_id, item.record);
    if (isInspectable(item) && item.record.request) {
      const marker = cellsOf(tx, item.record).find((cell): cell is Extract<InspectableDisplayItem, { type: 'RequestBoundary' }> => cell.type === 'RequestBoundary')!;
      requests.set(item.owner_record_id, marker);
    }
  }
  const cells = items.filter(isInspectable);
  // As in Harness, a Request's marker sits on the boundary above the first
  // output its own Step shows after the Request's own System/Context cells.
  // That row hosts the marker only; the Request keeps its own owner. Without
  // such a row (still running, or a retry follows) it keeps a marker seat.
  const hosts = new Map<string, Extract<InspectableDisplayItem, { type: 'RequestBoundary' }>>();
  const markerSeats = new Set<string>();
  for (const [owner, marker] of requests) {
    const last = cells.reduce((found, cell, index) => cell.owner_record_id === owner ? index : found, -1);
    const next = cells[last + 1];
    const output = next && !next.record.request && next.record.location.attempt_id != null
      && next.record.location.attempt_id === marker.record.location.attempt_id
      && next.record.location.step_id === marker.record.location.step_id ? next : undefined;
    hosts.set((output ?? marker).display_key, marker);
    if (!output) markerSeats.add(marker.display_key);
  }
  const turnOf = (record: TraceRecord | undefined) => {
    const attempt = record?.location.attempt_id;
    return attempt == null ? undefined : turns.get(attempt);
  };
  const seats: TrajectoryLedgerRow[] = items.flatMap<TrajectoryLedgerRow>(item => {
    if (item.type === 'HistoryBoundary') return [{ display_key: item.display_key, kind: 'history', height: 30, turnStart: false }];
    if (item.type === 'CallsSummary') return [{ display_key: item.display_key, kind: 'summary', height: 20, turn: turnOf(owners.get(item.owner_record_id)), turnStart: false, summary: item.summary, callsOwner: item.owner_record_id }];
    if (!isInspectable(item)) return [];
    if (item.type === 'RequestBoundary' && !markerSeats.has(item.display_key)) return [];
    return [{ display_key: item.type === 'RequestBoundary' ? displayKey('marker-seat', item.display_key) : item.display_key, kind: item.type === 'RequestBoundary' ? 'marker' : 'semantic', height: item.type === 'RequestBoundary' ? 10 : 30,
      item, turn: turnOf(item.record), request: hosts.get(item.display_key), turnStart: false }];
  });
  const ownerOf = (row: TrajectoryLedgerRow) => row.item?.owner_record_id ?? row.callsOwner;
  const rows = seats.filter(row => row.kind === 'history');
  const isInitial = (row: TrajectoryLedgerRow) => row.item?.type === 'SystemPromptCell' && row.item.record.request?.system_prompt.state === 'initial';
  // Sections and groups are the native ordering authority. Visibility changes
  // a group's representation, never its position relative to another group.
  for (const section of projection.sections) {
    if (section.kind === 'outside') {
      rows.push(...seats.filter(row => ownerOf(row) === section.record.id));
      continue;
    }
    const turn = turns.get(section.nativeAttemptId);
    if (!turn) continue;
    const owned = seats.filter(row => row.turn === turn);
    // As in Harness, a Turn folds only when it has more than one content row;
    // its System cells and first content row stay visible above the summary.
    const content = owned.filter(row => row.kind === 'semantic' && row.item?.type !== 'SystemPromptCell');
    const collapsed = !searching && folded.has(turn.attempt_id) && content.length > 1;
    const main = content[0];
    // Promotion changes presentation only; the native Request/Step stays intact.
    rows.push(...owned.filter(isInitial));
    const body: TrajectoryLedgerRow[] = [];
    for (const group of section.groups) {
      // Group membership was already resolved from native locations. In
      // particular, wire null and omitted Step identities both belong to the
      // Attempt's message group; neither creates a synthetic Step.
      const members = new Set(group.records.map(record => record.id));
      const segment = owned.filter(row => members.has(ownerOf(row) ?? '') && !isInitial(row)
        && (!collapsed || row.item?.type === 'SystemPromptCell' || row === main));
      // As in Harness, a Step has no chrome of its own: a Step with no visible
      // content has no seat, and a folded Turn counts Steps in its summary.
      body.push(...segment);
    }
    if (collapsed) {
      const steps = section.groups.filter(group => group.kind === 'step').length;
      const calls = section.records.filter(record => record.kind === 'tool').length;
      body.push({ display_key: displayKey('turn-summary', turn.attempt_id), kind: 'summary', height: 20, turn, turnStart: false,
        summary: [tx(steps === 1 ? 'trajectory:summary.steps.one' : 'trajectory:summary.steps.other', { count: steps }),
          tx(calls === 1 ? 'trajectory:summary.tool-calls.one' : 'trajectory:summary.tool-calls.other', { count: calls })].join(' · ') });
    }
    if (!body.length) body.push({ display_key: displayKey('structure-marker', turn.attempt_id), kind: 'structure', height: 20, turn, turnStart: false });
    body[0]!.turnStart = true;
    // A tiny Request seat cannot carry the Turn label.
    if (body[0]!.kind === 'marker') { body[0]!.kind = 'structure'; body[0]!.height = 20; }
    rows.push(...body);
  }
  let run = 0;
  for (const row of rows) {
    if (row.request) row.requestRun = run;
    run = row.request && row.kind === 'marker' ? run + 1 : 0;
  }
  return rows;
}

export function rowOwnsKey(row: TrajectoryLedgerRow, key: string): boolean {
  return row.display_key === key || row.request?.display_key === key;
}

/** Logical navigation is projected from resolved native ownership, not DOM order.
 * A Request boundary has one target, even when it owns an otherwise empty seat. */
export interface LedgerFocusTarget {
  display_key: string;
  row_key: string;
  item: InspectableDisplayItem;
}
export function ledgerFocusTargets(rows: readonly TrajectoryLedgerRow[]): LedgerFocusTarget[] {
  return rows.flatMap(row => {
    const targets: LedgerFocusTarget[] = [];
    const add = (item: InspectableDisplayItem) => targets.push({ item, display_key: item.display_key, row_key: row.display_key });
    if (row.request) add(row.request);
    if (row.item && row.item.type !== 'RequestBoundary') add(row.item);
    return targets;
  });
}
