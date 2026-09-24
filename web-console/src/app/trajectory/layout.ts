/* Copyright (c) 2026 DeepSeek. MIT. Adapted from pinned Harness ui-trajectory/layout.ts; see PROVENANCE.md. */
import type { TraceContextPresentation, TraceRecord } from '../../../../protocol/app-server/v20';

export type TrajectoryFacet = 'Summary' | 'System Prompt' | 'Diff' | 'Context' | 'Tools' | 'Options' | 'Usage' | 'Timing' | 'Native' | 'Content' | 'Thinking' | 'Raw' | 'Input' | 'Code' | 'Result' | 'Schema' | 'Artifacts';
export interface TrajectorySelection {
  display_key: string;
  owner_record_id: string;
  facet: TrajectoryFacet;
  context_message_id?: string;
}
interface Origin extends TrajectorySelection {
  record: TraceRecord;
  label: string;
  preview: string;
}
/** A segment anchor controls placement, never detail ownership. Headers stay
 * presentation-only even when the exact native structural record is loaded. */
interface Segment {
  display_key: string;
  attempt_id: string;
  segment_anchor_record_id: string;
  segment_record_ids: string[];
  native_record?: TraceRecord;
  label: string;
  preview: string;
}
export type StructuralDisplayItem =
  | (Segment & { type: 'StepHeader'; step_id: string })
  | (Segment & { type: 'AttemptSectionHeader'; ordinal: number });
/** Only this closed union owns inspectable native records. */
export type InspectableDisplayItem =
  | (Origin & { type: 'RecordRow' })
  | (Origin & { type: 'SystemRow' })
  | (Origin & { type: 'ContextRow'; context: TraceContextPresentation })
  | (Origin & { type: 'RequestBoundary' })
  | (Origin & { type: 'CollapsedCallSummary'; executions: readonly TraceRecord[] });
export type FocusableDisplayItem = InspectableDisplayItem | StructuralDisplayItem;
export type TrajectoryDisplayItem = FocusableDisplayItem | { type: 'HistoryBoundary'; display_key: string; cursor: string };
export function isInspectable(item: TrajectoryDisplayItem): item is InspectableDisplayItem {
  return item.type !== 'HistoryBoundary' && item.type !== 'StepHeader' && item.type !== 'AttemptSectionHeader';
}
export const displayKey = (...parts: (string | number | null | undefined)[]) => JSON.stringify(parts);

function origin(record: TraceRecord, tag: string, label: string, preview = '', facet: TrajectoryFacet = 'Summary', ...parts: string[]): Origin {
  return { record, owner_record_id: record.id, display_key: displayKey(tag, ...parts), facet, label, preview };
}

/** Relationships have already been classified against complete frozen snapshots. */
export function systemLabel(record: TraceRecord): { label: string; facet: TrajectoryFacet } | undefined {
  const request = record.request;
  if (!request) return;
  const prompt = request.system_prompt.state;
  const tools = request.tool_catalog;
  if (prompt === 'previous_unavailable' || tools === 'previous_unavailable') return { label: 'Previous input unavailable', facet: 'Summary' };
  if (prompt === 'initial') return { label: 'Initial System Prompt', facet: 'System Prompt' };
  if (prompt === 'changed' && tools === 'changed') return { label: 'System Prompt and Tools Updated', facet: 'Summary' };
  if (prompt === 'changed') return { label: 'System Prompt Updated', facet: 'System Prompt' };
  if (tools === 'changed') return { label: 'Tools Updated', facet: 'Tools' };
}

export function recordLabel(record: TraceRecord): string {
  if (record.kind === 'compaction') return record.state === 'completed' ? 'COMPACTED' : record.state === 'running' ? 'Compacting…' : `Compaction · ${record.state}`;
  return record.kind.toUpperCase();
}

/** The caller supplies exactly one native conversation window in durable order. */
export function trajectoryItems(records: readonly TraceRecord[], cursor?: string | null): TrajectoryDisplayItem[] {
  const items: TrajectoryDisplayItem[] = [];
  if (cursor) items.push({ type: 'HistoryBoundary', display_key: displayKey('history-boundary', cursor), cursor });
  const ordinals = new Map<string, number>();
  const nativeAttempts = new Map(records.filter(record => record.kind === 'attempt').map(record => [record.location.attempt_id, record]));
  const nativeSteps = new Map(records.filter(record => record.kind === 'step').map(record => [displayKey(record.location.attempt_id, record.location.step_id), record]));
  let attemptSegment: StructuralDisplayItem | undefined;
  let stepSegment: StructuralDisplayItem | undefined;
  let previousAttempt: string | null | undefined;
  let previousStep: string | null | undefined;
  for (const record of records) {
    const attempt = record.location.attempt_id;
    const step = record.location.step_id;
    const newSection = attempt != null && attempt !== previousAttempt;
    if (newSection) {
      if (!ordinals.has(attempt)) ordinals.set(attempt, ordinals.size + 1);
      const ordinal = ordinals.get(attempt)!;
      const native = nativeAttempts.get(attempt);
      attemptSegment = { type: 'AttemptSectionHeader', display_key: displayKey('attempt-section', attempt, record.id), attempt_id: attempt, segment_anchor_record_id: record.id, segment_record_ids: [], ordinal, label: `Attempt ${ordinal}`, preview: '', ...(native ? { native_record: native } : {}) };
      items.push(attemptSegment);
    }
    if (attempt != null && step != null && (newSection || step !== previousStep)) {
      const native = nativeSteps.get(displayKey(attempt, step));
      stepSegment = { type: 'StepHeader', display_key: displayKey('step-segment', attempt, step, record.id), attempt_id: attempt, step_id: step, segment_anchor_record_id: record.id, segment_record_ids: [], label: `Step ${step}`, preview: '', ...(native ? { native_record: native } : {}) };
      items.push(stepSegment);
    }
    if (attempt == null) attemptSegment = undefined;
    if (attempt == null || step == null) stepSegment = undefined;
    attemptSegment?.segment_record_ids.push(record.id);
    stepSegment?.segment_record_ids.push(record.id);
    previousAttempt = attempt;
    previousStep = step;
    if (record.kind === 'attempt' || record.kind === 'step') continue;
    if (record.kind === 'request' && record.request) {
      const request = record.request;
      const change = systemLabel(record);
      if (change) items.push({ ...origin(record, 'system', change.label, request.system_prompt.preview?.text ?? (change.facet === 'Tools' ? 'Frozen Tool catalog changed' : 'Prompt preview unavailable'), change.facet, request.request_id), type: 'SystemRow' });
      for (const context of request.context_additions) {
        items.push({ ...origin(record, 'context', context.context_kind.replaceAll('_', ' '), context.preview?.text ?? 'Content unavailable', 'Context', request.request_id, context.message_id), type: 'ContextRow', context, context_message_id: context.message_id });
      }
      items.push({ ...origin(record, 'request-boundary', 'Request', request.model, 'Summary', request.request_id), type: 'RequestBoundary' });
    } else items.push({ ...origin(record, 'record', recordLabel(record), record.preview?.text ?? '', 'Summary', record.id), type: 'RecordRow' });
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

/** Search bypasses both collapse policies. It never performs a read. */
export function visibleItems(items: readonly TrajectoryDisplayItem[], records: readonly TraceRecord[], attempts: ReadonlySet<string>, calls: ReadonlySet<string>, matches: ReadonlySet<string> | null): TrajectoryDisplayItem[] {
  if (matches) return items.filter(item => matches.has(item.display_key));
  const matching = matchingCalls(records);
  const hidden = new Set<string>();
  for (const owner of calls) for (const record of matching.get(owner) ?? []) hidden.add(record.id);
  return items.flatMap<TrajectoryDisplayItem>(item => {
    if (item.type === 'HistoryBoundary') return [item];
    const attempt = isInspectable(item) ? item.record.location.attempt_id : item.attempt_id;
    if (item.type !== 'AttemptSectionHeader' && attempt != null && attempts.has(attempt)) return [];
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
  return { display_key: item.display_key, owner_record_id: item.owner_record_id, facet: item.facet, ...(item.context_message_id ? { context_message_id: item.context_message_id } : {}) };
}

/** Regrouping can move a segment's anchor, never its native structural identity.
 * Membership selects the containing segment, not another segment of the same Step. */
export function preferredStructure(items: readonly TrajectoryDisplayItem[], previous: StructuralDisplayItem): StructuralDisplayItem | undefined {
  return items.find((item): item is StructuralDisplayItem =>
    item.type === previous.type && !isInspectable(item)
    && item.attempt_id === previous.attempt_id
    && (item.type !== 'StepHeader' || previous.type !== 'StepHeader' || item.step_id === previous.step_id)
    && item.segment_record_ids.includes(previous.segment_anchor_record_id));
}
export function preferredDisplayItem(items: readonly TrajectoryDisplayItem[], previous: FocusableDisplayItem): FocusableDisplayItem | undefined {
  return isInspectable(previous) ? preferredItem(items, previous.owner_record_id, previous) : preferredStructure(items, previous);
}
