import type { Translate } from '../../locale/translation';
import { useTranslation } from '../../locale/react';
/* Copyright (c) 2026 DeepSeek. MIT. Adapted from pinned Harness TrajectoryTable.tsx semantic cells; see PROVENANCE.md. */
import type { ReactNode } from 'react';
import type { TraceKind, TraceRecord } from '../../../../protocol/app-server/v37';
import { IconSettingsOutline16, IconSparkle16, IconUserOutline16 } from '../../presentation/primitives/icons';
import { Tooltip } from '../../presentation/primitives/Tooltip';
import type { InspectableDisplayItem } from './layout';
import { trajectoryPreviewText } from './preview';
import css from './Trajectory.module.css';

/** The ledger's role vocabulary: native kinds plus the two input cells. */
export type CellKind = TraceKind | 'system' | 'context';

export function cellKind(item: InspectableDisplayItem): CellKind {
  return item.type === 'SystemPromptCell' ? 'system' : item.type === 'ContextRow' || item.record.kind === 'user' && !!item.record.agent_id ? 'context' : item.record.kind;
}

/** Harness role labels; a compaction record is shown as its Compacted result. */
export function cellLabel(tx: Translate, kind: CellKind): string {
  return kind === 'compaction' ? tx('trajectory:kind.compacted') : tx(`trajectory:kind.${kind}`);
}

/** Failure states read as errors, as Harness marks a failed Tool or request. */
export function isErrorRecord(record: TraceRecord): boolean {
  return record.state === 'failed' || record.state === 'timed_out' || record.state === 'denied';
}

/**
 * Compact labels for the kinds that share no Harness glyph.
 *
 * `background`, `subagent`, `workflow` and `interaction` are rustX-specific
 * evidence with no distinct icon, so at narrow widths the icon alone cannot
 * tell them apart. They keep a short visible word instead.
 */
function narrowLabel(tx: Translate, kind: CellKind): string | undefined {
  switch (kind) {
    case 'background': return tx('trajectory:short.background');
    case 'subagent': return tx('trajectory:short.subagent');
    case 'workflow': return tx('trajectory:short.workflow');
    case 'interaction': return tx('trajectory:short.interaction');
    default: return undefined;
  }
}

function Glyph({ children }: { children: ReactNode }) {
  return <svg width="13" height="13" viewBox="0 0 16 16" fill="none" stroke="currentColor" strokeWidth="1.5" strokeLinecap="round" strokeLinejoin="round" aria-hidden="true">{children}</svg>;
}

/** Same role glyphs as Harness. */
function CellIcon({ kind }: { kind: CellKind }) {
  switch (kind) {
    case 'system': return <IconSettingsOutline16 size={13} />;
    case 'user': return <IconUserOutline16 size={13} />;
    case 'assistant': return <IconSparkle16 size={13} />;
    case 'tool': return <Glyph><path d="M14 3.3a3.8 3.8 0 0 1-4.8 4.8l-5.1 5.1a1.6 1.6 0 1 1-2.3-2.3l5.1-5.1A3.8 3.8 0 0 1 11.7 1l-2.3 2.3 2.3 2.3L14 3.3Z" /></Glyph>;
    case 'compaction': return <Glyph><path d="m2.5 2.5 3.75 3.75M3 6.25h3.25V3" /><path d="m13.5 2.5-3.75 3.75M13 6.25H9.75V3" /><path d="m2.5 13.5 3.75-3.75M3 9.75h3.25V13" /><path d="m13.5 13.5-3.75-3.75M13 9.75H9.75V13" /></Glyph>;
    default: return <svg width="14" height="14" viewBox="0 0 16 16" fill="none" stroke="currentColor" strokeWidth="1.4" strokeLinecap="round" aria-hidden="true"><circle cx="8" cy="8" r="6.7" /><circle cx="8" cy="5.5" r=".85" fill="currentColor" stroke="none" /><path d="M8 7.75v3.4" strokeWidth="1.8" /></svg>;
  }
}

/** The Harness role tag: a word, or its glyph once the ledger is too narrow. */
export function KindTag({ kind }: { kind: CellKind }) {
  const tx = useTranslation();
  const label = cellLabel(tx, kind);
  const short = narrowLabel(tx, kind);
  return <span className={css.kindSlot}>
    <span className={css.kindTag} data-role-kind={kind}>
      {short === undefined
        ? <Tooltip label={label} side="right"><span className={css.kindTagIcon} aria-hidden="true"><CellIcon kind={kind} /></span></Tooltip>
        : <span className={css.kindTagShort} aria-hidden="true">{short}</span>}
      <span className={css.kindTagLabel}>{label}</span>
    </span>
  </span>;
}

/** Harness's attachment summary for an input row: "Images ×2 · Files ×1". */
function attachmentSummary(tx: Translate, record: TraceRecord): string {
  const images = record.attachments.filter(artifact => artifact.image).length;
  const files = record.attachments.length - images;
  return [
    images > 0 ? tx('trajectory:layout.image-count', { count: images }) : undefined,
    files > 0 ? tx('trajectory:layout.file-attachments', { count: files }) : undefined,
  ].filter(Boolean).join(' · ');
}

function compactionText(tx: Translate, record: TraceRecord): string {
  if (record.state === 'running') return tx('trajectory:layout.compacting');
  if (record.state !== 'completed') return tx('trajectory:layout.compaction-failed');
  return record.preview?.text ? trajectoryPreviewText(record.preview.text) : tx('trajectory:layout.compacted');
}

/** The plain one-line text a record row shows, as Harness's display text. */
export function recordText(tx: Translate, record: TraceRecord): string {
  if (record.kind === 'compaction') return compactionText(tx, record);
  const preview = record.preview?.text ? trajectoryPreviewText(record.preview.text) : '';
  const attachments = attachmentSummary(tx, record);
  return attachments === '' ? preview : preview === '' ? attachments : `${attachments} · ${preview}`;
}

/** The text a row lists, as Harness's row label names it: a Tool's call, a record's display text. */
export function listText(tx: Translate, item: InspectableDisplayItem): string {
  const record = item.record;
  if (item.type === 'SystemPromptCell') return item.label;
  if (item.type === 'ContextRow') return item.context.preview?.text ? trajectoryPreviewText(item.context.preview.text) : '';
  if (record.tool) return [record.tool.name ?? record.tool.tool_id, record.tool.arguments?.text].filter(Boolean).join(' ');
  const text = recordText(tx, record);
  return text === '' && record.kind === 'assistant' && record.calls.length ? tx('trajectory:record.tool-call-only') : text;
}

/** One row's content cell. */
export function CellContent({ item }: { item: InspectableDisplayItem }) {
  const tx = useTranslation();
  const record = item.record;
  if (item.type === 'SystemPromptCell') {
    // As in Harness, the row names the change; the prompt itself is inspected.
    return <span className={css.contentText} title={item.label} data-system-prompt-state={record.request?.system_prompt.state} data-tool-catalog-state={record.request?.tool_catalog}>{item.label}</span>;
  }
  if (item.type === 'ContextRow') {
    const text = item.context.preview?.text ? trajectoryPreviewText(item.context.preview.text) : '';
    return <span className={css.contentText} title={text}>{text || '—'}</span>;
  }
  if (record.tool) {
    const name = record.tool.name ?? record.tool.tool_id;
    const args = record.tool.arguments?.text;
    const result = record.preview?.text ?? record.tool.detail?.text;
    const request = [name, args].filter(Boolean).join(' ');
    return <span className={result === undefined ? css.contentText : css.resultPreview} title={result === undefined ? request : `${request} → ${result}`}>
      <span className={result === undefined ? undefined : css.resultRequest}>
        <span className={css.toolCallNameTypeface}>{name}</span>
        {args !== undefined && <span className={css.toolCallPayload}>{args}</span>}
      </span>
      {result !== undefined && <span className={isErrorRecord(record) ? `${css.inlineResult} ${css.error}` : css.inlineResult}>
        <span className={css.arrow}>→</span>
        <span className={css.inlineResultText}>{result}</span>
      </span>}
    </span>;
  }
  const text = recordText(tx, record);
  if (text === '' && record.kind === 'assistant' && record.calls.length) {
    return <span className={css.contentText}><span className={css.toolCallOnly}>{tx('trajectory:record.tool-call-only')}</span></span>;
  }
  return <span className={css.contentText} title={text}>{text || '—'}</span>;
}
