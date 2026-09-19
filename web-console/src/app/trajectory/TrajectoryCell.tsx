/* Copyright (c) 2026 DeepSeek. MIT. Adapted from pinned Harness TrajectoryTable.tsx semantic cells; see PROVENANCE.md. */
import type { TraceKind, TraceRecord } from '../../../../protocol/app-server/v11';
import { IconSparkle16, IconUserOutline16 } from '../../presentation/primitives/icons';
import { MarkdownText } from '../../presentation/markdown/MarkdownText';
import css from './Trajectory.module.css';

/** Display choices only. Native kind, location, lifecycle and identity stay on the record. */
export const cellLabel: Record<TraceKind, string> = {
  user: 'User', assistant: 'Assistant', tool: 'Tool', compaction: 'Compaction',
  attempt: 'Attempt', step: 'Step', request: 'Request',
  background: 'Background', subagent: 'Subagent', workflow: 'Workflow', interaction: 'Interaction',
};

export function previewOf(record: TraceRecord): string {
  if (record.preview?.text) return record.preview.text;
  if (record.kind === 'assistant' && record.calls.length) return `Tool calls · ${record.calls.map(call => call.name).join(', ')}`;
  if (record.request) return record.request.model;
  if (record.attachments.length) return `${record.attachments.length} attachment${record.attachments.length === 1 ? '' : 's'}`;
  return record.kind === 'attempt' || record.kind === 'step' ? '' : 'No preview recorded';
}

/** Same role glyphs as Harness; no Tool-name inference. */
export function CellIcon({ kind }: { kind: TraceKind }) {
  if (kind === 'assistant') return <IconSparkle16 size={13} />;
  if (kind === 'user') return <IconUserOutline16 size={13} />;
  return <svg width="13" height="13" viewBox="0 0 16 16" fill="none" stroke="currentColor" strokeWidth="1.5" strokeLinecap="round" strokeLinejoin="round" aria-hidden="true">
    {kind === 'tool' ? <path d="M14 3.3a3.8 3.8 0 0 1-4.8 4.8l-5.1 5.1a1.6 1.6 0 1 1-2.3-2.3l5.1-5.1A3.8 3.8 0 0 1 11.7 1l-2.3 2.3 2.3 2.3L14 3.3Z" />
      : kind === 'compaction' ? <><path d="m2.5 2.5 3.75 3.75M3 6.25h3.25V3m7.25-.5-3.75 3.75M13 6.25H9.75V3m-7.25 10.5 3.75-3.75M3 9.75h3.25V13m7.25.5-3.75-3.75M13 9.75H9.75V13" /></>
      : <><circle cx="8" cy="8" r="6" /><path d="M8 7v4M8 4.5v.5" /></>}
  </svg>;
}

export function CellContent({ record }: { record: TraceRecord }) {
  const preview = previewOf(record);
  const result = record.tool?.detail?.text || record.tool?.outcome;
  return <>
    {record.tool && <strong className={css.toolName}>{record.tool.name ?? record.tool.tool_id}</strong>}
    <div className={css.preview}>
      {record.kind === 'assistant' || record.kind === 'user' || record.kind === 'compaction'
        ? <div className={css.markdownPreview} inert><MarkdownText text={preview} /></div>
        : preview}
    </div>
    {result && <span className={css.result} data-error={record.state === 'failed' || undefined}>→ {result}</span>}
    {record.attachments.length > 0 && <span className={css.attachmentCount} title="Open the record to inspect attachments">▧ {record.attachments.length}</span>}
  </>;
}
