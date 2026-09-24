/* Copyright (c) 2026 DeepSeek. MIT. Adapted from pinned Harness ui-trajectory/trajectory-search-index.ts; see PROVENANCE.md. */
import type { TrajectoryDisplayItem } from './layout';

/** Deterministic, bounded loaded-window filter over native previews and IDs. */
export function searchItems(items: readonly TrajectoryDisplayItem[], query: string): ReadonlySet<string> | null {
  const terms = query.trim().toLowerCase().split(/\s+/).filter(Boolean);
  if (!terms.length) return null;
  const matches = new Set<string>();
  for (const item of items) {
    if (item.type === 'HistoryBoundary') continue;
    const record = item.record;
    const text = [item.label, item.preview, record.id, record.kind, record.state,
      record.location.attempt_id, record.location.step_id, record.request?.model, record.request?.request_id,
      record.request?.failure_kind, record.request?.system_prompt.preview?.text,
      record.tool?.name, record.tool?.tool_id, record.tool?.call_id, record.tool?.detail?.text,
      record.native_id, record.message_id, record.originating_tool_call_id,
      ...record.calls.map(call => `${call.name} ${call.tool_id} ${call.call_id}`),
      ...(item.type === 'ContextRow' ? [item.context.message_id, JSON.stringify(item.context.producer), item.context.context_kind, JSON.stringify(item.context.source)] : []),
    ].join('\n').toLowerCase();
    if (terms.every(term => text.includes(term))) matches.add(item.display_key);
  }
  return matches;
}
