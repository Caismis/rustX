/* Copyright (c) 2026 DeepSeek. MIT. Derived from pinned Harness; see PROVENANCE.md. */
import { useState, type ReactNode } from 'react';
import { IconApiOutline14, IconChevronDownOutline14, IconChevronRightOutline14 } from '../primitives/icons';
import { TextShimmer } from '../primitives/TextShimmer';
import { fileExtension, FileTypeIcon } from '../primitives/FileTypeIcon';
import { fileSizeText } from '../primitives/file-size';
import css from './Message.module.css';
/** Harness user-stack/bubble; native adapters supply canonical content and actions. */
export function UserMessage({ label, attachments, children, actions }: { label: string; attachments?: ReactNode; children: ReactNode; actions?: ReactNode }) {
 return <article className={css.userRow} aria-label={label}><div className={css.userStack}>
 {attachments && <div className={css.attachmentRow} data-message-attachments>{attachments}</div>}{children != null && children !== false && children !== '' && <div className={css.bubble} data-message-body>{children}</div>}</div>{actions}</article>;
}

/** Harness UserContent file tile; bytes are shown only when supplied by its owner. */
export function MessageFile({ name, bytes, onOpen }: { name: string; bytes?: number; onOpen?: () => void }) {
 const metadata = [fileExtension(name).toUpperCase().slice(0, 8), bytes === undefined ? '' : fileSizeText(bytes)].filter(Boolean).join(' ');
 const content = <><FileTypeIcon path={name} className={css.fileIcon}/><span className={css.fileContent}>
  <span className={css.fileName}>{name}</span><span className={css.fileMeta}>{metadata}</span>
 </span></>;
 return onOpen ? <button type="button" className={css.fileCard} title={name} data-message-file onClick={onOpen}>{content}</button>
  : <span className={css.fileCard} title={name} data-message-file>{content}</span>;
}
export function AssistantMessage({ label, children, stopped, actions }: { label: string; children: ReactNode; stopped?: string; actions?: ReactNode }) {
 return <article aria-label={label} data-assistant-message>{children}{stopped && <span className={css.stopped} data-assistant-stopped="">{stopped}</span>}{actions}</article>;
}

/** Harness compaction marker/command. Only its expanded body contains summary text. */
export function CompactionMessage({ title, summary, children, running = false, error = false }: { title: string; summary: string; children?: ReactNode; running?: boolean; error?: boolean }) {
 const [open, setOpen] = useState(false);
 const expandable = !!children, expanded = expandable && open;
 return <div className={css.compactionRow} data-compaction-marker="" data-state={running ? 'running' : error ? 'error' : 'ok'}>
  <button type="button" className={css.compactionButton} disabled={!expandable} aria-label={`${title} · ${summary}`} aria-expanded={expandable ? expanded : undefined} onClick={() => setOpen(value => !value)}>
   <span className={css.compactionLeading} aria-hidden="true">
    <span className={css.compactionContextIcon} data-compaction-icon="context"><IconApiOutline14/></span>
    <span className={css.compactionDisclosureIcon} data-compaction-disclosure={expanded ? 'expanded' : 'collapsed'}>
     {expanded ? <IconChevronDownOutline14/> : <IconChevronRightOutline14/>}
    </span>
   </span>
   <span className={css.compactionTitle}>{title}</span>
   <span className={css.compactionSep} aria-hidden="true"/>
   <span className={css.compactionSummary} data-error={error || undefined}>{running ? <TextShimmer active>{summary}</TextShimmer> : summary}</span>
  </button>
  {expanded && <div className={css.compactionBody} data-compaction-body="">{children}</div>}
 </div>;
}
