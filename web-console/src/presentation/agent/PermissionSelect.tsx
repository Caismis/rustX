import { useTranslation } from '../../locale/react';
/* Copyright (c) 2026 DeepSeek. MIT. Adapted PermissionSelect/RiskConfirmation; see PROVENANCE.md. */
import { useState } from 'react';
import type { Translate } from '../../locale/translation';
import { Menu } from '../primitives/Menu';
import { Button } from '../primitives/Button';
import { DialogSurface } from '../primitives/DialogSurface';
import { IconChevronDownOutline14, IconWarningOutline16, SHIELD_OUTLINE_PATH, SHIELD_OUTLINE_STROKE } from '../primitives/icons';
import { modalCss } from '../primitives/modalCss';
import { Checkbox } from '@/components/ui/checkbox';
/** Closed native-expressible vocabulary supplied by the Workspace adapter. */
type ApprovalMode = 'plan' | 'automatic' | 'full_access';
export function PermissionSelect({ value, disabled, choose }: { value?: ApprovalMode; disabled: boolean; choose: (value: ApprovalMode) => void }) {
  const tx = useTranslation();
  const [open, setOpen] = useState(false), [confirm, setConfirm] = useState(false), [acknowledged, setAcknowledged] = useState(false);
  const label = permissionLabel(value, tx);
  return <>
    <Menu open={open && !disabled} side="top" autoFocus selectedId={value} onClose={() => setOpen(false)}
      items={[
        { id: 'plan', label: tx('agent:permission-select.plan'), icon: <PermissionIcon value="plan" /> },
        { id: 'automatic', label: tx('agent:permission-select.automatic'), icon: <PermissionIcon value="automatic" /> },
        { id: 'full_access', label: tx('agent:permission-select.full-access'), icon: <PermissionIcon value="full_access" /> },
      ]}
      onSelect={id => {
        setOpen(false);
        if (disabled || id === value) return;
        if (id === 'full_access') { setAcknowledged(false); setConfirm(true); return; }
        if (id === 'plan' || id === 'automatic') choose(id);
      }}
      anchor={<button type="button" className="inline-flex items-center gap-[4px] min-w-0 max-w-[220px] h-[28px] pt-0 pr-[4px] pb-0 pl-[8px] border-none rounded-[var(--dsw-radius-sm)] outline-none bg-transparent [color:var(--dsw-alias-label-secondary)] text-[13px] leading-[20px] font-medium cursor-pointer hover:not-disabled:bg-[var(--dsw-alias-interactive-bg-hover)] focus-visible:shadow-[0_0_0_2px_var(--dsw-focus-ring-color,var(--dsw-alias-state-business-primary))] disabled:[color:var(--dsw-alias-label-dimmed)] disabled:cursor-default" title={label} aria-label={tx('agent:permission-select.workspace-permissions')} aria-haspopup="menu" aria-expanded={open} disabled={disabled} onClick={() => setOpen(v => !v)}>
        <span className="inline-flex flex-none [&_svg]:w-[14px] [&_svg]:h-[14px]" aria-hidden><PermissionIcon value={value} /></span><span className="min-w-0 overflow-hidden text-ellipsis whitespace-nowrap @max-[460px]:hidden">{label}</span><IconChevronDownOutline14 />
      </button>} />
    <DialogSurface open={confirm} onClose={() => setConfirm(false)} title={tx('agent:permission-select.enable-full-access')} overlayClassName={`${modalCss.root} ${modalCss.scrim}`} className={`${modalCss.dialog} w-[min(440px,100%)] max-h-[calc(100vh-48px)] supports-[height:100dvh]:max-h-[calc(100dvh-48px)] overflow-hidden`}>
      <div className={`${modalCss.content} min-h-0 overflow-y-auto overscroll-contain`}><div className={modalCss.header}><h2 className={modalCss.titleCls}>{tx('agent:permission-select.enable-full-access')}</h2></div><div className={modalCss.body}>
        <div className="flex items-start gap-[10px] [color:var(--dsw-alias-label-secondary)] text-[14px] leading-[22px] [&_p]:m-0"><IconWarningOutline16 size={18} className="flex-none mt-[2px] [color:var(--dsw-alias-state-error-primary)]"/><p>{tx('agent:permission-select.future-attempts-in-this-workspace-may-run-without-policy-approva')}</p></div>
        <label className="flex items-start gap-[10px] mt-[20px] [color:var(--dsw-alias-label-primary)] text-[14px] leading-[22px] cursor-pointer"><Checkbox checked={acknowledged} onCheckedChange={setAcknowledged}/><span>{tx('agent:permission-select.i-understand-and-authorize-full-access-for-this-workspace')}</span></label></div>
      </div>
      <div className={modalCss.footer}><Button onClick={() => setConfirm(false)}>{tx('agent:permission-select.cancel')}</Button><Button variant="primary" disabled={disabled || !acknowledged} onClick={() => { setConfirm(false); choose('full_access'); }}>{tx('agent:permission-select.enable-full-access-2')}</Button></div>
    </DialogSurface>
  </>;
}

function permissionLabel(value: ApprovalMode | undefined, tx: Translate) {
  if (value === 'full_access') return tx('agent:permission-select.full-access');
  if (value === 'automatic') return tx('agent:permission-select.automatic');
  if (value === 'plan') return tx('agent:permission-select.plan');
  return tx('agent:permission-select.permissions');
}

function PermissionIcon({ value }: { value?: ApprovalMode }) {
  return <svg width="16" height="16" viewBox="0 0 16 16" fill="none"><path d={SHIELD_OUTLINE_PATH} stroke="currentColor" strokeWidth={SHIELD_OUTLINE_STROKE} strokeLinejoin="round"/>{value === 'full_access' && <><path d="M9.10094 4.5V8.75939H7.59888V4.5H9.10094Z" fill="currentColor"/><path d="M9.10094 9.8114V11.5H7.59888V9.8114H9.10094Z" fill="currentColor"/></>}</svg>;
}
