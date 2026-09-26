import { useTranslation } from '../../locale/react';
/* Copyright (c) 2026 DeepSeek. MIT. Source-derived; see PROVENANCE.md. */
// DeepSeek Harness ApprovalFlow presentation. Pending.answer/answered state and
// Remote-waterfall ownership removed; authoritative rustX binding supplies status.
import type { ReactNode } from 'react';
import { Button } from '../primitives/Button';
import css from './Approval.module.css';
export function ApprovalTakeover({ title, detail, status, disabled, onAllow, onDeny }: {
  title: string; detail: ReactNode; status: string; disabled: boolean;
  onAllow: () => void; onDeny: () => void;
}) {
  const tx = useTranslation();
  return <section className={css.root} aria-label={tx('interactions:approval-takeover.approval')}>
    <div className={css.card}>
      <div className={css.strip}><span className={css.dot} />{status}</div>
      <div className={css.body} data-approval-scroll tabIndex={0} role="group" aria-label={tx('interactions:approval-takeover.approval-details')}>
        <div className={css.headline}>{title}</div><div className={css.command}>{detail}</div>
      </div>
      <div className={css.actionRow}>
        <Button variant="outline" className={css.reject} disabled={disabled} onClick={onDeny}>{tx('interactions:approval-takeover.deny')}</Button>
        <Button variant="primary" disabled={disabled} onClick={onAllow}>{tx('interactions:approval-takeover.allow-once')}</Button>
      </div>
    </div>
  </section>;
}
