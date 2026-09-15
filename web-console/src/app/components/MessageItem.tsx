/* Copyright (c) 2026 DeepSeek. MIT. Source-derived; see PROVENANCE.md. */
// UserStyleBubble extraction from DeepSeek Harness ui-chat/MessageItem.
// No Harness message nodes, pending echoes, retry countdowns, or action slots.
import type { ReactNode } from 'react';
import css from './MessageItem.module.css';
export function MessageItem({ user, label, children }: { user: boolean; label: string; children: ReactNode }) {
  return user ? <article className={css.userRow} aria-label={label}>
    <div className={css.userStack}><div className={css.bubble}>{children}</div></div>
  </article> : <article className={css.assistantRow} aria-label={label}>
    <span className="eyebrow">{label}</span><div className={css.assistantBody}>{children}</div>
  </article>;
}
