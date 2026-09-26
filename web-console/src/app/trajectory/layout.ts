/* Copyright (c) 2026 DeepSeek. MIT. Adapted from pinned Harness ui-trajectory/layout.ts; see PROVENANCE.md. */
import type { TraceContextPresentation, TraceRecord } from '../../../../protocol/app-server/v23';

export type TrajectoryFacet = 'Summary' | 'System Prompt' | 'Diff' | 'Context' | 'Tools' | 'Options' | 'Usage' | 'Timing' | 'Native' | 'Content' | 'Thinking' | 'Raw' | 'Input' | 'Code' | 'Result' | 'Schema' | 'Artifacts';
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
  | (Origin & { type: 'RequestBoundary' })
  | (Origin & { type: 'CollapsedCallSummary'; executions: readonly TraceRecord[] });
export type FocusableDisplayItem = InspectableDisplayItem | StructuralDisplayItem;
export type TrajectoryDisplayItem = FocusableDisplayItem | { type: 'HistoryBoundary'; display_key: string; cursor: string };
export function isInspectable(item: TrajectoryDisplayItem): item is InspectableDisplayItem {
  return item.type !== 'HistoryBoundary' && item.type !== 'GroupHeader' && item.type !== 'TurnHeader';
}
export const displayKey = (...parts: (string | number | null | undefined)[]) => JSON.stringify(parts);

function origin(record: TraceRecord, tag: string, label: string, preview = '', facet: TrajectoryFacet = 'Summary', ...parts: string[]): Origin {
  return { record, owner_record_id: record.id, display_key: displayKey(tag, ...parts), facet, label, preview };
}

/** Project each native dimension independently. Previews and neighboring requests
 * cannot establish a relationship or erase a fact from the other dimension. */
export function systemPresentation(record: TraceRecord): { label: string; facet: TrajectoryFacet } | undefined {
  const request = record.request;
  if (!request) return;
  const prompt = request.system_prompt.state;
  const tools = request.tool_catalog;
  const promptLabel = {
    initial: 'Initial System Prompt', changed: 'System Prompt Updated',
    unchanged: '', previous_unavailable: 'Previous System Prompt unavailable',
  }[prompt];
  const toolsLabel = {
    initial: 'Initial Tools', changed: 'Tools Updated',
    unchanged: '', previous_unavailable: 'Previous Tool catalog unavailable',
  }[tools];
  // These compact names retain the established presentation for complete facts.
  const label = prompt === 'initial' && tools === 'initial' ? 'Initial System Prompt'
    : prompt === 'changed' && tools === 'changed' ? 'System Prompt and Tools Updated'
    : [promptLabel, toolsLabel].filter(Boolean).join(' · ');
  if (!label) return;
  const facet = prompt === 'changed' ? 'Diff' : prompt === 'initial' ? 'System Prompt'
    : tools === 'changed' || tools === 'initial' ? 'Tools' : 'Summary';
  return { label, facet };
}

export function recordLabel(record: TraceRecord): string {
  if (record.kind === 'compaction') return record.state === 'completed' ? 'COMPACTED' : record.state === 'running' ? 'Compacting…' : `Compaction · ${record.state}`;
  return record.kind.toUpperCase();
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

function cellsOf(record: TraceRecord): InspectableDisplayItem[] {
  if (record.kind === 'attempt' || record.kind === 'step') return [];
  if (record.kind !== 'request' || !record.request) return [{ ...origin(record, 'record', recordLabel(record), record.preview?.text ?? '', 'Summary', record.id), type: 'RecordRow' }];
  const request = record.request;
  const cells: InspectableDisplayItem[] = [];
  const change = systemPresentation(record);
  if (change) cells.push({ ...origin(record, 'system', change.label, request.system_prompt.preview?.text ?? (change.facet === 'Tools' ? (request.tool_catalog === 'changed' ? 'Frozen Tool catalog changed' : 'Initial frozen Tool catalog') : 'Prompt preview unavailable'), change.facet, record.id, request.request_id), type: 'SystemPromptCell' });
  for (const context of request.context_additions) {
    cells.push({ ...origin(record, 'context', context.context_kind.replaceAll('_', ' '), context.preview?.text ?? 'Content unavailable', 'Context', record.id, request.request_id, context.message_id), type: 'ContextRow', context, context_message_id: context.message_id });
  }
  cells.push({ ...origin(record, 'request-boundary', 'Request', request.model, 'Summary', record.id, request.request_id), type: 'RequestBoundary' });
  return cells;
}

/** Exactly one finite, native-owned Turn projection for ledger and overview.
 * Durable input order orders Turns and Steps, but never establishes ownership.
 * Interleaved records with the same exact location join the same group. Unscoped
 * records remain standalone sections, never members of a nearby Turn. */
export function projectTrajectory(records: readonly TraceRecord[]): TrajectoryProjection {
  const sections: TrajectorySection[] = [];
  const turns = new Map<string, TrajectoryTurnModel>();
  const groups = new Map<string, TrajectoryGroupModel>();
  for (const record of records) {
    const { attempt_id: attempt, step_id: step } = record.location;
    if (attempt == null) {
      sections.push({ kind: 'outside', record, cells: cellsOf(record) });
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
        ? { kind: 'message', label: 'Message', records: [], cells: [] }
        : { kind: 'step', nativeStepId: step, label: `Step ${turn.groups.filter(group => group.kind === 'step').length + 1}`, records: [], cells: [] };
      groups.set(key, group);
      // Attempt-only inputs form the Message group; request-owned prompt/context
      // cells retain their exact Step, including an initial prompt.
      if (group.kind === 'message') turn.groups.unshift(group);
      else turn.groups.push(group);
    }
    group.records.push(record);
    group.cells.push(...cellsOf(record));
  }
  return { sections };
}

/** Flatten only the shared projection, never reconstruct ownership in a renderer. */
export function trajectoryItems(projection: TrajectoryProjection, cursor?: string | null): TrajectoryDisplayItem[] {
  const items: TrajectoryDisplayItem[] = [];
  if (cursor) items.push({ type: 'HistoryBoundary', display_key: displayKey('history-boundary', cursor), cursor });
  for (const section of projection.sections) {
    if (section.kind === 'outside') { items.push(...section.cells); continue; }
    const attempt = section.nativeAttemptId;
    const native = section.records.find(record => record.kind === 'attempt');
    items.push({ type: 'TurnHeader', display_key: displayKey('turn', attempt), attempt_id: attempt,
      anchor_record_id: section.records[0]!.id, record_ids: section.records.map(record => record.id),
      ordinal: section.displayOrdinal, label: `Turn ${section.displayOrdinal}`, preview: '', ...(native ? { native_record: native } : {}) });
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

export function callsSummary(owner: TraceRecord, executions: readonly TraceRecord[]): string {
  const states = new Map<string, number>();
  for (const execution of executions) {
    const state = execution.state === 'completed' ? 'settled' : execution.state;
    states.set(state, (states.get(state) ?? 0) + 1);
  }
  return `${owner.calls.length} proposed · ${executions.length} loaded matching executions${[...states].map(([state, count]) => ` · ${count} ${state}`).join('')} · ${executions.filter(record => record.tool?.started).length} started`;
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

/** Search bypasses both collapse policies. It never performs a read. */
export function visibleItems(items: readonly TrajectoryDisplayItem[], records: readonly TraceRecord[], attempts: ReadonlySet<string>, calls: ReadonlySet<string>, matches: ReadonlySet<string> | null): TrajectoryDisplayItem[] {
  if (matches) {
    const owners = matchedRecordIds(items, matches)!;
    return items.filter(item => isInspectable(item) ? matches.has(item.display_key)
      : item.type !== 'HistoryBoundary' && item.record_ids.some(id => owners.has(id)));
  }
  const matching = matchingCalls(records);
  const hidden = new Set<string>();
  for (const owner of calls) for (const record of matching.get(owner) ?? []) hidden.add(record.id);
  return items.flatMap<TrajectoryDisplayItem>(item => {
    if (item.type === 'HistoryBoundary') return [item];
    const attempt = isInspectable(item) ? item.record.location.attempt_id : item.attempt_id;
    if (item.type !== 'TurnHeader' && attempt != null && attempts.has(attempt)) return [];
    if (item.type === 'RecordRow' && hidden.has(item.owner_record_id)) return [];
    if (item.type === 'RecordRow' && calls.has(item.owner_record_id) && item.record.calls.length) {
      const executions = matching.get(item.owner_record_id) ?? [];
      const summary: TrajectoryDisplayItem = { ...origin(item.record, 'collapsed-calls', 'Calls', callsSummary(item.record, executions), 'Summary', item.record.message_id ?? item.record.id), type: 'CollapsedCallSummary', executions };
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

/** Ordinals and loaded anchors may change; native structural identity cannot. */
export function preferredStructure(items: readonly TrajectoryDisplayItem[], previous: StructuralDisplayItem): StructuralDisplayItem | undefined {
  return items.find((item): item is StructuralDisplayItem =>
    item.type === previous.type && !isInspectable(item)
    && item.attempt_id === previous.attempt_id
    && (item.type !== 'GroupHeader' || previous.type !== 'GroupHeader' || item.step_id === previous.step_id)
    );
}
export function preferredDisplayItem(items: readonly TrajectoryDisplayItem[], previous: FocusableDisplayItem): FocusableDisplayItem | undefined {
  return isInspectable(previous) ? preferredItem(items, previous.owner_record_id, previous) : preferredStructure(items, previous);
}
