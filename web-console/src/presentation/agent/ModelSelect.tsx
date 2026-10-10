import { useTranslation } from '../../locale/react';
/* Copyright (c) 2026 DeepSeek. MIT. Derived from ui-model-selection/ModelSelect; see PROVENANCE.md. */
import { useState } from 'react';
import { Menu, type MenuEntry } from '../primitives/Menu';
import { IconDataOutline16, IconChevronDownOutline14 } from '../primitives/icons';
import css from './ModelSelect.module.css';
export interface ModelChoice { id: string; profiles: { id: string; label: string }[]; defaultProfile?: string }
/** The Harness model/profile two-level menu over exact adapter-supplied choices. */
export function ModelSelect({ choices, current, profile, disabled, loading, error, load, choose, initialOpen = false, binding = '' }: {
 choices: ModelChoice[]; current?: string; profile?: string; disabled: boolean; loading: boolean; error?: string;
 load: () => void; choose: (model: string, profile?: string) => void; initialOpen?: boolean; binding?: string;
}) {
  const tx = useTranslation();
 const [open, setOpen] = useState(initialOpen);
 const [owner, setOwner] = useState(binding);
 if (owner !== binding) { setOwner(binding); setOpen(false); }
 const selected = choices.find(choice => choice.id === current);
 const effectiveProfile = profile ?? selected?.defaultProfile;
 const items: MenuEntry[] = [{ id: 'models', disabled: disabled || loading, label: tx('agent:model-select.model'), submenu: choices.map(choice => ({ id: `model:${choice.id}`, label: choice.id, disabled: disabled || loading })) }];
 if (selected?.profiles.length) items.push({ id: 'profiles', disabled: disabled || loading, label: tx('agent:model-select.reasoning-profile'), submenu: selected.profiles.map(choice => ({ id: `profile:${choice.id}`, label: choice.label, disabled: disabled || loading })) });
 // Catalog reads must not resize the Composer (and its history viewport).
 // Like Harness, keep read status and selection diagnostics in the portal.
 const status: MenuEntry[] = [];
 if (loading && !error) status.push({ id: 'loading', disabled: true, label: <span role="status">{tx('agent:model-select.reading-native-models')}</span> });
 if (error) status.push({ id: 'error', disabled: true, label: <span role="alert">{error}</span> }, { id: 'retry', disabled, label: tx('agent:agent-controls.reread-models') });
 if (current && !loading && !selected) status.push({ id: 'unavailable-model', disabled: true, label: <span role="status">{current} {tx('agent:model-select.is-unavailable-in-this-workspace')}</span> });
 if (!loading && selected && profile && !selected.profiles.some(choice => choice.id === profile)) status.push({ id: 'unavailable-profile', disabled: true, label: <span role="status">{tx('agent:model-select.reasoning-profile')}{' '}{profile} {tx('agent:model-select.is-unavailable-for')}{' '}{current} {tx('agent:model-select.in-this-workspace')}</span> });
 return <div className={css.root}>
 <Menu open={open} side="top" align="end" autoFocus={!loading && !disabled} items={items} footer={status}
 selectedIds={[`model:${current}`, `profile:${effectiveProfile}`]} onClose={() => setOpen(false)}
 onSelect={id => { if (disabled) return; if (id === 'retry') { load(); return; } if (loading) return; setOpen(false); if (id.startsWith('model:')) choose(id.slice(6)); else if (current && id.startsWith('profile:')) choose(current, id.slice(8)); }}
 anchor={<button data-model-select="" className={css.trigger} type="button" aria-label={tx('agent:model-select.model-and-reasoning')} title={[current ?? tx('agent:model-select.choose-model'), effectiveProfile].filter(Boolean).join(' · ')} aria-haspopup="menu" aria-expanded={open} disabled={disabled} onClick={() => { setOpen(v => !v); if (!open) load(); }}><IconDataOutline16 size={16}/><span className={css.triggerLabel}>{current ?? tx('agent:model-select.choose-model')}</span>{effectiveProfile && <span className={css.triggerEffort}>{effectiveProfile}</span>}<IconChevronDownOutline14/></button>}/>
 </div>;
}
