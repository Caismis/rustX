import type { Translate } from '../../locale/translation';
/* Copyright (c) 2026 DeepSeek. MIT. Adapted from pinned Harness ui-trajectory/timeline.ts; see PROVENANCE.md. */
/**
 * Timing projections for the Trajectory overview.
 *
 * Generation phases use request-relative monotonic offsets supplied by Trace,
 * anchored through the native runtime's paired durable-start clock reading.
 * Journal wall spans and dispatch-origin numeric metrics cannot supply that
 * relationship. Missing bridge evidence leaves a request as a marker, with separate numeric metrics.
 */
import { trajectoryItems, isInspectable, type TrajectoryProjection } from './layout';
import type { TraceKind, TraceRecord } from '../../../../protocol/app-server/v36';

/** Horizontal projection of the overview's domain: equal-width operations, or
 * recorded durations with idle time compressed, the two Harness exposes. */
export type TrajectoryTimelineMode = 'sequence' | 'duration';

/** Inclusive selection in the active projection's domain. */
export interface TrajectoryTimeRange {
  start: number;
  end: number;
}

/** One record projected into the active domain. */
export interface TrajectorySpan extends TrajectoryTimeRange {
  id: string;
  ownerId?: string;
  displayKey?: string;
  kind: TraceKind | 'system' | 'context';
  lane: number;
  label: string;
  error: boolean;
  /** Positions in the duration domain, authorized by native phase evidence. */
  dispatchAt?: number;
  firstOutputAt?: number;
  lastOutputAt?: number;
  providerTerminalAt?: number;
  /** Exact recorded start in epoch milliseconds, when known. */
  startedAt?: number;
  /** Exact recorded duration in milliseconds, when known. */
  durationMs?: number;
  ttftMs?: number;
  generationMs?: number;
}

/** One Turn boundary in the active domain. */
export interface TrajectoryBoundary {
  nativeAttemptId: string;
  at: number;
}

/** The overview's complete domain. */
export interface TrajectoryTimelineModel extends TrajectoryTimeRange {
  spans: readonly TrajectorySpan[];
  boundaries: readonly TrajectoryBoundary[];
}

/**
 * Collision-free identity of coordinate meaning, independent of object identity,
 * display ordinals, labels and lifecycle status. A changed identity retires the
 * interaction generation, even when the outer numeric domain is unchanged.
 */
export function timelineProjectionRevision(model: TrajectoryTimelineModel | null, mode: TrajectoryTimelineMode): string {
  return JSON.stringify([mode, model === null ? null : [
    model.start, model.end,
    model.spans.map(span => [span.id, span.lane, span.start, span.end,
      span.dispatchAt, span.firstOutputAt, span.lastOutputAt, span.providerTerminalAt]),
    model.boundaries.map(boundary => [boundary.nativeAttemptId, boundary.at]),
  ]]);
}

/** Lanes group related activity, exactly as the Harness overview does. */
function laneOf(kind: TraceKind): number {
  if (kind === 'tool' || kind === 'background' || kind === 'subagent' || kind === 'workflow') return 2;
  if (kind === 'request' || kind === 'compaction') return 1;
  return 0;
}

/** Lane titles, top to bottom. */
export const TRAJECTORY_LANES = ['Input', 'Model', 'Tools'] as const;

/** Kinds whose failure state should read as an error in the overview. */
function isError(record: TraceRecord): boolean {
  return record.state === 'failed' || record.state === 'timed_out' || record.state === 'denied';
}

/**
 * The span's tooltip heading, as Harness names its overview blocks: the
 * role, not the record. A Request is the Model lane's Assistant generation.
 */
function label(tx: Translate, record: TraceRecord): string {
  if (record.kind === 'request') return tx('trajectory:kind.assistant');
  if (record.kind === 'compaction') return tx('trajectory:kind.compacted');
  return tx(`trajectory:kind.${record.kind}`);
}

export function instantMillis(value: string | null | undefined): number | undefined {
  if (value == null) return undefined;
  const parsed = Date.parse(value);
  return Number.isFinite(parsed) ? parsed : undefined;
}

/** A large integer the wire carries losslessly as a string. */
export function wireCount(value: string | number | null | undefined): number | undefined {
  if (value == null) return undefined;
  const parsed = Number(value);
  return Number.isFinite(parsed) ? parsed : undefined;
}

/** The recorded facts one record contributes to the overview. */
function timingOf(record: TraceRecord) {
  const generation = record.request?.generation ?? undefined;
  return {
    startedAt: instantMillis(record.timing.started_at),
    durationMs: wireCount(record.timing.duration_ms),
    ttftMs: wireCount(generation?.ttft_ms),
    generationMs: wireCount(generation?.generation_ms),
    timeline: generation?.timeline,
  };
}

/**
 * Project records into the overview's domain.
 *
 * `sequence` gives every record equal width, which stays readable when one
 * slow Tool would otherwise compress an entire session into a sliver.
 * Timed spans require endpoints in the rendered domain: provider timing for
 * Requests, record timing otherwise. Journal settlement never supplies a
 * Request provider endpoint. A record with no usable start is
 * omitted from the timed projection rather than placed at an invented point.
 */
export function trajectoryTimeline(tx: Translate,
  projection: TrajectoryProjection,
  mode: TrajectoryTimelineMode,
): TrajectoryTimelineModel | null {
  const records = projection.sections.flatMap(section => section.kind === 'outside'
    ? [section.record] : section.groups.flatMap(group => group.records));
  const spans: TrajectorySpan[] = [];
  const items = trajectoryItems(tx, projection).filter(isInspectable);
  const inputs = items.filter(item => item.type === 'SystemPromptCell' || item.type === 'ContextRow');
  const inputSpan = (item: typeof inputs[number], at: number): TrajectorySpan => ({
    id: item.display_key, ownerId: item.owner_record_id, displayKey: item.display_key,
    kind: item.type === 'SystemPromptCell' ? 'system' : 'context', lane: 0,
    label: tx(item.type === 'SystemPromptCell' ? 'trajectory:kind.system' : 'trajectory:kind.context'),
    error: false, start: at, end: at + (mode === 'sequence' ? 1 : 0),
  });
  const displayKey = (id: string) => items.find(item => item.owner_record_id === id && (item.type === 'RequestBoundary' || item.type === 'RecordRow'))?.display_key;

  // Derive boundaries only after projection (including idle compression).
  // Membership comes from the shared Turn model, never timestamps or adjacency.
  const boundaries = (): TrajectoryBoundary[] => {
    const starts = new Map(spans.map(span => [span.id, span.start]));
    return projection.sections.flatMap(section => {
      if (section.kind === 'outside') return [];
      const positions = section.records.flatMap(record => {
        const start = starts.get(record.id);
        return start === undefined ? [] : [start];
      });
      return positions.length ? [{ nativeAttemptId: section.nativeAttemptId, at: Math.min(...positions) }] : [];
    });
  };
  if (mode === 'sequence') {
    const initial = inputs.filter(item => item.type === 'SystemPromptCell' && item.record.request?.system_prompt.state === 'initial');
    for (const item of initial) spans.push(inputSpan(item, spans.length));
    for (const record of records) {
      if (record.kind === 'attempt' || record.kind === 'step' || record.kind === 'assistant') continue;
      const timing = timingOf(record);
      for (const item of inputs.filter(item => item.owner_record_id === record.id && !initial.includes(item))) spans.push(inputSpan(item, spans.length));
      spans.push({
        id: record.id, displayKey: displayKey(record.id),
        kind: record.kind,
        lane: laneOf(record.kind),
        label: label(tx, record),
        error: isError(record),
        start: spans.length,
        end: spans.length + 1,
        ...(timing.startedAt === undefined ? {} : { startedAt: timing.startedAt }),
        ...(timing.durationMs === undefined ? {} : { durationMs: timing.durationMs }),
        ...(timing.ttftMs === undefined ? {} : { ttftMs: timing.ttftMs }),
        ...(timing.generationMs === undefined ? {} : { generationMs: timing.generationMs }),
      });
    }
    if (spans.length === 0) return null;
    return { start: 0, end: spans.length, spans, boundaries: boundaries() };
  }
  for (const record of records) {
    const timing = timingOf(record);
    if (timing.startedAt === undefined) continue;
    if (record.kind === 'attempt' || record.kind === 'step' || record.kind === 'assistant') continue;
    spans.push({
      id: record.id, displayKey: displayKey(record.id),
      kind: record.kind,
      lane: laneOf(record.kind),
      label: label(tx, record),
      error: isError(record),
      start: timing.startedAt,
      // Endpoints must belong to the rendered domain. Even a Journal-terminal
      // Request stays a marker without the native provider timeline bridge.
      end: timing.startedAt + (record.kind === 'request' ? wireCount(timing.timeline?.terminal_ms) ?? 0 : timing.durationMs ?? 0),
      ...phasePositions(timing.startedAt, timing.timeline),
      startedAt: timing.startedAt,
      ...(timing.durationMs === undefined ? {} : { durationMs: timing.durationMs }),
      ...(timing.ttftMs === undefined ? {} : { ttftMs: timing.ttftMs }),
      ...(timing.generationMs === undefined ? {} : { generationMs: timing.generationMs }),
    });
  }
  for (const item of inputs) {
    const at = timingOf(item.record).startedAt;
    if (at !== undefined) spans.push(inputSpan(item, at));
  }
  if (spans.length === 0) return null;
  // One union-of-occupied-time transform for every lane. Overlapping work
  // shares coordinates; summing parent/child durations is never an aggregate.
  const gaps: { start: number; end: number }[] = [];
  let covered = Math.min(...spans.map(span => span.start));
  for (const span of [...spans].sort((a, b) => a.start - b.start)) {
    if (span.start > covered) gaps.push({ start: covered, end: span.start });
    covered = Math.max(covered, span.end);
  }
  const project = (at: number) => at - gaps.reduce((sum, gap) => sum + Math.max(0, Math.min(at, gap.end) - gap.start), 0);
  for (const span of spans) {
    span.start = project(span.start); span.end = project(span.end);
    for (const key of ['dispatchAt', 'firstOutputAt', 'lastOutputAt', 'providerTerminalAt'] as const) {
      if (span[key] !== undefined) span[key] = project(span[key]);
    }
  }
  return {
    start: Math.min(...spans.map(span => span.start)),
    end: Math.max(...spans.map(span => span.end)),
    spans,
    boundaries: boundaries(),
  };
}

/** Only the native bridge authorizes positions; metrics alone never do. */
function phasePositions(start: number, timeline: ReturnType<typeof timingOf>['timeline']):
  Pick<TrajectorySpan, 'dispatchAt' | 'firstOutputAt' | 'lastOutputAt' | 'providerTerminalAt'> {
  if (timeline == null) return {};
  const dispatch = wireCount(timeline.dispatch_ms);
  const first = wireCount(timeline.first_output_ms);
  const last = wireCount(timeline.last_output_ms);
  const terminal = wireCount(timeline.terminal_ms);
  return {
    ...(dispatch === undefined ? {} : { dispatchAt: start + dispatch }),
    ...(first === undefined ? {} : { firstOutputAt: start + first }),
    ...(last === undefined ? {} : { lastOutputAt: start + last }),
    ...(terminal === undefined ? {} : { providerTerminalAt: start + terminal }),
  };
}

/** Records active at any point inside an inclusive selected interval. */
export function timelineFocus(
  model: TrajectoryTimelineModel | null,
  range: TrajectoryTimeRange | null,
): ReadonlySet<string> | null {
  if (model === null || range === null) return null;
  return new Set(
    model.spans
      .filter(span => span.start <= range.end && span.end >= range.start)
      .map(span => span.ownerId ?? span.id),
  );
}

/** Harness integer-millisecond label with thousands separators, `—` when unknown. */
export function formatDurationMillis(tx: Translate, milliseconds: number | null | undefined): string {
  if (milliseconds == null || !Number.isFinite(milliseconds)) return '—';
  return tx('trajectory:unit.milliseconds', { value: String(Math.round(milliseconds)).replace(/\B(?=(\d{3})+(?!\d))/g, ',') });
}

/** Harness compact duration: milliseconds below a second, then seconds. */
export function formatDurationMs(tx: Translate, milliseconds: number): string {
  if (milliseconds < 1_000) return tx('trajectory:unit.milliseconds', { value: Math.round(milliseconds) });
  return tx('trajectory:unit.seconds', { value: (milliseconds / 1_000).toFixed(milliseconds < 10_000 ? 2 : 1) });
}

/** A recorded instant as the overview tooltip shows it: local time to the millisecond. */
export function formatRecordedTime(tx: Translate, timestamp: number): string {
  return new Date(timestamp).toLocaleTimeString(tx.language, {
    hour: '2-digit',
    minute: '2-digit',
    second: '2-digit',
    fractionalSecondDigits: 3,
  });
}

/** A recorded instant as the inspector shows it: local date and time to the millisecond. */
export function formatStartedAt(timestamp: number): string {
  const date = new Date(timestamp);
  const two = (value: number) => String(value).padStart(2, '0');
  const three = (value: number) => String(value).padStart(3, '0');
  return `${date.getFullYear()}-${two(date.getMonth() + 1)}-${two(date.getDate())} ${two(date.getHours())}:${two(date.getMinutes())}:${two(date.getSeconds())}.${three(date.getMilliseconds())}`;
}
