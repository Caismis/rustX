import { useContext, useEffect, useRef, useState, type ReactNode } from 'react';
import type { SourceMutation } from '../../../../protocol/app-server/v17';
import { Switch } from '../../presentation/primitives/Switch';
import { Button } from '../../presentation/primitives/Button';
import { DraftContext, SourceContext, authoredUnit } from './drafts';
import css from '../../presentation/settings/SettingsContent.module.css';

export type SaveSource = (mutation: SourceMutation, revision: string) => Promise<string | undefined>;
export function UnitForm<T>({ title, initial, revision, mutation, save, children, removable = true }: {
  title: string; initial: T; revision: string; mutation: (value: T | null) => SourceMutation;
  save: SaveSource; children: (value: T, change: (value: T) => void) => ReactNode; removable?: boolean;
}) {
  const drafts = useContext(DraftContext);
  const source = useContext(SourceContext);
  const workspace = source?.target.kind === 'workspace';
  const own = authoredUnit(source?.workspace?.authored, mutation(null));
  const identity = JSON.stringify(mutation(null));
  const cached = drafts?.get(identity);
  const [value, change] = useState<T>(() => cached ? cached.value as T : initial), [base, setBase] = useState(cached?.base ?? revision);
  const [busy, setBusy] = useState(false), [saved, setSaved] = useState(false), [dirty, setDirty] = useState(cached?.dirty ?? false);
  const committed = useRef<string | undefined>(cached?.committed);
  // The pre-save revision this form's last acknowledged save was based on. An
  // acknowledgement advances `base` before the authoritative projection catches
  // up; while the projection still carries exactly that pre-save revision the
  // source is merely unobserved, not changed — no review prompt. Any other
  // revision means the source really moved and keeps the explicit review.
  const savedFrom = useRef<string | undefined>(undefined);
  useEffect(() => { if (dirty) drafts?.set(identity, { value, base, dirty, committed: committed.current }); else drafts?.delete(identity); }, [drafts, identity, value, base, dirty]);
  useEffect(() => {
    if (committed.current === revision || !dirty) {
      committed.current = undefined; savedFrom.current = undefined;
      // Reconstruct from the native redacted projection after acknowledgement.
      // Literal credentials must not remain in a successful editor draft.
      change(initial); setBase(revision); setDirty(false);
      drafts?.delete(identity);
    }
    // Source publication may precede the save promise. Consume its
    // acknowledgement even when the projection dependencies already settled.
  }, [initial, revision, dirty, saved, drafts, identity]);
  const commit = async (remove = false) => {
    // Save and Remove both freeze the revision, including an otherwise clean form.
    // A rejected removal must never adopt the reread revision implicitly.
    const from = base;
    setDirty(true); setBusy(true); setSaved(false);
    try { const next = await save(mutation(remove ? null : value), base); if (next) { drafts?.delete(identity); committed.current = next; savedFrom.current = from; setBase(next); setSaved(true); } }
    finally { setBusy(false); }
  };
  const reviewNeeded = base !== revision && revision !== savedFrom.current;
  return <form aria-label={title} className={css.unit} onSubmit={e => { e.preventDefault(); void commit(); }}>
    <fieldset disabled={busy}><legend>{title}</legend>
      {workspace && mutation(null).kind === 'config' && <><p>{own == null ? 'Inherited — no Workspace override' : 'Workspace override — empty selections remain explicit'}</p><details><summary>Native resolved preview (not Session adoption)</summary><pre>{JSON.stringify(authoredUnit(source?.resolved, mutation(null)), null, 2) ?? 'Unset'}</pre></details></>}
      {children(value, next => { committed.current = undefined; change(next); setDirty(true); setSaved(false); })}
      <details><summary>Source revision & replacement</summary><p className={css.hint}>Draft base revision: {base}<br />Current revision: {revision}</p><p>Save replaces this native semantic unit. Remove omits it from this scope. Empty selections remain explicit.</p></details>
      {reviewNeeded && <div className={css.review}><p role="status">Source revision changed. Your draft and original revision are preserved. Review the current source before replacing it.</p><details><summary>Review current authored unit (redacted)</summary><pre>{JSON.stringify(initial, null, 2)}</pre></details></div>}
      <div className={css.actions}><Button variant="primary" type="submit">Save {title}</Button>
        {removable && <Button type="button" title={workspace ? 'Reset to global default — remove this override' : 'Remove authored value'} onClick={() => void commit(true)}>Remove {title}</Button>}
        <Button type="button" onClick={() => { change(initial); setBase(revision); setDirty(false); setSaved(false); }}>Discard draft</Button>
        {reviewNeeded && <Button type="button" onClick={() => setBase(revision)}>Use reviewed revision</Button>}
      </div>{saved && <p role="status">Saved. Native application proceeds automatically.</p>}
    </fieldset>
  </form>;
}
export function TextField({ label, value, change, required = false, secret = false }: { label: string; value?: string | null; change: (value: string) => void; required?: boolean; secret?: boolean }) {
  return <label>{label}<input type={secret ? 'password' : 'text'} value={value ?? ''} required={required} onChange={e => change(e.target.value)} autoComplete={secret ? 'new-password' : undefined} /></label>;
}
export function Names({ label, value, change }: { label: string; value: string[]; change: (value: string[]) => void }) {
  return <fieldset><legend>{label}</legend>{value.map((name, index) => <div className={css.names} key={index}><input aria-label={`${label} ${index + 1}`} value={name} onChange={e => change(value.map((item, at) => at === index ? e.target.value : item))} /><Button aria-label={`Remove ${label} ${index + 1}`} onClick={() => change(value.filter((_, at) => at !== index))}>Remove</Button></div>)}<Button onClick={() => change([...value, ''])}>Add {label}</Button>{!value.length && <p className={css.hint}>Empty list · no entries</p>}</fieldset>;
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

export function Toggle({ label, checked, change }: { label: string; checked: boolean; change: (value: boolean) => void }) {
  return <div className={css.rowHead}><span>{label}</span><Switch label={label} checked={checked} onChange={change} /></div>;
}
export function OptionalBoolean({ label, value, change }: { label: string; value?: boolean; change: (value: boolean | undefined) => void }) {
  return <label>{label}<select value={value === undefined ? '' : String(value)} onChange={e => change(e.target.value === '' ? undefined : e.target.value === 'true')}><option value="">Native default</option><option value="true">On</option><option value="false">Off</option></select></label>;
}
