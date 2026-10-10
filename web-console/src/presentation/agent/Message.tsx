/* Copyright (c) 2026 DeepSeek. MIT. Derived from pinned Harness; see PROVENANCE.md. */
import { useState, type ReactNode } from 'react';
import { IconApiOutline14, IconChevronDownOutline14, IconChevronRightOutline14 } from '../primitives/icons';
import css from './Message.module.css';
/** Harness user-stack/bubble; native adapters supply canonical content and actions. */
export function UserMessage({ label, attachments, children, actions }: { label: string; attachments?: ReactNode; children: ReactNode; actions?: ReactNode }) {
 return <article className={css.userRow} aria-label={label}><div className={css.userStack}>
 {attachments && <div className={css.attachmentRow}>{attachments}</div>}<div className={css.bubble}>{children}</div></div>{actions}</article>;
}
export function AssistantMessage({ label, children }: { label: string; children: ReactNode }) {
 return <article aria-label={label} data-assistant-message>{children}</article>;
}

/** Harness's in-transcript checkpoint disclosure; content comes from the native Ledger. */
export function CompactionMessage({ title, summary, preview, description, children }: { title: string; summary: string; preview: string; description: string; children: ReactNode }) {
 const [open, setOpen] = useState(false);
 return <div className={css.compactionRow} data-compaction-marker="">
  <button type="button" className={css.compactionButton} aria-label={`${title} · ${summary}`} aria-expanded={open} onClick={() => setOpen(value => !value)}>
   <span className={css.compactionLeading} aria-hidden="true">
    <span className={css.compactionContextIcon} data-compaction-icon="context"><IconApiOutline14/></span>
    <span className={css.compactionDisclosureIcon} data-compaction-disclosure={open ? 'expanded' : 'collapsed'}>
     {open ? <IconChevronDownOutline14/> : <IconChevronRightOutline14/>}
    </span>
   </span>
   <span className={css.compactionTitle}>{title}</span>
   <span className={css.compactionSep} aria-hidden="true"/>
   <span className={css.compactionSummary}>{preview}</span>
  </button>
  {open && <div className={css.compactionBody} data-compaction-body=""><p className={css.compactionDescription}>{description}</p>{children}</div>}
 </div>;
}
