import { useTranslation } from '../../locale/react';
/* Copyright (c) 2026 DeepSeek. MIT. Adapted PermissionSelect/RiskConfirmation; see PROVENANCE.md. */
import { useState } from 'react';
import type { ApprovalMode } from '../../../../protocol/app-server/v78';
import { Menu } from '../primitives/Menu';
import { Button } from '../primitives/Button';
import { DialogSurface } from '../primitives/DialogSurface';
import { IconChevronDownOutline14, IconWarningOutline16, SHIELD_OUTLINE_PATH, SHIELD_OUTLINE_STROKE } from '../primitives/icons';
import css from './PermissionSelect.module.css';
import risk from '../primitives/RiskConfirmation.module.css';
import modal from '../primitives/Modal.module.css';
/** Closed native-expressible vocabulary supplied by the Workspace adapter. */
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
      anchor={<button type="button" className={css.trigger} title={label} aria-label={tx('agent:permission-select.workspace-permissions')} aria-haspopup="menu" aria-expanded={open} disabled={disabled} onClick={() => setOpen(v => !v)}>
        <span className={css.triggerIcon} aria-hidden><PermissionIcon value={value} /></span><span className={css.triggerLabel}>{label}</span><IconChevronDownOutline14 />
      </button>} />
    <DialogSurface open={confirm} onClose={() => setConfirm(false)} title={tx('agent:permission-select.enable-full-access')} overlayClassName={`${modal.root} ${modal.scrim}`} className={`${modal.dialog} ${risk.confirmation}`}>
      <div className={`${modal.content} ${risk.confirmationContent}`}><div className={modal.header}><h2 className={modal.title}>{tx('agent:permission-select.enable-full-access')}</h2></div><div className={modal.body}>
        <div className={risk.warning}><IconWarningOutline16 size={18} className={risk.warningIcon}/><p>{tx('agent:permission-select.future-attempts-in-this-workspace-may-run-without-policy-approva')}</p></div>
        <label className={risk.acknowledgement}><input type="checkbox" checked={acknowledged} onChange={e => setAcknowledged(e.target.checked)}/><span>{tx('agent:permission-select.i-understand-and-authorize-full-access-for-this-workspace')}</span></label></div>
      </div>
      <div className={modal.footer}><Button onClick={() => setConfirm(false)}>{tx('agent:permission-select.cancel')}</Button><Button variant="primary" disabled={disabled || !acknowledged} onClick={() => { setConfirm(false); choose('full_access'); }}>{tx('agent:permission-select.enable-full-access-2')}</Button></div>
    </DialogSurface>
  </>;
}

function permissionLabel(value: ApprovalMode | undefined, tx: (key: string) => string) {
  if (value === 'full_access') return tx('agent:permission-select.full-access');
  if (value === 'automatic') return tx('agent:permission-select.automatic');
  if (value === 'plan') return tx('agent:permission-select.plan');
  return tx('agent:permission-select.permissions');
}

function PermissionIcon({ value }: { value?: ApprovalMode }) {
  return <svg width="16" height="16" viewBox="0 0 16 16" fill="none"><path d={SHIELD_OUTLINE_PATH} stroke="currentColor" strokeWidth={SHIELD_OUTLINE_STROKE} strokeLinejoin="round"/>{value === 'full_access' && <><path d="M9.10094 4.5V8.75939H7.59888V4.5H9.10094Z" fill="currentColor"/><path d="M9.10094 9.8114V11.5H7.59888V9.8114H9.10094Z" fill="currentColor"/></>}</svg>;
}
