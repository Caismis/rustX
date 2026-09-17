import { useEffect, useRef, useState, type ReactNode } from 'react';
import type { SourceMutation } from '../../../../protocol/app-server/v6';
import { Button } from '../../presentation/primitives/Button';
import css from './Settings.module.css';

export type SaveSource = (mutation: SourceMutation, revision: string) => Promise<string | undefined>;
export function UnitForm<T>({ title, initial, revision, mutation, save, children, removable = true }: {
  title: string; initial: T; revision: string; mutation: (value: T | null) => SourceMutation;
  save: SaveSource; children: (value: T, change: (value: T) => void) => ReactNode; removable?: boolean;
}) {
  const [value, change] = useState(initial), [base, setBase] = useState(revision);
  const [busy, setBusy] = useState(false), [saved, setSaved] = useState(false);
  const committed = useRef<string | undefined>(undefined);
  useEffect(() => {
    if (committed.current === revision) {
      committed.current = undefined;
      // Reconstruct from the native redacted projection after acknowledgement.
      // Literal credentials must not remain in a successful editor draft.
      change(initial);
    }
  }, [initial, revision]);
  const commit = async (remove = false) => {
    setBusy(true); setSaved(false);
    try { const next = await save(mutation(remove ? null : value), base); if (next) { committed.current = next; setBase(next); setSaved(true); } }
    finally { setBusy(false); }
  };
  return <form className={css.unit} onSubmit={e => { e.preventDefault(); void commit(); }}>
    <fieldset disabled={busy}><legend>{title}</legend>{children(value, next => { change(next); setSaved(false); })}
      {base !== revision && <p role="status">Source revision changed. Your draft and original revision are preserved. Review the current source before replacing it.</p>}
      <div className={css.actions}><Button variant="primary" type="submit">Save {title}</Button>
        {removable && <Button type="button" onClick={() => void commit(true)}>Remove {title}</Button>}
        <Button type="button" onClick={() => { change(initial); setBase(revision); setSaved(false); }}>Discard draft</Button>
        {base !== revision && <Button type="button" onClick={() => setBase(revision)}>Use reviewed revision</Button>}
      </div>{saved && <p role="status">Source saved. Reload separately to publish configuration.</p>}
    </fieldset>
  </form>;
}
export function TextField({ label, value, change, required = false, secret = false }: { label: string; value?: string | null; change: (value: string) => void; required?: boolean; secret?: boolean }) {
  return <label>{label}<input type={secret ? 'password' : 'text'} value={value ?? ''} required={required} onChange={e => change(e.target.value)} autoComplete={secret ? 'new-password' : undefined} /></label>;
}
export function Names({ label, value, change }: { label: string; value: string[]; change: (value: string[]) => void }) {
  return <label>{label}<textarea aria-label={label} value={value.join('\n')} onChange={e => change(e.target.value ? e.target.value.split('\n') : [])} /><span className={css.hint}>One exact identity per line. An empty list selects none.</span></label>;
}
export function Selection({ label, value, change }: { label: string; value: 'all' | string[]; change: (value: 'all' | string[]) => void }) {
  const mode = value === 'all' ? 'all' : value.length ? 'exact' : 'none';
  return <fieldset><legend>{label}</legend><label>Selection<select value={mode} onChange={e => change(e.target.value === 'all' ? 'all' : e.target.value === 'exact' ? [''] : [])}><option value="none">None</option><option value="all">All</option><option value="exact">Exact identities</option></select></label>{Array.isArray(value) && value.length > 0 && <Names label={`${label} identities`} value={value} change={change} />}</fieldset>;
}
export function CheckboxList({ label, values, selected, change }: { label: string; values: readonly string[]; selected: string[]; change: (next: string[]) => void }) {
  return <fieldset><legend>{label}</legend>{[...new Set([...values, ...selected])].map(name => <label key={name}><input type="checkbox" checked={selected.includes(name)} onChange={e => change(e.target.checked ? [...selected, name] : selected.filter(id => id !== name))} />{name}</label>)}</fieldset>;
}
export const policyTools = ['read', 'write', 'edit', 'glob', 'grep', 'bash'] as const;
export const nativeTools = [...policyTools, 'ask_user', 'execution'];
