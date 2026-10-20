/* Copyright (c) 2026 DeepSeek. MIT. Derived from pinned Harness; see PROVENANCE.md. */
import { useState, type ReactNode } from 'react';
import { IconApiOutline14, IconChevronDownOutline14, IconChevronRightOutline14 } from '../primitives/icons';
import { TextShimmer } from '../primitives/TextShimmer';
import css from './Message.module.css';
/** Harness user-stack/bubble; native adapters supply canonical content and actions. */
export function UserMessage({ label, attachments, children, actions }: { label: string; attachments?: ReactNode; children: ReactNode; actions?: ReactNode }) {
 return <article className={css.userRow} aria-label={label}><div className={css.userStack}>
 {attachments && <div className={css.attachmentRow}>{attachments}</div>}<div className={css.bubble}>{children}</div></div>{actions}</article>;
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
