/* Copyright (c) 2026 DeepSeek. MIT. Adapted from pinned Harness ui-trajectory/timeline.ts; see PROVENANCE.md. */
/**
 * Timing projections for the Trajectory overview.
 *
 * Every span comes from two authoritative timestamps the server recorded. A
 * record with only a start has no span: it contributes a start marker, and
 * the overview never fills the missing end with render time, reconnect time
 * or `Date.now()`. An Assistant request additionally splits its span into
 * the TTFT and decode halves the server settled, so the visual division is
 * evidence rather than an estimate.
 */
import type { TraceKind, TraceRecord } from '../../../../protocol/app-server/v9';

/** Horizontal projection of the overview's domain. */
export type TrajectoryTimelineMode = 'sequence' | 'duration';

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
  /** Fraction of the span spent before the first model output, when known. */
  ttftFraction?: number;
  /** Exact recorded start in epoch milliseconds, when known. */
  startedAt?: number;
  /** Exact recorded duration in milliseconds, when known. */
  durationMs?: number;
  ttftMs?: number;
  generationMs?: number;
}

/** One Attempt boundary in the active domain. */
export interface TrajectoryBoundary {
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
  if (kind === 'tool' || kind === 'background') return 2;
  if (kind === 'request' || kind === 'assistant' || kind === 'compaction') return 1;
  return 0;
}

/** Lane titles, top to bottom. */
export const TRAJECTORY_LANES = ['Session', 'Model', 'Execution'] as const;

/** Kinds whose failure state should read as an error in the overview. */
function isError(record: TraceRecord): boolean {
  return record.state === 'failed' || record.state === 'timed_out' || record.state === 'denied';
}

/**
 * The span's accessible name.
 *
 * It names the record, not just its kind: two requests of one Step differ
 * only by their native ordinal, and two Tool spans only by their call, so a
 * label that omitted those would make the overview ambiguous to a screen
 * reader and to the ledger it links to.
 */
function label(record: TraceRecord): string {
  if (record.kind === 'request' && record.request) {
    return `Request #${record.request.retry_number} · ${record.request.model}`;
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
  };
}

/**
 * Project records into the overview's domain.
 *
 * `sequence` gives every record equal width, which stays readable when one
 * slow Tool would otherwise compress an entire session into a sliver.
 * `duration` uses recorded wall time. A record with no usable start is
 * omitted from the timed projection rather than placed at an invented point.
 */
export function trajectoryTimeline(
  records: readonly TraceRecord[],
  mode: TrajectoryTimelineMode,
  sectionLabelOf: (record: TraceRecord, index: number) => string | undefined,
): TrajectoryTimelineModel | null {
  const spans: TrajectorySpan[] = [];
  const boundaries: TrajectoryBoundary[] = [];
  if (mode === 'sequence') {
    for (const [index, record] of records.entries()) {
      const boundary = sectionLabelOf(record, index);
      if (boundary !== undefined) boundaries.push({ label: boundary, at: spans.length });
      const timing = timingOf(record);
      spans.push({
        id: record.id,
        kind: record.kind,
        lane: laneOf(record.kind),
        label: label(record),
        error: isError(record),
        start: spans.length,
        end: spans.length + 1,
        ...ttftFraction(timing),
        ...(timing.startedAt === undefined ? {} : { startedAt: timing.startedAt }),
        ...(timing.durationMs === undefined ? {} : { durationMs: timing.durationMs }),
        ...(timing.ttftMs === undefined ? {} : { ttftMs: timing.ttftMs }),
        ...(timing.generationMs === undefined ? {} : { generationMs: timing.generationMs }),
      });
    }
    if (spans.length === 0) return null;
    return { start: 0, end: spans.length, spans, boundaries };
  }
  for (const [index, record] of records.entries()) {
    const timing = timingOf(record);
    if (timing.startedAt === undefined) continue;
    const boundary = sectionLabelOf(record, index);
    if (boundary !== undefined) boundaries.push({ label: boundary, at: timing.startedAt });
    spans.push({
      id: record.id,
      kind: record.kind,
      lane: laneOf(record.kind),
      label: label(record),
      error: isError(record),
      start: timing.startedAt,
      // An in-flight or unterminated record is a marker, not a span: its end
      // equals its start, so nothing on screen claims a duration it lacks.
      end: timing.startedAt + (timing.durationMs ?? 0),
      ...ttftFraction(timing),
      startedAt: timing.startedAt,
      ...(timing.durationMs === undefined ? {} : { durationMs: timing.durationMs }),
      ...(timing.ttftMs === undefined ? {} : { ttftMs: timing.ttftMs }),
      ...(timing.generationMs === undefined ? {} : { generationMs: timing.generationMs }),
    });
  }
  if (spans.length === 0) return null;
  return {
    start: Math.min(...spans.map(span => span.start)),
    end: Math.max(...spans.map(span => span.end)),
    spans,
    boundaries,
  };
}

/** The TTFT share of a request's span, when both halves are recorded. */
function ttftFraction(timing: {
  ttftMs?: number | undefined;
  generationMs?: number | undefined;
}): { ttftFraction?: number } {
  const { ttftMs, generationMs } = timing;
  if (ttftMs === undefined || generationMs === undefined) return {};
  const total = ttftMs + generationMs;
  return total > 0 ? { ttftFraction: ttftMs / total } : {};
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
