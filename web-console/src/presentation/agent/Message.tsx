/* Copyright (c) 2026 DeepSeek. MIT. Derived from pinned Harness; see PROVENANCE.md. */
import { useState, type ReactNode } from 'react';
import { IconApiOutline14, IconChevronDownOutline14, IconChevronRightOutline14 } from '../primitives/icons';
import { TextShimmer } from '../primitives/TextShimmer';
import { fileExtension, FileTypeIcon } from '../primitives/FileTypeIcon';
import { fileSizeText } from '../primitives/file-size';
import { messageCss } from './messageCss';
/** Harness user-stack/bubble; native adapters supply canonical content and actions. */
export function UserMessage({ label, attachments, children, actions }: { label: string; attachments?: ReactNode; children: ReactNode; actions?: ReactNode }) {
 return <article className={messageCss.userRow} aria-label={label}><div className={messageCss.userStack}>
 {attachments && <div className={messageCss.attachmentRow} data-message-attachments>{attachments}</div>}{children != null && children !== false && children !== '' && <div className={messageCss.bubble} data-message-body>{children}</div>}</div>{actions}</article>;
}

/** Harness UserContent file tile; bytes are shown only when supplied by its owner. */
export function MessageFile({ name, bytes, onOpen }: { name: string; bytes?: number; onOpen?: () => void }) {
 const metadata = [fileExtension(name).toUpperCase().slice(0, 8), bytes === undefined ? '' : fileSizeText(bytes)].filter(Boolean).join(' ');
 const content = <><FileTypeIcon path={name} className={messageCss.fileIcon}/><span className={messageCss.fileContent}>
  <span className={messageCss.fileName}>{name}</span><span className={messageCss.fileMeta}>{metadata}</span>
 </span></>;
 return onOpen ? <button type="button" className={messageCss.fileCard} title={name} data-message-file onClick={onOpen}>{content}</button>
  : <span className={messageCss.fileCard} title={name} data-message-file>{content}</span>;
}
export function AssistantMessage({ label, children, stopped, actions }: { label: string; children: ReactNode; stopped?: string; actions?: ReactNode }) {
 return <article className={messageCss.assistant} aria-label={label} data-assistant-message>{children}{stopped && <span className={messageCss.stopped} data-assistant-stopped="">{stopped}</span>}{actions}</article>;
}

/** Harness compaction marker/command. Only its expanded body contains summary text. */
export function CompactionMessage({ title, summary, children, running = false, error = false }: { title: string; summary: string; children?: ReactNode; running?: boolean; error?: boolean }) {
 const [open, setOpen] = useState(false);
 const expandable = !!children, expanded = expandable && open;
 return <div className={messageCss.compactionRow} data-compaction-marker="" data-state={running ? 'running' : error ? 'error' : 'ok'}>
  <button type="button" className={messageCss.compactionButton} disabled={!expandable} aria-label={`${title} · ${summary}`} aria-expanded={expandable ? expanded : undefined} onClick={() => setOpen(value => !value)}>
   <span className={messageCss.compactionLeading} aria-hidden="true">
    <span className={messageCss.compactionContextIcon} data-compaction-icon="context"><IconApiOutline14/></span>
    <span className={messageCss.compactionDisclosureIcon} data-compaction-disclosure={expanded ? 'expanded' : 'collapsed'}>
     {expanded ? <IconChevronDownOutline14/> : <IconChevronRightOutline14/>}
    </span>
   </span>
   <span className={messageCss.compactionTitle}>{title}</span>
   <span className={messageCss.compactionSep} aria-hidden="true"/>
   <span className={messageCss.compactionSummary} data-error={error || undefined}>{running ? <TextShimmer active>{summary}</TextShimmer> : summary}</span>
  </button>
  {expanded && <div className={messageCss.compactionBody} data-compaction-body="">{children}</div>}
 </div>;
}
