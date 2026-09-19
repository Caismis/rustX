/* Copyright (c) 2026 DeepSeek. MIT. Adapted from pinned Harness ui-trajectory/trajectory-search-index.ts; see PROVENANCE.md. */
/**
 * Loaded-window search over the Trace ledger.
 *
 * Search covers exactly what the browser has loaded, and says so: it is not
 * a server query, so a record outside the loaded window is not a miss, it is
 * simply not searched yet. The index is rebuilt only for records whose
 * searchable text actually changed, so a live lifecycle repair does not
 * reindex the window.
 */
import type { TraceRecord } from '../../../../protocol/app-server/v12';
import type { TrajectoryRow } from './layout';

interface SearchEntry {
  readonly sources: readonly string[];
  readonly text: string;
}

/** The searchable text of one row, in the order a reader would scan it. */
function sourcesOf(row: TrajectoryRow): readonly string[] {
  const record: TraceRecord = row.record;
  return [
    row.groupLabel,
    row.attempt ?? 'outside an attempt',
    record.kind,
    record.state,
    record.preview?.text ?? '',
    record.request?.model ?? '',
    record.request?.request_id ?? '',
    record.request?.failure_kind ?? '',
    record.request?.previous_failure_kind ?? '',
    record.tool?.name ?? '',
    record.tool?.tool_id ?? '',
    record.tool?.call_id ?? '',
    record.tool?.outcome ?? '',
    record.tool?.detail?.text ?? '',
    record.native_id ?? '',
    record.originating_tool_call_id ?? '',
    record.message_id ?? '',
    // Bounded semantic labels and identities from the server-resolved
    // relationships. Search indexes what the server already decided; it
    // never becomes the authority for any of these relations.
    record.request?.system_prompt.state ?? '',
    ...(record.request?.context_additions ?? []).map(
      addition =>
        `${addition.context_kind} ${addition.source.type} ${
          addition.source.type === 'certified_extension' ? addition.source.contributor : ''
        } ${addition.message_id}`,
    ),
    ...record.calls.map(call => `${call.name} ${call.tool_id} ${call.call_id}`),
  ];
}

function sameSources(left: readonly string[], right: readonly string[]): boolean {
  return left.length === right.length && left.every((value, index) => value === right[index]);
}

/** Incremental index over the loaded ledger window. */
export class TrajectorySearchIndex {
  private readonly entries = new Map<string, SearchEntry>();
  private rows: readonly TrajectoryRow[] | undefined;

  /**
   * Synchronize the index with the current rows.
   * @returns whether the indexed content changed.
   */
  update(rows: readonly TrajectoryRow[]): boolean {
    if (this.rows === rows) return false;
    this.rows = rows;
    const seen = new Set<string>();
    for (const row of rows) {
      if (row.collapsedSummary !== undefined) continue;
      const id = row.record.id;
      const sources = sourcesOf(row);
      const previous = this.entries.get(id);
      const entry =
        previous !== undefined && sameSources(previous.sources, sources)
          ? previous
          : { sources, text: sources.join('\n').toLocaleLowerCase() };
      this.entries.set(id, entry);
      seen.add(id);
    }
    for (const id of [...this.entries.keys()]) if (!seen.has(id)) this.entries.delete(id);
    return true;
  }

  /**
   * Match a query against the indexed window.
   * @returns matching record identities, or null when there is no query.
   */
  search(query: string): ReadonlySet<string> | null {
    const terms = query.trim().toLocaleLowerCase().split(/\s+/).filter(Boolean);
    if (terms.length === 0) return null;
    const matches = new Set<string>();
    for (const [id, entry] of this.entries) {
      if (terms.every(term => entry.text.includes(term))) matches.add(id);
    }
    return matches;
  }
}
