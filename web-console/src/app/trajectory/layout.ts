/* Copyright (c) 2026 DeepSeek. MIT. Adapted from pinned Harness ui-trajectory/layout.ts; see PROVENANCE.md. */
/**
 * Fold server-resolved Trace records into the ledger's display structure.
 *
 * The one difference from the Harness fold this adapts: Harness derives turn
 * and step membership in the browser from raw session events. rustX does not
 * — every record arrives carrying the grouping the server already resolved
 * from native authority, so this module reads `location`, it never infers it.
 * Nothing here decides execution truth; it decides row order, section
 * boundaries and fold summaries.
 */
import type { TraceRecord } from '../../../../protocol/app-server/v14';

/** One ledger row: a record plus the display structure around it. */
export interface TrajectoryRow {
  record: TraceRecord;
  /** Display ordinal of the owning section, for section-level highlighting. */
  section: number;
  /** The section's Attempt, or null for records committed outside one. */
  attempt: string | null;
  /** Stable group key inside the section. */
  group: string;
  /** Human label of the group. */
  groupLabel: string;
  sectionStart: boolean;
  sectionEnd: boolean;
  groupStart: boolean;
  /** Session-global request number, on the row that opens a request group. */
  requestNumber?: number;
  /** Set on a synthetic row standing in for a folded run. */
  collapsedSummary?: string;
  collapsedKind?: 'attempt' | 'step';
}

/**
 * The section a record with no Attempt belongs to.
 *
 * Harness places an unassigned user message into the turn that answers it.
 * rustX cannot: which Attempt answered an adopted inbound batch is native
 * execution truth, and the browser may not decide it. Such records keep
 * their own chronological section instead, the same shape Harness gives a
 * standalone compaction.
 */
const SECTION_OUTSIDE = 'trajectory:outside-attempt';

/** Composite key, built so no component value can forge another key. */
function key(...parts: (string | null)[]): string {
  return JSON.stringify(parts);
}

function groupKeyOf(record: TraceRecord): string {
  return record.location.step_id == null ? 'attempt' : `step:${record.location.step_id}`;
}

function groupLabelOf(record: TraceRecord): string {
  return record.location.step_id == null ? 'Attempt' : `Step ${record.location.step_id}`;
}

/** The section key of one record. */
export function sectionKeyOf(record: TraceRecord): string {
  return record.location.attempt_id ?? SECTION_OUTSIDE;
}

/** The fold key of one Step group, scoped to its own Attempt. */
export function stepFoldKey(record: TraceRecord): string {
  return key(sectionKeyOf(record), groupKeyOf(record));
}

/**
 * Section label shown on the row that opens a section.
 *
 * Sections use native Attempt ownership, not Harness Turn semantics.
 * Ordinals name only the sections visible in this loaded window.
 */
export function sectionLabel(attempt: string | null, ordinal: number): string {
  return attempt == null ? 'Outside an Attempt' : `Attempt ${ordinal}`;
}

/**
 * Project the loaded window into ledger rows.
 *
 * Records keep server order. A section opens at the first appearance of its
 * Attempt in the loaded window, so a page that begins mid-Attempt still
 * opens a section rather than claiming the Attempt started there.
 */
export function trajectoryRows(records: readonly TraceRecord[]): TrajectoryRow[] {
  const sectionOrdinals = new Map<string, number>();
  let requestNumber = 0;
  const rows: TrajectoryRow[] = records.map(record => {
    const sectionKey = sectionKeyOf(record);
    let section = sectionOrdinals.get(sectionKey);
    if (section === undefined) sectionOrdinals.set(sectionKey, (section = sectionOrdinals.size));
    return {
      record,
      section,
      attempt: record.location.attempt_id ?? null,
      group: key(sectionKey, groupKeyOf(record)),
      groupLabel: groupLabelOf(record),
      sectionStart: false,
      sectionEnd: false,
      groupStart: false,
      // Requests are counted across the loaded window in server order, so a
      // reader can name "request 7 in the loaded window". It is a display
      // ordinal scoped to what is loaded, never a stable request identity:
      // the native retry ordinal and request id stay on the record itself.
      ...(record.kind === 'request' ? { requestNumber: ++requestNumber } : {}),
    };
  });
  for (const [index, row] of rows.entries()) {
    const previous = rows[index - 1];
    const next = rows[index + 1];
    row.sectionStart = previous === undefined || previous.section !== row.section;
    row.sectionEnd = next === undefined || next.section !== row.section;
    row.groupStart = previous === undefined || previous.group !== row.group;
  }
  return rows;
}

/** Display ordinals for Attempt sections, so a label can read `Attempt 3`. */
export function sectionOrdinals(rows: readonly TrajectoryRow[]): Map<number, number> {
  const ordinals = new Map<number, number>();
  let next = 0;
  for (const row of rows) {
    if (row.attempt === null || ordinals.has(row.section)) continue;
    ordinals.set(row.section, ++next);
  }
  return ordinals;
}

/** A one-line summary of what a folded run of rows contains. */
function summarize(rows: readonly TrajectoryRow[]): string {
  const counts = new Map<string, number>();
  for (const row of rows) {
    const label = row.record.tool?.name ?? (row.record.kind === 'assistant' ? 'Assistant' : row.record.kind);
    counts.set(label, (counts.get(label) ?? 0) + 1);
  }
  return [...counts].map(([label, count]) => (count > 1 ? `${label} x${count}` : label)).join(' · ');
}

/**
 * Replace the rows after the first of each folded run with one summary row.
 *
 * The opening row stays visible so the reader can still see, and unfold,
 * what was folded: a folded run never hides its own boundary.
 */
export function foldRows(
  rows: readonly TrajectoryRow[],
  folded: ReadonlySet<string>,
  kind: 'attempt' | 'step',
  keyOf: (row: TrajectoryRow) => string,
): TrajectoryRow[] {
  if (folded.size === 0) return [...rows];
  const result: TrajectoryRow[] = [];
  let index = 0;
  while (index < rows.length) {
    const row = rows[index]!;
    const runKey = keyOf(row);
    if (!folded.has(runKey)) {
      result.push(row);
      index += 1;
      continue;
    }
    const run: TrajectoryRow[] = [];
    while (index < rows.length && keyOf(rows[index]!) === runKey) run.push(rows[index++]!);
    const [first, ...rest] = run;
    result.push(first!);
    if (rest.length > 0) result.push({ ...first!, collapsedSummary: summarize(rest), collapsedKind: kind });
  }
  return result;
}

/** Fold keys whose run holds more than one row, so folding is meaningful. */
export function foldableKeys(
  rows: readonly TrajectoryRow[],
  keyOf: (row: TrajectoryRow) => string,
): string[] {
  const counts = new Map<string, number>();
  for (const row of rows) {
    const runKey = keyOf(row);
    counts.set(runKey, (counts.get(runKey) ?? 0) + 1);
  }
  return [...counts].filter(([, count]) => count > 1).map(([runKey]) => runKey);
}
