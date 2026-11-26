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
  return <div className="group relative box-border flex items-center gap-[10px] min-w-0 h-[60px] px-[10px] py-[8px] overflow-hidden [border:0.5px_solid_var(--dsw-alias-border-l1)] rounded-[16px] bg-[var(--deliverable-fill)] [color:var(--dsw-alias-label-primary)] transition-colors duration-[120ms] hover:bg-[var(--deliverable-hover)]" data-presented-file data-delivery-card role="group" aria-label={file.name}>
    <button type="button" className="absolute z-[1] inset-0 w-full p-0 border-0 rounded-[inherit] bg-transparent cursor-pointer focus-visible:outline-none focus-visible:[box-shadow:inset_0_0_0_2px_var(--dsw-focus-ring-color,var(--dsw-alias-state-business-primary))] disabled:cursor-default" title={file.path}
      aria-label={tx('artifacts:presented.preview-card', { name: file.name })} disabled={!onPreview} onClick={onPreview}/>
    <span className="relative z-[2] box-border flex-none grid place-items-center w-[40px] h-[40px] overflow-hidden pointer-events-none [border:0.5px_solid_var(--dsw-alias-border-l1)] rounded-[10px] bg-[color-mix(in_srgb,var(--dsw-static-neutral-00)_50%,transparent)] dark:bg-[color-mix(in_srgb,var(--dsw-static-neutral-00)_5%,transparent)] [color:var(--dsw-alias-link)]"><FileTypeIcon path={file.name} size={20}/></span>
    <div className="relative z-[2] flex flex-1 items-center justify-between gap-[12px] min-w-0 pointer-events-none">
      <div className="flex flex-1 flex-col justify-center gap-[2px] min-w-0">
        <span className="overflow-hidden text-ellipsis whitespace-nowrap text-[13px] font-medium leading-[20px]" data-presented-name>{file.name}</span>
        <span className="overflow-hidden [color:var(--dsw-alias-label-tertiary)] text-[10px] font-normal leading-[16px] text-ellipsis whitespace-nowrap" data-presented-description>
          <span className="group-hover:hidden">{cardDescription(file.description, metadata)}</span>
          {onPreview && <span className="hidden group-hover:inline">{tx('artifacts:presented.preview')}</span>}
        </span>
      </div>
      <div className="inline-flex flex-none pointer-events-auto">{actions}</div>
    </div>
  </div>;
}

/** One Turn's ordered delivery selection: one row, or a 2x2 summary. */
export function PresentedFiles({ files, card }: { files: readonly PresentedFileView[]; card: (file: PresentedFileView) => ReactNode }) {
  const tx = useTranslation();
  const [expanded, setExpanded] = useState(false);
  if (!files.length) return null;
  const collapsible = files.length > COLLAPSED_PRESENTED_COUNT;
  const shown = collapsible && !expanded ? files.slice(0, COLLAPSED_PRESENTED_COUNT) : files;
  return <div className="[--deliverable-fill:var(--dsw-static-neutral-50)] [--deliverable-hover:var(--dsw-static-neutral-100)] dark:[--deliverable-fill:var(--dsw-static-neutral-850)] dark:[--deliverable-hover:var(--dsw-static-neutral-800)] @container flex flex-col gap-[16px] min-w-0 mt-[4px]">
    <div className="grid grid-cols-2 @max-[620px]:grid-cols-1 gap-[10px] min-w-0 [&[data-single=true]]:grid-cols-1" data-presented-files-row data-single={files.length === 1 || undefined}>
      {shown.map(file => <Fragment key={file.key}>{card(file)}</Fragment>)}
    </div>
    {collapsible && <button type="button" className="self-center inline-flex items-center gap-[4px] min-w-0 px-[11px] py-[1px] border-0 rounded-[8px] bg-transparent [color:var(--dsw-alias-label-tertiary)] cursor-pointer [font:inherit] text-[12px] leading-[18px] hover:bg-[var(--dsw-alias-interactive-bg-hover)] [&_svg]:flex-none [&_svg]:w-[14px] [&_svg]:h-[14px]" aria-expanded={expanded}
      aria-label={tx(expanded ? 'artifacts:presented.collapse-aria' : 'artifacts:presented.expand-aria', { count: files.length })}
      onClick={() => setExpanded(value => !value)}>
      <span>{tx(expanded ? 'artifacts:presented.collapse' : 'artifacts:presented.all', { count: files.length })}</span>
      {expanded ? <IconChevronUpOutline14/> : <IconChevronDownOutline14/>}
    </button>}
  </div>;
}
