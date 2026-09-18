/* Copyright (c) 2026 DeepSeek. MIT. Derived from ui-model-selection/ModelSelect; see PROVENANCE.md. */
import { useState } from 'react';
import { Menu, type MenuEntry } from '../primitives/Menu';
import { IconDataOutline16, IconChevronDownOutline14 } from '../primitives/icons';
import css from './ModelSelect.module.css';
export interface ModelChoice { id: string; profiles: { id: string; label: string }[]; defaultProfile?: string }
/** The Harness model/profile two-level menu over exact adapter-supplied choices. */
export function ModelSelect({ choices, current, profile, disabled, loading, error, load, choose, initialOpen = false }: {
 choices: ModelChoice[]; current?: string; profile?: string; disabled: boolean; loading: boolean; error?: string;
 load: () => void; choose: (model: string, profile?: string) => void; initialOpen?: boolean;
}) {
 const [open, setOpen] = useState(initialOpen);
 const selected = choices.find(choice => choice.id === current);
 const effectiveProfile = profile ?? selected?.defaultProfile;
 const items: MenuEntry[] = [{ id: 'models', label: 'Model', submenu: choices.map(choice => ({ id: `model:${choice.id}`, label: choice.id, disabled: disabled || loading })) }];
 if (selected?.profiles.length) items.push({ id: 'profiles', label: 'Reasoning profile', submenu: selected.profiles.map(choice => ({ id: `profile:${choice.id}`, label: choice.label, disabled: disabled || loading })) });
 return <div className={css.root}>
 <Menu open={open} portal side="top" align="end" autoFocus items={items}
 selectedIds={[`model:${current}`, `profile:${effectiveProfile}`]} onClose={() => setOpen(false)}
 onSelect={id => { if (disabled || loading) return; if (id.startsWith('model:')) choose(id.slice(6)); else if (current && id.startsWith('profile:')) choose(current, id.slice(8)); }}
 anchor={<button className={css.trigger} type="button" aria-label="Model and reasoning" aria-haspopup="menu" aria-expanded={open} disabled={disabled} onClick={() => { setOpen(v => !v); if (!open) load(); }}><IconDataOutline16 size={16}/><span className={css.triggerLabel}>{current ?? 'Choose model'}</span>{effectiveProfile && <span className={css.triggerEffort}>{effectiveProfile}</span>}<IconChevronDownOutline14/></button>}/>
 {open && loading && <span role="status">Reading native models…</span>}
 {error && <p role="alert">{error}</p>}
 </div>;
}
