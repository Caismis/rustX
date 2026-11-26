/* Copyright (c) 2026 DeepSeek. MIT. Adapted from MessageItem.tsx TurnErrorItem; see PROVENANCE.md. */
import { StateDot } from '../primitives/StateDot';

/** Persistent failure at the terminal turn's transcript position. */
export function TurnError({ title, message, code }: { title: string; message: string; code?: string }) {
  return <div className="grid grid-cols-[10px_minmax(0,1fr)_auto] gap-[8px] items-start px-0 py-[2px] [font-size:var(--dsh-content-font-size-secondary,13px)] leading-[calc(20px+var(--dsh-content-font-delta-secondary,0px))]" role="status" data-turn-error>
    <StateDot state="error" className="mt-[5px]"/>
    <div className="min-w-0 wrap-anywhere"><span className="mr-[6px] [color:var(--dsw-alias-state-error-primary)] font-semibold">{title}</span><span className="[color:var(--dsw-alias-label-secondary)]">{message}</span></div>
    {code && <code className="[color:var(--dsw-alias-label-tertiary)] [font:var(--dsw-font-markdown-code-block-small)] max-w-[12em] wrap-anywhere">{code}</code>}
  </div>;
}
