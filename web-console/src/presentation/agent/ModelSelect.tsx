import { useTranslation } from '../../locale/react';
/* Copyright (c) 2026 DeepSeek. MIT. Derived from ui-model-selection/ModelSelect; see PROVENANCE.md. */
import { useState } from 'react';
import { Menu, type MenuEntry } from '../primitives/Menu';
import { IconDataOutline16, IconChevronDownOutline14 } from '../primitives/icons';
import css from './ModelSelect.module.css';
export interface ModelChoice { id: string; profiles: { id: string; label: string }[]; defaultProfile?: string }
/** One selection gesture, never inferred from what an invocation resolves to:
 * choose a Model (keeping its pinned Profile when it is already selected), pin
 * a named Profile, or follow the Model's default Profile. */
export type ModelSelectionIntent =
 | { kind: 'model'; model: string }
 | { kind: 'profile'; model: string; profile: string }
 | { kind: 'model-default'; model: string };
type Translate = ReturnType<typeof useTranslation>;
/** A configured Profile the Model no longer declares. */
export function profileUnavailable(choice: ModelChoice, profile?: string) {
 return profile !== undefined && !choice.profiles.some(item => item.id === profile);
}
/** Whether a Model offers the explicit `model-default` action: always when it
 * declares Profiles, and for the configured Model while its pinned Profile is
 * unavailable. Choosing the selected Model keeps a pinned Profile, so on a
 * Model without Profiles this is the only way to clear a stale pin. */
export function offersModelDefault(choice: ModelChoice, current?: string, profile?: string) {
 return choice.profiles.length > 0 || choice.id === current && profileUnavailable(choice, profile);
}
/** A Model without Profiles has no default Profile to name. */
export function modelDefaultLabel(tx: Translate, choice: ModelChoice) {
 return !choice.profiles.length ? tx('agent:model-select.use-model-defaults')
  : choice.defaultProfile ? tx('agent:model-select.model-default-profile-value', { p0: choice.defaultProfile }) : tx('agent:model-select.model-default-profile');
}
/** The Harness model/profile two-level menu over exact adapter-supplied choices.
 * `profile` is the configured (pinned) Profile; `effectiveProfile` is what the
 * current invocation resolves to and is only displayed. */
export function ModelSelect({ choices, current, profile, effectiveProfile, disabled, loading, error, load, choose, initialOpen = false, binding = '' }: {
 choices: ModelChoice[]; current?: string; profile?: string; effectiveProfile?: string; disabled: boolean; loading: boolean; error?: string;
 load: () => void; choose: (intent: ModelSelectionIntent) => void; initialOpen?: boolean; binding?: string;
}) {
  const tx = useTranslation();
 const [open, setOpen] = useState(initialOpen);
 const [owner, setOwner] = useState(binding);
 if (owner !== binding) { setOwner(binding); setOpen(false); }
 const selected = choices.find(choice => choice.id === current);
 const displayed = effectiveProfile ?? profile ?? selected?.defaultProfile;
 const items: MenuEntry[] = [{ id: 'models', disabled: disabled || loading, label: tx('agent:model-select.model'), submenu: choices.map(choice => ({ id: `model:${choice.id}`, label: choice.id, disabled: disabled || loading })) }];
 if (selected && offersModelDefault(selected, current, profile)) items.push({ id: 'profiles', disabled: disabled || loading, label: tx('agent:model-select.profile'), submenu: [
  { id: 'profile-default', label: modelDefaultLabel(tx, selected), disabled: disabled || loading },
  ...selected.profiles.map(choice => ({ id: `profile:${choice.id}`, label: choice.label, disabled: disabled || loading })),
 ] });
 // Catalog reads must not resize the Composer (and its history viewport).
 // Like Harness, keep read status and selection diagnostics in the portal.
 const status: MenuEntry[] = [];
 if (loading) status.push({ id: 'loading', disabled: true, label: <span role="status">{tx('agent:model-select.reading-native-models')}</span> });
 if (current && !loading && !selected) status.push({ id: 'unavailable-model', disabled: true, label: <span role="status">{current} {tx('agent:model-select.is-unavailable-in-this-workspace')}</span> });
 if (!loading && selected && profileUnavailable(selected, profile)) status.push({ id: 'unavailable-profile', disabled: true, label: <span role="status">{tx('agent:model-select.profile')}{' '}{profile} {tx('agent:model-select.is-unavailable-for')}{' '}{current} {tx('agent:model-select.in-this-workspace')}</span> });
 return <div className={css.root}>
 <Menu open={open} side="top" align="end" autoFocus={!loading && !disabled} items={items} footer={status}
 selectedIds={[`model:${current}`, profile === undefined ? 'profile-default' : `profile:${profile}`]} onClose={() => setOpen(false)}
 onSelect={id => {
  if (disabled || loading) return; setOpen(false);
  if (id.startsWith('model:')) choose({ kind: 'model', model: id.slice(6) });
  else if (current && id === 'profile-default') choose({ kind: 'model-default', model: current });
  else if (current && id.startsWith('profile:')) choose({ kind: 'profile', model: current, profile: id.slice(8) });
 }}
 anchor={<button data-model-select="" className={css.trigger} type="button" aria-label={tx('agent:model-select.model-and-profile')} aria-haspopup="menu" aria-expanded={open} disabled={disabled} onClick={() => { setOpen(v => !v); if (!open) load(); }}><IconDataOutline16 size={16}/><span className={css.triggerLabel}>{current ?? tx('agent:model-select.choose-model')}</span>{displayed && <span className={css.triggerEffort}>{displayed}</span>}<IconChevronDownOutline14/></button>}/>
 {error && <p role="alert">{error}</p>}
 </div>;
}
