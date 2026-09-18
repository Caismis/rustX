/* Copyright (c) 2026 DeepSeek. MIT. Derived from ui-permission-presets/PermissionSelect; see PROVENANCE.md. */
import { useState } from 'react';
import { Menu } from '../primitives/Menu';
import { IconChevronDownOutline14 } from '../primitives/icons';
import css from './PermissionSelect.module.css';
export function PermissionSelect({ choices, desired, disabled, loading, load, choose, title }: {
 title?: string; choices: { id: string; label: string }[]; desired?: string; disabled: boolean; loading: boolean; load: () => void; choose: (id: string) => void;
}) {
 const [open, setOpen] = useState(false);
 return <Menu open={open} portal side="top" autoFocus items={choices.map(choice => ({ ...choice, disabled: disabled || loading }))} selectedId={desired}
 onClose={() => setOpen(false)} onSelect={id => { setOpen(false); choose(id); }} anchor={<button type="button" className={css.trigger} aria-label="Approval mode" title={title} aria-haspopup="menu" aria-expanded={open} disabled={disabled} onClick={() => { setOpen(v => !v); if (!open) load(); }}>{choices.find(choice => choice.id === desired)?.label ?? 'Approval mode'}<IconChevronDownOutline14/></button>}/>;
}
