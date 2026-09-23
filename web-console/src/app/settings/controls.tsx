import { useContext, type ReactNode } from 'react';
import { useSelector } from '@xstate/react';
import type { SourceMutation } from '../../../../protocol/app-server/v18';
import { Switch } from '../../presentation/primitives/Switch';
import { Button } from '../../presentation/primitives/Button';
import { SourceContext } from './source-context';
import { useSettingsActor, useUnitTransaction } from './machines/react';
import { admitsSourceMutation, committedUnit } from './machines/settings-target';
import { committed as unitCommitted, discardable, requiresReview } from './machines/unit-transaction';
import { authoredStateLabel, effectiveStateLabel, provenanceLabel, revisionSelector, unitFacts } from './projection';
import css from '../../presentation/settings/SettingsContent.module.css';

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
 * None of those five live in this component. It renders the unit's transaction
 * actor and sends user intent to the Settings authority actor; the transaction,
 * its pinned base and its settlement are owned by that actor and outlive this
 * form, the section it sits in and the whole Settings dialog.
 *
 * Rendering an inherited unit shows the native effective value while authoring
 * nothing: no draft is created, Save stays unavailable, and a no-op Save can
 * therefore never turn "no Workspace override" into an explicit empty one.
 * `blank` is only an editing seed for a unit this scope has yet to author; it
 * is never presented as an effective value and never written on its own.
 *
 * Remove is the exact inverse of authoring, not a generic mutation: it exists
 * only while this scope really authors the unit, because "remove the authored
 * unit" has no meaning for one that is already absent. Authored presence is the
 * native projection fact the call site passes, never a truthiness test — `false`,
 * `[]`, `{}` and `""` are authored values like any other. */
export function UnitForm<T>({ title, authored, blank, revision, mutation, children, removable = true, inherited = value => value as T, redacted = false }: {
  title: string;
  /** The exact value this scope authors for this unit, or `undefined` when it
   * authors none. Call sites pass the native projection without a fallback. */
  authored?: T;
  /** The neutral seed for authoring a unit this scope does not have yet. */
  blank: T;
  revision: string; mutation: (value: T | null) => SourceMutation;
  children: (value: T, change: (value: T) => void) => ReactNode; removable?: boolean;
  /** Adapt one native effective value into this control's authored shape.
   * Returning `undefined` declares the unit non-inheritable in the editor —
   * a Provider credential is never read back from a shadowed definition. */
  inherited?: (effective: unknown) => T | undefined;
  /** The native projection of this unit carries its presence but never its
   * value, because the value is a secret-bearing literal. The form then never
   * displays or copies an existing value: authoring replaces it outright. */
  redacted?: boolean;
}) {
  const actor = useSettingsActor();
  const source = useContext(SourceContext);
  const scope = source?.target.kind ?? 'user';
  const unitMutation = mutation(null);
  const facts = unitFacts(source, scope, unitMutation);
  const workspace = scope === 'workspace';
  const inheritance = workspace && unitMutation.kind === 'config';
  const identity = JSON.stringify(unitMutation);
  const selector = revisionSelector(unitMutation);
  // The unit's live transaction, owned by the Settings authority actor.
  // `undefined` is exactly "this browser authored nothing for the unit and
  // fences on native authority".
  const snapshot = useUnitTransaction(actor, identity);
  const transaction = snapshot?.context;
  // A live transaction answers for its own commit. Once a settled commit's
  // transaction has retired, the target's mutation outcome still names it.
  const lastCommit = useSelector(actor, target => committedUnit(target));
  const committed = snapshot ? unitCommitted(snapshot) : lastCommit === identity;
  // This unit's own mutation is in flight: its transaction owns the intent
  // until the outcome, so its controls are closed.
  const busy = !!snapshot?.matches({ mutation: 'submitting' });
  // Whether the target admits any new source mutation now. It is one
  // target-wide fact owned by the Settings authority actor: while another
  // unit's mutation is submitting, or a settled mutation still awaits its
  // authoritative observation, this unit stays editable but cannot submit.
  const admitted = useSelector(actor, target => admitsSourceMutation(target.context));
  const draft = transaction?.draft as { value: T } | undefined;
  // This exact scope's authored presence, from the native projection the call
  // site passes without a fallback. Only `undefined` means "authors none".
  const authoredPresent = authored !== undefined;
  const base = transaction?.base ?? revision;
  const observed = transaction?.observed ?? revision;
  // The native effective value, adapted to this control's authored shape. It is
  // displayed, never copied into authoring state. A redacted unit has no value
  // to adapt, so nothing can be copied out of it by construction.
  const inheritedValue = inheritance && facts.effective.state === 'available' ? inherited(facts.effective.value) : undefined;
  const overriding = draft !== undefined || authoredPresent;
  const displayed: T = draft ? draft.value
    : redacted ? blank
      : authored !== undefined ? authored
        : inheritedValue !== undefined ? inheritedValue : blank;
  // An edit is an unambiguous override transition: it starts from whatever this
  // control currently displays and becomes this browser's authored intent.
  const edit = (value: T) => actor.send({ type: 'UNIT.EDIT', identity, selector, revision, value });
  const submit = (remove = false) => {
    if (!remove && !draft) return;
    // Only the mutation itself carries the authored payload; the transaction is
    // handed the token and the non-sensitive selector that settle it.
    actor.send({ type: 'UNIT.SUBMIT', identity, selector, revision, mutation: mutation(remove ? null : draft!.value) });
  };
  // Whether the current source diverges from the CAS base is the transaction's
  // own fact: it alone knows whether a post-commit observation has completed.
  const reviewNeeded = !!snapshot && requiresReview(snapshot);
  // Discard abandons browser authoring intent only. A definitive commit, and
  // the observation and review it still owes, are never offered as a draft.
  const intent = !!snapshot && discardable(snapshot);
  const preserved = draft ? 'Your draft and original revision are preserved.'
    : committed ? 'Your committed revision is no longer the current source.'
      : 'Your removal and its original revision are preserved.';
  return <form aria-label={title} className={css.unit} onSubmit={e => { e.preventDefault(); submit(); }}>
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
      {redacted && <p className={css.hint}>The authored value is never projected to the browser. Saving replaces it with exactly what you enter here.</p>}
      {children(displayed, edit)}
      <details><summary>Source revision & replacement</summary><p className={css.hint}>Draft base revision: {base}<br />Current revision: {observed}</p><p>Save replaces this native semantic unit. Remove omits it from this scope. Empty selections remain explicit.</p></details>
      {reviewNeeded && <div className={css.review}><p role="status">Source revision changed. {preserved} Review the current source before replacing it.</p><details><summary>Review current authored unit (redacted)</summary><pre>{redacted ? 'Authored value not projected' : JSON.stringify(authored, null, 2)}</pre></details></div>}
      <div className={css.actions}><Button variant="primary" type="submit" disabled={!draft || !admitted}>Save {title}</Button>
        {inheritance && !overriding && <Button type="button" title="Author this unit in this Workspace. Nothing is written until you save." onClick={() => edit(displayed)}>Override {title}</Button>}
        {removable && authoredPresent && <Button type="button" title={workspace ? 'Remove the unit this Workspace authors, through exact CAS. The native inherited value becomes effective.' : 'Remove the authored value through exact CAS'} disabled={!admitted} onClick={() => submit(true)}>{workspace ? 'Use global default' : 'Remove'} {title}</Button>}
        {intent && <Button type="button" onClick={() => actor.send({ type: 'UNIT.DISCARD', identity })}>Discard draft</Button>}
        {reviewNeeded && <Button type="button" onClick={() => actor.send({ type: 'UNIT.REVIEW', identity })}>Use reviewed revision</Button>}
      </div>{committed && <p role="status">Saved. Native application proceeds automatically.</p>}
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
