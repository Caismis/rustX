/* Copyright (c) 2026 DeepSeek. MIT. Adapted from pinned Harness TrajectoryTable.tsx semantic cells; see PROVENANCE.md. */
import type {
  TraceArtifact,
  TraceKind,
  TraceRecord,
  TraceSystemPromptState,
} from '../../../../protocol/app-server/v23';
import { IconSparkle16, IconUserOutline16 } from '../../presentation/primitives/icons';
import { MarkdownText } from '../../presentation/markdown/MarkdownText';
import type { InspectableDisplayItem } from './layout';
import css from './Trajectory.module.css';

/** Display choices only. Native kind, location, lifecycle and identity stay on the record. */
export const cellLabel: Record<TraceKind, string> = {
  user: 'User', assistant: 'Assistant', tool: 'Tool', compaction: 'Compaction',
  attempt: 'Attempt', step: 'Step', request: 'Request',
  background: 'Background', subagent: 'Subagent', workflow: 'Workflow', interaction: 'Interaction',
};

/**
 * Compact labels for the kinds that share the generic fallback glyph.
 *
 * `background`, `subagent`, `workflow` and `interaction` are rustX-specific
 * evidence with no Harness counterpart and no distinct icon, so at narrow
 * widths the icon alone cannot tell them apart. They keep a short visible
 * word instead of relying on a hover Tooltip that a touch reader never gets.
 */
export const cellNarrowLabel: Partial<Record<TraceKind, string>> = {
  background: 'BG',
  subagent: 'SUBAGENT',
  workflow: 'WORKFLOW',
  interaction: 'INTERACT',
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

/**
 * The name a ledger row shows for one artifact.
 *
 * Native `name` when the record carries one. Otherwise a neutral word for
 * what it is: the artifact id is a machine identity and belongs in the
 * inspector, not in the primary label of an ordinary row.
 */
export function artifactLabel(artifact: TraceArtifact): string {
  return artifact.name?.trim() || (artifact.image ? 'Image' : 'File');
}

/**
 * Compact artifact identity for an ordinary row.
 *
 * Summary-only by construction: it reads the artifact facts already on the
 * record and never touches `ArtifactResources`, so scrolling a virtualized
 * history cannot start a resource read. Loading and preview stay in the
 * inspector's Artifacts section.
 */
export function CellArtifacts({ artifacts }: { artifacts: readonly TraceArtifact[] }) {
  const [first, ...rest] = artifacts;
  if (first === undefined) return null;
  return (
    <span
      className={css.attachment}
      data-image={first.image || undefined}
      title={
        rest.length === 0
          ? artifactLabel(first)
          : [first, ...rest].map(artifactLabel).join(' · ')
      }
    >
      <span className={css.attachmentIcon} aria-hidden="true">
        {first.image ? (
          <svg width="11" height="11" viewBox="0 0 16 16" fill="none" stroke="currentColor" strokeWidth="1.5" strokeLinejoin="round">
            <rect x="2" y="3" width="12" height="10" rx="1.5" />
            <circle cx="6" cy="6.5" r="1" /><path d="m3 11.5 3-3 2.5 2.5L11 8l2 2" />
          </svg>
        ) : (
          <svg width="11" height="11" viewBox="0 0 16 16" fill="none" stroke="currentColor" strokeWidth="1.5" strokeLinejoin="round">
            <path d="M9 2H4.5A1.5 1.5 0 0 0 3 3.5v9A1.5 1.5 0 0 0 4.5 14h7a1.5 1.5 0 0 0 1.5-1.5V6L9 2Z" /><path d="M9 2v4h4" />
          </svg>
        )}
      </span>
      <span className={css.attachmentName}>{artifactLabel(first)}</span>
      {rest.length > 0 && <span className={css.attachmentMore}>+{rest.length}</span>}
    </span>
  );
}

/**
 * How a row labels the server's System Prompt classification.
 *
 * The browser renders this relationship; it never derives it. `unchanged`
 * and `previous_unavailable` get no row marker: a row says something when
 * this request introduced or replaced the prompt, and stays quiet otherwise.
 */
const systemPromptLabel: Partial<Record<TraceSystemPromptState, string>> = {
  initial: 'System prompt',
  changed: 'System prompt changed',
};

/**
 * Server-resolved relationships shown beside a request row.
 *
 * Every value comes from the record's own summary, so a row keeps its
 * meaning when the related record is outside the loaded window.
 */
export function CellRelations({ record }: { record: TraceRecord }) {
  const request = record.request;
  if (!request) return null;
  const system = systemPromptLabel[request.system_prompt.state];
  const context = request.context_additions.length;
  if (system === undefined && context === 0) return null;
  return (
    <span
      className={css.relation}
      data-changed={request.system_prompt.state === 'changed' || undefined}
      title={request.system_prompt.preview?.text || undefined}
    >
      {system}
      {context > 0 && (
        <span>
          {system === undefined ? '' : '· '}
          {context} context fact{context === 1 ? '' : 's'}
          {request.context_truncated ? '+' : ''}
        </span>
      )}
    </span>
  );
}

export function CellContent({ record }: { record: TraceRecord }) {
  const preview = previewOf(record);
  const result = record.tool?.detail?.text || (record.tool?.outcome === 'success' ? undefined : record.tool?.outcome);
  return <>
    {record.tool && <strong className={css.toolName}>{record.tool.name ?? record.tool.tool_id}</strong>}
    <div className={css.preview}>
      {record.kind === 'assistant' || record.kind === 'user' || record.kind === 'compaction'
        ? <div className={css.markdownPreview} inert><MarkdownText text={preview} /></div>
        : preview}
    </div>
    {result && <span className={css.result} data-error={record.state === 'failed' || undefined}>→ {result}</span>}
    <CellRelations record={record} />
    <CellArtifacts artifacts={record.attachments} />
  </>;
}

/** Dedicated prompt cell: its request identity and native classification survive paging. */
export function SystemPromptCell({ cell }: { cell: Extract<InspectableDisplayItem, { type: 'SystemPromptCell' }> }) {
  return <span className={css.preview} data-system-prompt-state={cell.record.request?.system_prompt.state} data-tool-catalog-state={cell.record.request?.tool_catalog}>
    <strong>{cell.label}</strong> · {cell.preview || 'Empty'}
  </span>;
}
