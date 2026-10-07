/* Copyright (c) 2026 DeepSeek. MIT. Adapted from MessageItem.tsx TurnErrorItem; see PROVENANCE.md. */
import { StateDot } from '../primitives/StateDot';
import css from './TurnError.module.css';

/** Persistent failure at the terminal turn's transcript position. */
export function TurnError({ title, message, code }: { title: string; message: string; code?: string }) {
  return <div className={css.turnErrorRow} role="status" data-turn-error>
    <StateDot state="error" className={css.turnErrorDot}/>
    <div className={css.turnErrorCopy}><span className={css.turnErrorTitle}>{title}</span><span className={css.turnErrorMessage}>{message}</span></div>
    {code && <code className={css.turnErrorCode}>{code}</code>}
  </div>;
}
