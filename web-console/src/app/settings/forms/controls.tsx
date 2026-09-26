import { useTranslation } from '../../../locale/react';
import { Button } from '../../../presentation/primitives/Button';
import { Choice } from '../primitives/aria';
import css from '../../../presentation/settings/SettingsContent.module.css';

/** Plain value-contract controls, for semantic units that are one scalar or
 * one small closed choice. A typed form layer would add field mechanics to
 * these without removing any, so they stay ordinary controlled inputs. */

export function TextField({ label, value, change, required = false, secret = false }: {
  label: string; value?: string | null; change: (value: string) => void; required?: boolean; secret?: boolean;
}) {
  return <label>{label}<input type={secret ? 'password' : 'text'} value={value ?? ''} required={required}
    onChange={event => change(event.target.value)} autoComplete={secret ? 'new-password' : undefined} /></label>;
}

export function Names({ label, value, change }: { label: string; value: string[]; change: (value: string[]) => void }) {
  const tx = useTranslation();
  return <fieldset><legend>{label}</legend>
    {value.map((name, index) => <div className={css.names} key={index}>
      <input aria-label={tx('settings:extension-detail.value-value', { p0: label, p1: index + 1 })} value={name}
        onChange={event => change(value.map((item, at) => at === index ? event.target.value : item))} />
      <Button aria-label={tx('settings:controls.remove-value-value', { p0: label, p1: index + 1 })} onClick={() => change(value.filter((_, at) => at !== index))}>{tx('settings:controls.remove')}</Button>
    </div>)}
    <Button onClick={() => change([...value, ''])}>{tx('settings:extensions-page.add')}{' '}{label}</Button>
    {!value.length && <p className={css.hint}>{tx('settings:controls.empty-list-no-entries')}</p>}
  </fieldset>;
}

/** The native `all` / exact-identities / explicit-none selection.
 *
 * The three are distinct native values and are never collapsed: `all` is not
 * "every identity currently known", an explicit empty list is an authored
 * decision to select none, and an absent unit is neither. */
export function Selection({ label, value, change }: { label: string; value: 'all' | string[]; change: (value: 'all' | string[]) => void }) {
  const tx = useTranslation();
  const mode = value === 'all' ? 'all' : value.length ? 'exact' : 'none';
  return <fieldset><legend>{label}</legend>
    <Choice label={tx('settings:controls.selection')} value={mode} options={[['none', tx('settings:copy.none')], ['all', tx('settings:copy.all')], ['exact', tx('settings:copy.exact-identities')]]}
      onChange={next => change(next === 'all' ? 'all' : next === 'exact' ? [''] : [])} />
    {Array.isArray(value) && value.length > 0 && <Names label={tx('settings:controls.value-identities', { p0: label })} value={value} change={change} />}
  </fieldset>;
}

export function CheckboxList({ label, values, selected, change }: {
  label: string; values: readonly string[]; selected: string[]; change: (next: string[]) => void;
}) {
  return <fieldset><legend>{label}</legend>
    {[...new Set([...values, ...selected])].map(name => <label key={name}>
      <input type="checkbox" checked={selected.includes(name)}
        onChange={event => change(event.target.checked ? [...selected, name] : selected.filter(id => id !== name))} />{name}
    </label>)}
  </fieldset>;
}

/** The native Tool identities a policy may be authored for, and the complete
 * set of built-in Tool identities the root Agent may be granted. */
export const policyTools = ['read', 'write', 'edit', 'glob', 'grep', 'bash'] as const;
export const nativeTools = [...policyTools, 'ask_user', 'execution'];
