import { Fragment, useState, type ReactNode } from 'react';
import { useTranslation } from '../../locale/react';
/* Copyright (c) 2026 DeepSeek. MIT. Source-derived; see PROVENANCE.md. */
// Adapted from DeepSeek Harness ui-deliverables PresentedFileCard and the
// presented-files grid of Deliverables: a whole-card Preview gesture with
// filename/description hierarchy, a file-type glyph, and separate actions that
// never nest inside the card button. Harness's Host open/reveal phases, desktop
// metadata and file URLs are excluded; rustX supplies Preview and Download
// intents from its existing PreviewWorkspace owners.
import { FileTypeIcon, fileExtension } from '../primitives/FileTypeIcon';
import { IconChevronDownOutline14, IconChevronUpOutline14 } from '../primitives/icons';
import css from './Deliverables.module.css';

const COLLAPSED_PRESENTED_COUNT = 4;

function cardDescription(description: string | null | undefined, fallback: string): string {
  const trimmed = description?.replace(/\s*(?:\([^()]*\)|（[^（）]*）)\s*$/u, '').trim();
  return trimmed === undefined || trimmed === '' ? fallback : trimmed;
}

/** One committed delivery as the card shows it. */
export interface PresentedFileView {
  readonly key: string;
  readonly name: string;
  readonly path: string;
  readonly description?: string | null | undefined;
}

export function PresentedFileCard({ file, onPreview, actions }: { file: PresentedFileView; onPreview?: () => void; actions: ReactNode }) {
  const tx = useTranslation();
  const metadata = fileExtension(file.name).toUpperCase() || tx('artifacts:presented.file');
  return <div className={css.file} data-presented-file data-delivery-card role="group" aria-label={file.name}>
    <button type="button" className={css.cardPreview} title={file.path}
      aria-label={tx('artifacts:presented.preview-card', { name: file.name })} disabled={!onPreview} onClick={onPreview}/>
    <span className={css.fileIcon}><FileTypeIcon path={file.name} size={20}/></span>
    <div className={css.fileBody}>
      <div className={css.details}>
        <span className={css.fileName} data-presented-name>{file.name}</span>
        <span className={css.description} data-presented-description>
          <span className={css.secondaryText}>{cardDescription(file.description, metadata)}</span>
          {onPreview && <span className={css.previewHint}>{tx('artifacts:presented.preview')}</span>}
        </span>
      </div>
      <div className={css.actions}>{actions}</div>
    </div>
  </div>;
}

/** The ordered committed files of one Tool result: one row, or a 2x2 summary. */
export function PresentedFiles({ files, card }: { files: readonly PresentedFileView[]; card: (file: PresentedFileView) => ReactNode }) {
  const tx = useTranslation();
  const [expanded, setExpanded] = useState(false);
  if (!files.length) return null;
  const collapsible = files.length > COLLAPSED_PRESENTED_COUNT;
  const shown = collapsible && !expanded ? files.slice(0, COLLAPSED_PRESENTED_COUNT) : files;
  return <div className={css.root}>
    <div className={css.presented} data-presented-files-row data-single={files.length === 1 || undefined}>
      {shown.map(file => <Fragment key={file.key}>{card(file)}</Fragment>)}
    </div>
    {collapsible && <button type="button" className={css.toggle} aria-expanded={expanded}
      aria-label={tx(expanded ? 'artifacts:presented.collapse-aria' : 'artifacts:presented.expand-aria', { count: files.length })}
      onClick={() => setExpanded(value => !value)}>
      <span>{tx(expanded ? 'artifacts:presented.collapse' : 'artifacts:presented.all', { count: files.length })}</span>
      {expanded ? <IconChevronUpOutline14/> : <IconChevronDownOutline14/>}
    </button>}
  </div>;
}
