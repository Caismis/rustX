/* Copyright (c) 2026 DeepSeek. MIT. Adapted from pinned Harness ui-trajectory/timeline.ts; see PROVENANCE.md. */
/**
 * Timing projections for the Trajectory overview.
 *
 * Generation phases use request-relative monotonic offsets supplied by Trace,
 * anchored through the native runtime's paired durable-start clock reading.
 * Journal wall spans and dispatch-origin numeric metrics cannot supply that
 * relationship. Missing bridge evidence leaves a request as a marker, with separate numeric metrics.
 */
import type { TrajectoryProjection } from './layout';
import type { TraceKind, TraceRecord } from '../../../../protocol/app-server/v23';

/** Horizontal projection of the overview's domain. */
export type TrajectoryTimelineMode = 'sequence' | 'duration' | 'time' | 'actual';

/** Inclusive selection in the active projection's domain. */
export interface TrajectoryTimeRange {
  start: number;
  end: number;
}

/** One record projected into the active domain. */
export interface TrajectorySpan extends TrajectoryTimeRange {
  id: string;
  kind: TraceKind;
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
  label: string;
  at: number;
}

/** The overview's complete domain. */
export interface TrajectoryTimelineModel extends TrajectoryTimeRange {
  spans: readonly TrajectorySpan[];
  boundaries: readonly TrajectoryBoundary[];
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
 * The span's accessible name.
 *
 * It names the record, not just its kind: two Tool spans differ only by
 * their call, so a label that omitted that would make the overview
 * ambiguous to a screen reader and to the ledger it links to. Requests are
 * named by model and separated by their exact native request identity.
 *
 * `retry_number` stays out of this label. Native counts actual requests
 * within a logical Step, so a nonzero ordinal proves only that the request
 * was not the first one — not whether it was a retry or a recovery. The
 * ordinal belongs in the inspector's native disclosure, where it is named
 * for what it is; `request_id` is the stable disambiguator here.
 */
function label(record: TraceRecord): string {
  if (record.kind === 'request' && record.request) {
    return `Request · ${record.request.model} · ${record.request.request_id}`;
  }
  if (record.kind === 'tool' && record.tool) {
    return `Tool · ${record.tool.name ?? record.tool.tool_id} · ${record.tool.call_id}`;
  }
  return `${record.kind} · ${record.id}`;
}

function millis(value: string | null | undefined): number | undefined {
  if (value == null) return undefined;
  const parsed = Date.parse(value);
  return Number.isFinite(parsed) ? parsed : undefined;
}

/** A large integer the wire carries losslessly as a string. */
function count(value: string | number | null | undefined): number | undefined {
  if (value == null) return undefined;
  const parsed = Number(value);
  return Number.isFinite(parsed) ? parsed : undefined;
}

/** The recorded facts one record contributes to the overview. */
function timingOf(record: TraceRecord) {
  const generation = record.request?.generation ?? undefined;
  return {
    startedAt: millis(record.timing.started_at),
    durationMs: count(record.timing.duration_ms),
    ttftMs: count(generation?.ttft_ms),
    generationMs: count(generation?.generation_ms),
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
export function trajectoryTimeline(
  projection: TrajectoryProjection,
  mode: TrajectoryTimelineMode,
): TrajectoryTimelineModel | null {
  const records = projection.sections.flatMap(section => section.kind === 'outside'
    ? [section.record] : section.groups.flatMap(group => group.records));
  const spans: TrajectorySpan[] = [];
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
      return positions.length ? [{ nativeAttemptId: section.nativeAttemptId,
        label: `Turn ${section.displayOrdinal}`, at: Math.min(...positions) }] : [];
    });
  };
  if (mode === 'sequence') {
    for (const record of records) {
      if (record.kind === 'attempt' || record.kind === 'step' || record.kind === 'assistant') continue;
      const timing = timingOf(record);
      spans.push({
        id: record.id,
        kind: record.kind,
        lane: laneOf(record.kind),
        label: label(record),
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
      id: record.id,
      kind: record.kind,
      lane: laneOf(record.kind),
      label: label(record),
      error: isError(record),
      start: timing.startedAt,
      // Endpoints must belong to the rendered domain. Even a Journal-terminal
      // Request stays a marker without the native provider timeline bridge.
      end: timing.startedAt + (record.kind === 'request' ? count(timing.timeline?.terminal_ms) ?? 0 : timing.durationMs ?? 0),
      ...phasePositions(timing.startedAt, timing.timeline),
      startedAt: timing.startedAt,
      ...(timing.durationMs === undefined ? {} : { durationMs: timing.durationMs }),
      ...(timing.ttftMs === undefined ? {} : { ttftMs: timing.ttftMs }),
      ...(timing.generationMs === undefined ? {} : { generationMs: timing.generationMs }),
    });
  }
  if (spans.length === 0) return null;
  // One union-of-occupied-time transform for every lane. Overlapping work
  // shares coordinates; summing parent/child durations is never an aggregate.
  if (mode === 'duration') {
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
  } else if (mode === 'time') {
    for (const span of spans) {
      span.end = span.start;
      delete span.dispatchAt; delete span.firstOutputAt; delete span.lastOutputAt; delete span.providerTerminalAt;
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
  const dispatch = count(timeline.dispatch_ms);
  const first = count(timeline.first_output_ms);
  const last = count(timeline.last_output_ms);
  const terminal = count(timeline.terminal_ms);
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
      .map(span => span.id),
  );
}

/** Format a duration the way the Harness overview labels one. */
export function formatDuration(milliseconds: number | null | undefined): string {
  if (milliseconds == null || !Number.isFinite(milliseconds)) return 'Unavailable';
  if (milliseconds < 1000) return `${Math.round(milliseconds)} ms`;
  return `${(milliseconds / 1000).toFixed(milliseconds < 10_000 ? 2 : 1)} s`;
}

/** Format an exact recorded instant, or say it is unavailable. */
export function formatInstant(value: string | null | undefined): string {
  const parsed = millis(value);
  if (parsed === undefined) return 'Unavailable';
  return new Date(parsed).toLocaleTimeString(undefined, {
    hour: '2-digit',
    minute: '2-digit',
    second: '2-digit',
    fractionalSecondDigits: 3,
  });
}
