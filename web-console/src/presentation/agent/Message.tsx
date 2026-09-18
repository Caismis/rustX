/* Copyright (c) 2026 DeepSeek. MIT. Derived from pinned Harness; see PROVENANCE.md. */
import type { ReactNode } from 'react';
import css from './Message.module.css';
/** Harness user-stack/bubble; native adapters supply canonical content and actions. */
export function UserMessage({ label, attachments, children, actions }: { label: string; attachments?: ReactNode; children: ReactNode; actions?: ReactNode }) {
 return <article className={css.userRow} aria-label={label}><div className={css.userStack}>
 {attachments && <div className={css.attachmentRow}>{attachments}</div>}<div className={css.bubble}>{children}</div></div>{actions}</article>;
}
export function AssistantMessage({ label, children }: { label: string; children: ReactNode }) {
 return <article aria-label={label} data-assistant-message>{children}</article>;
}
