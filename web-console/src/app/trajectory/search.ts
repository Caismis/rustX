/* Copyright (c) 2026 DeepSeek. MIT. Adapted from pinned Harness ui-trajectory/trajectory-search-index.ts; see PROVENANCE.md. */
import { recordLabel, systemPresentation, type InspectableDisplayItem, type TrajectoryProjection } from './layout';
import { translator } from '../../locale/translation';

// Search accepts both built-in vocabularies regardless of the active locale.
// Display labels and fallback previews must never change result membership.
const vocabulary = [translator('en'), translator('zh')];

/** Cell keys are the only search results. Structural context comes from the
 * shared projection; native owners never expand a match into sibling cells. */
export function searchItems(projection: TrajectoryProjection, query: string): ReadonlySet<string> | null {
  const terms = query.trim().toLowerCase().replace(/\s+/g, ' ').match(/\b(?:turn|step) \d+\b|\S+/g) ?? [];
  if (!terms.length) return null;
  const matches = new Set<string>();
  const match = (item: InspectableDisplayItem, structure: readonly string[]) => {
    const record = item.record;
    const labels = item.type === 'ContextRow' ? [item.context.context_kind.replaceAll('_', ' ')]
      : vocabulary.map(tx => item.type === 'SystemPromptCell' ? systemPresentation(tx, record)?.label : recordLabel(tx, record));
    const preview = item.type === 'ContextRow' ? item.context.preview?.text
      : item.type === 'SystemPromptCell' ? record.request?.system_prompt.preview?.text
        : record.preview?.text;
    const text = [...structure, ...labels, preview, record.id, record.state,
      record.location.attempt_id, record.location.step_id, record.request?.request_id,
      ...(item.type === 'RequestBoundary' ? [record.request?.model, record.request?.failure_kind] : []),
      record.tool?.name, record.tool?.tool_id, record.tool?.call_id, record.tool?.detail?.text,
      record.native_id, record.message_id, record.originating_tool_call_id,
      ...record.calls.map(call => `${call.name} ${call.tool_id} ${call.call_id}`),
      ...(item.type === 'ContextRow' ? [item.context.message_id, JSON.stringify(item.context.producer), item.context.context_kind, JSON.stringify(item.context.source)] : []),
    ].join('\n').toLowerCase();
    if (terms.every(term => /^(turn|step) \d+$/.test(term)
      ? structure.some(label => label.toLowerCase() === term) : text.includes(term))) matches.add(item.display_key);
  };
  for (const section of projection.sections) {
    if (section.kind === 'outside') {
      for (const cell of section.cells) match(cell, []);
    } else {
      let step = 0;
      for (const group of section.groups) {
        if (group.kind === 'step') step++;
        const structure = vocabulary.flatMap(tx => [
          tx('trajectory:copy.turn-value', { p0: section.displayOrdinal }),
          group.kind === 'step' ? tx('trajectory:group.step', { n: step }) : tx('trajectory:group.message'),
        ]);
        for (const cell of group.cells) match(cell, structure);
      }
    }
  }
  return matches;
}
