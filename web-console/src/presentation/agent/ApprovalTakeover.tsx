import { useTranslation } from '../../locale/react';
/* Copyright (c) 2026 DeepSeek. MIT. Source-derived; see PROVENANCE.md. */
// DeepSeek Harness ApprovalFlow presentation. Pending.answer/answered state and
// Remote-waterfall ownership removed; authoritative rustX binding supplies status.
import type { ReactNode } from 'react';
import { Button } from '../primitives/Button';
export function ApprovalTakeover({ title, description, detail, status, disabled, onAllow, onDeny }: {
 title: string; description?: string; detail: ReactNode; status: string; disabled: boolean;
 onAllow: () => void; onDeny: () => void;
}) {
 const tx = useTranslation();
 return <section className="flex flex-col items-center px-[calc(var(--dsh-composer-side-clearance)+16px)] pt-[8px] pb-[12px]" aria-label={tx('interactions:approval-takeover.approval')}>
 <div className="overflow-hidden w-full max-w-[var(--dsh-chat-content-width)] [border:1px_solid_var(--dsw-alias-state-warn-secondary)] rounded-[var(--dsw-radius-xl)] bg-[var(--dsw-specific-input-major)] shadow-[var(--dsw-shadow-lv2)] [--dsh-scrollbar-thumb:var(--dsw-alias-scrollbar-bg-l2)] [--dsh-scrollbar-thumb-hover:var(--dsw-alias-scrollbar-hover-l2)]">
 <div className="flex items-center gap-[8px] px-[16px] py-[10px] bg-[var(--dsw-alias-state-warn-tertiary)] [color:var(--dsw-alias-state-warn-primary)] text-[13px] leading-[18px]"><span className="w-[8px] h-[8px] rounded-full [corner-shape:round] bg-[var(--dsw-alias-state-warn-primary)]" />{status}</div>
 <div className="flex flex-col gap-[6px] box-border max-h-[var(--dsh-composer-text-max-height)] overflow-y-auto pt-[12px] px-[16px] pb-0" data-approval-scroll tabIndex={0} role="group" aria-label={tx('interactions:approval-takeover.approval-details')}>
 <div className="[color:var(--dsw-alias-label-primary)] text-[15px] font-medium leading-[24px]">{title}</div>
 {description && <p className="m-0 [color:var(--dsw-alias-label-secondary)] text-[13px] leading-[20px]">{description}</p>}
 <div className="[color:var(--dsw-alias-label-tertiary)] [font-family:var(--ds-font-family-code),monospace] text-[13px] leading-[20px] break-all [&>pre]:m-0 [&>pre]:[font-family:inherit] [&>pre]:[font-size:inherit] [&>pre]:[font-weight:inherit] [&>pre]:[line-height:inherit] [&>pre]:max-h-none [&>pre]:overflow-visible">{detail}</div>
 </div>
 <div className="flex justify-end gap-[8px] px-[16px] py-[14px]">
 <Button variant="outline" className="hover:not-disabled:bg-[var(--dsw-alias-interactive-bg-hover-danger)]! hover:not-disabled:[color:var(--dsw-alias-state-error-primary)]! hover:not-disabled:border-transparent!" disabled={disabled} onClick={onDeny}>{tx('interactions:approval-takeover.deny')}</Button>
 <Button variant="primary" disabled={disabled} onClick={onAllow}>{tx('interactions:approval-takeover.allow-once')}</Button>
 </div>
 </div>
 </section>;
}
