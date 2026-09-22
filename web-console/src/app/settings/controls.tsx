import { useContext, useEffect, useRef, useState, type ReactNode } from 'react';
import type { SourceMutation } from '../../../../protocol/app-server/v18';
import { Switch } from '../../presentation/primitives/Switch';
import { Button } from '../../presentation/primitives/Button';
import { EditorStateContext, SourceContext } from './drafts';
import { authoredStateLabel, effectiveStateLabel, provenanceLabel, unitFacts } from './projection';
import css from '../../presentation/settings/SettingsContent.module.css';

export type SaveSource = (mutation: SourceMutation, revision: string) => Promise<string | undefined>;

/** One native semantic unit's editing surface.
 *
 * Five facts stay distinct and are never collapsed into one form value:
 *
 * - the native **effective** value, projected from `SourceSettings.resolved`;
 * - this scope's native **authored** presence/value (`authored`);
 * - this browser's **override intent**, which exists only after an explicit
 *   Override action or an actual edit;
 * - the **dirty draft** carrying that intent's value;
 * - the exact **CAS base revision** the next write is fenced on.
 *
 * Rendering an inherited unit shows the native effective value while authoring
 * nothing: no draft is created, Save stays unavailable, and a no-op Save can
 * therefore never turn "no Workspace override" into an explicit empty one.
 * `blank` is only an editing seed for a unit this scope has yet to author; it
 * is never presented as an effective value and never written on its own. */
export function UnitForm<T>({ title, authored, blank, revision, mutation, save, children, removable = true, inherited = value => value as T }: {
  title: string;
  /** The exact value this scope authors for this unit, or `undefined` when it
   * authors none. Call sites pass the native projection without a fallback. */
  authored?: T;
  /** The neutral seed for authoring a unit this scope does not have yet. */
  blank: T;
  revision: string; mutation: (value: T | null) => SourceMutation;
  save: SaveSource; children: (value: T, change: (value: T) => void) => ReactNode; removable?: boolean;
  /** Adapt one native effective value into this control's authored shape.
   * Returning `undefined` declares the unit non-inheritable in the editor —
   * a Provider credential is never read back from a shadowed definition. */
  inherited?: (effective: unknown) => T | undefined;
}) {
  const edits = useContext(EditorStateContext);
  const source = useContext(SourceContext);
  const scope = source?.target.kind ?? 'user';
  const unitMutation = mutation(null);
  const facts = unitFacts(source, scope, unitMutation);
  const workspace = scope === 'workspace';
  const inheritance = workspace && unitMutation.kind === 'config';
  const identity = JSON.stringify(unitMutation);
  const cached = edits?.get(identity);
  // Override intent and its dirty draft. `undefined` means this browser has
  // authored nothing for this unit: rendering, opening and navigating never
  // create it, and only an explicit Override or a real edit does.
  const [draft, setDraft] = useState<{ value: T } | undefined>(cached?.draft ? { value: cached.draft.value as T } : undefined);
  const [base, setBase] = useState(cached?.base ?? revision);
  // A submitted Save or Remove pins the reviewed base revision even for an
  // otherwise clean form. A rejected removal must never adopt the reread
  // revision implicitly — including across a remount.
  const [pinned, setPinned] = useState(cached?.pinned ?? false);
  const [busy, setBusy] = useState(false), [saved, setSaved] = useState(false);
  const committed = useRef<string | undefined>(cached?.committed);
  // The pre-save revision this form's last acknowledged save was based on. An
  // acknowledgement advances `base` before the authoritative projection catches
  // up; while the projection still carries exactly that pre-save revision the
  // source is merely unobserved, not changed — no review prompt. Any other
  // revision means the source really moved and keeps the explicit review.
  const savedFrom = useRef<string | undefined>(cached?.savedFrom);
  // Mirror the whole transaction into the durable per-section store, not only
  // when a value draft exists. A pinned base with no draft is a real editing
  // transaction and must survive remounts; a clean, unpinned form owns nothing.
  useEffect(() => {
    if (!edits) return;
    if (draft || pinned || committed.current !== undefined) edits.set(identity, { draft: draft ? { value: draft.value } : undefined, base, pinned, committed: committed.current, savedFrom: savedFrom.current });
    else edits.delete(identity);
  }, [edits, identity, draft, base, pinned, saved]);
  useEffect(() => {
    // Consume this form's own acknowledgement once the authoritative projection
    // carries the committed revision; the redacted projection then owns the
    // displayed value again, so no literal credential survives in a draft.
    if (committed.current === revision) {
      committed.current = undefined; savedFrom.current = undefined;
      setDraft(undefined); setPinned(false); setBase(revision); edits?.delete(identity);
    } else if (!draft && !pinned) setBase(revision);
    // Source publication may precede the save promise. Consume its
    // acknowledgement even when the projection dependencies already settled.
  }, [revision, draft, pinned, saved, edits, identity]);
  // The native effective value, adapted to this control's authored shape. It is
  // displayed, never copied into authoring state.
  const inheritedValue = inheritance && facts.effective.state === 'available' ? inherited(facts.effective.value) : undefined;
  const overriding = draft !== undefined || authored !== undefined;
  const displayed: T = draft ? draft.value : authored !== undefined ? authored : inheritedValue !== undefined ? inheritedValue : blank;
  // An edit is an unambiguous override transition: it starts from whatever this
  // control currently displays and becomes this browser's authored intent.
  const edit = (next: T) => { committed.current = undefined; setDraft({ value: next }); setPinned(true); setSaved(false); };
  const commit = async (remove = false) => {
    if (!remove && !draft) return;
    const from = base;
    setPinned(true); setBusy(true); setSaved(false);
    try { const next = await save(mutation(remove ? null : draft!.value), base); if (next) { committed.current = next; savedFrom.current = from; setBase(next); setSaved(true); setDraft(undefined); } }
    finally { setBusy(false); }
  };
  const reviewNeeded = base !== revision && revision !== savedFrom.current;
  return <form aria-label={title} className={css.unit} onSubmit={e => { e.preventDefault(); void commit(); }}>
    <fieldset disabled={busy}><legend>{title}</legend>
      {source && unitMutation.kind === 'config' && <>
        <p className={css.hint} data-authored={facts.authored.state} data-effective={facts.effective.state}>
          {authoredStateLabel(facts.authored, scope)} · {effectiveStateLabel(facts.effective)} · {provenanceLabel(facts.origin)}
        </p>
        {facts.authored.state === 'invalid' && <p role="alert">Authored source is invalid. {facts.authored.diagnostic}</p>}
        {facts.effective.state === 'invalid' && <p role="alert">Native effective resolution failed. {facts.effective.diagnostic}</p>}
        {inheritance && <details><summary>Native resolved value (not Session adoption)</summary>
          <pre>{facts.effective.state === 'available' ? JSON.stringify(facts.effective.value, null, 2) : effectiveStateLabel(facts.effective)}</pre></details>}
      </>}
      {children(displayed, edit)}
      <details><summary>Source revision & replacement</summary><p className={css.hint}>Draft base revision: {base}<br />Current revision: {revision}</p><p>Save replaces this native semantic unit. Remove omits it from this scope. Empty selections remain explicit.</p></details>
      {reviewNeeded && <div className={css.review}><p role="status">Source revision changed. Your draft and original revision are preserved. Review the current source before replacing it.</p><details><summary>Review current authored unit (redacted)</summary><pre>{JSON.stringify(authored, null, 2)}</pre></details></div>}
      <div className={css.actions}><Button variant="primary" type="submit" disabled={!draft}>Save {title}</Button>
        {inheritance && !overriding && <Button type="button" title="Author this unit in this Workspace. Nothing is written until you save." onClick={() => edit(displayed)}>Override {title}</Button>}
        {removable && <Button type="button" title={workspace ? 'Use global default — remove this Workspace override' : 'Remove authored value'} onClick={() => void commit(true)}>Remove {title}</Button>}
        <Button type="button" onClick={() => { committed.current = undefined; savedFrom.current = undefined; setDraft(undefined); setPinned(false); setBase(revision); setSaved(false); }}>Discard draft</Button>
        {reviewNeeded && <Button type="button" onClick={() => { setPinned(true); setBase(revision); }}>Use reviewed revision</Button>}
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
