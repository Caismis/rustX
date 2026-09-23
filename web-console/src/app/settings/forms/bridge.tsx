import { useContext, useEffect, useRef, type ReactNode } from 'react';
import { useForm, type ReactFormExtendedApi } from '@tanstack/react-form';
import { useSelector } from '@xstate/react';
import type { SourceMutation } from '../../../../../protocol/app-server/v18';
import { Button } from '../../../presentation/primitives/Button';
import { SourceContext } from '../source-context';
import { useSettingsActor, useUnitTransaction } from '../machines/react';
import { admitsSourceMutation, committedUnit, unitOutcome, type MutationOutcome } from '../machines/settings-target';
import { committed as unitCommitted, discardable, requiresReview } from '../machines/unit-transaction';
import { authoredStateLabel, effectiveStateLabel, provenanceLabel, revisionSelector, unitFacts } from '../projection';
import { Advanced, ConfirmAction } from '../primitives/aria';
import css from '../../../presentation/settings/SettingsContent.module.css';

/** The typed form instance of one semantic unit's editor.
 *
 * TanStack Form is the field-mechanics layer and nothing more: nested-object
 * and array editing, typed field state, precise subscriptions and the local
 * touched/dirty metadata a form needs to present itself. It is never the
 * configuration transaction owner — see `useUnitEditing`. */
export type TypedUnitForm<T> = ReactFormExtendedApi<
  T, undefined, undefined, undefined, undefined, undefined, undefined, undefined, undefined, undefined, undefined, never>;

/** Structural equality of two authored values.
 *
 * The bridge needs to know whether the value the transaction actor currently
 * owns is still the one this form last reflected into it. Native configuration
 * documents are plain JSON values, so a structural comparison answers exactly
 * that, without depending on key order or object identity. */
function sameValue(left: unknown, right: unknown): boolean {
  if (Object.is(left, right)) return true;
  if (typeof left !== 'object' || typeof right !== 'object' || left === null || right === null) return false;
  if (Array.isArray(left) !== Array.isArray(right)) return false;
  if (Array.isArray(left) && Array.isArray(right)) {
    return left.length === right.length && left.every((item, index) => sameValue(item, right[index]));
  }
  const a = left as Record<string, unknown>, b = right as Record<string, unknown>;
  const keys = new Set([...Object.keys(a), ...Object.keys(b)]);
  for (const key of keys) if (!sameValue(a[key], b[key])) return false;
  return true;
}

/** Everything one semantic unit's editing surface needs, owned where it
 * belongs.
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
 * None of those five live in a React component or in a TanStack Form instance.
 * They live in the unit's transaction actor, which is owned by the Settings
 * authority actor and outlives this form, the detail view it sits in, the page
 * it belongs to and the whole Settings dialog.
 *
 * Rendering an inherited unit shows the native effective value while authoring
 * nothing: no draft is created, Save stays unavailable, and a no-op Save can
 * therefore never turn "no Workspace override" into an explicit empty one.
 * `blank` is only an editing seed for a unit this scope has yet to author; it
 * is never presented as an effective value and never written on its own.
 *
 * Remove is the exact inverse of authoring, not a generic mutation: it exists
 * only while this scope really authors the unit. Authored presence is the
 * native projection fact the call site passes, never a truthiness test —
 * `false`, `[]`, `{}` and `""` are authored values like any other. */
export interface UnitEditing<T> {
  readonly identity: string;
  /** The value this editor must present: the browser draft when one exists,
   * otherwise this scope's authored value, otherwise the native inherited one,
   * otherwise the neutral authoring seed. */
  readonly displayed: T;
  /** Reflect a new authored value into the transaction actor. */
  readonly edit: (value: T) => void;
  readonly submit: (remove?: boolean) => void;
  readonly discard: () => void;
  readonly review: () => void;
  readonly draft: boolean;
  readonly authoredPresent: boolean;
  readonly overriding: boolean;
  readonly inheritance: boolean;
  readonly committed: boolean;
  readonly busy: boolean;
  readonly admitted: boolean;
  readonly reviewNeeded: boolean;
  readonly intent: boolean;
  /** This unit's own native outcome. Never another unit's. */
  readonly outcome: MutationOutcome;
  readonly base: string;
  readonly observed: string;
  readonly scope: 'user' | 'workspace';
  readonly facts: ReturnType<typeof unitFacts>;
  readonly configUnit: boolean;
}

export interface UnitOptions<T> {
  /** The exact value this scope authors for this unit, or `undefined` when it
   * authors none. Call sites pass the native projection without a fallback. */
  authored?: T;
  /** The neutral seed for authoring a unit this scope does not have yet. */
  blank: T;
  revision: string;
  mutation: (value: T | null) => SourceMutation;
  /** Adapt one native effective value into this control's authored shape.
   * Returning `undefined` declares the unit non-inheritable in the editor —
   * a Provider credential is never read back from a shadowed definition. */
  inherited?: (effective: unknown) => T | undefined;
  /** The native projection of this unit carries its presence but never its
   * value, because the value is a secret-bearing literal. The form then never
   * displays or copies an existing value: authoring replaces it outright. */
  redacted?: boolean;
}

export function useUnitEditing<T>({ authored, blank, revision, mutation, inherited = value => value as T, redacted = false }: UnitOptions<T>): UnitEditing<T> {
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
  // The target reports one outcome at a time; this reads only the one that is
  // about this unit, so two independent sections never borrow each other's.
  const outcome = useSelector(actor, target => unitOutcome(target, identity), (left, right) => left.kind === right.kind && JSON.stringify(left) === JSON.stringify(right));
  const draft = transaction?.draft as { value: T } | undefined;
  const authoredPresent = authored !== undefined;
  const base = transaction?.base ?? revision;
  const observed = transaction?.observed ?? revision;
  // The native effective value, adapted to this control's authored shape. It is
  // displayed, never copied into authoring state. A redacted unit has no value
  // to adapt, so nothing can be copied out of it by construction.
  const inheritedValue = inheritance && facts.effective.state === 'available' ? inherited(facts.effective.value) : undefined;
  const displayed: T = draft ? draft.value
    : redacted ? blank
      : authored !== undefined ? authored
        : inheritedValue !== undefined ? inheritedValue : blank;
  return {
    identity, displayed, outcome, draft: draft !== undefined, authoredPresent,
    overriding: draft !== undefined || authoredPresent, inheritance, committed, busy, admitted,
    base, observed, scope, facts, configUnit: unitMutation.kind === 'config',
    // An edit is an unambiguous override transition: it starts from whatever
    // this control currently displays and becomes this browser's authored
    // intent. The value goes to the transaction actor, never to component or
    // form state that a remount could lose.
    edit: (value: T) => actor.send({ type: 'UNIT.EDIT', identity, selector, revision, value }),
    submit: (remove = false) => {
      if (!remove && !draft) return;
      // Only the mutation itself carries the authored payload; the transaction
      // is handed the token and the non-sensitive selector that settle it.
      actor.send({ type: 'UNIT.SUBMIT', identity, selector, revision, mutation: mutation(remove ? null : draft!.value) });
    },
    discard: () => actor.send({ type: 'UNIT.DISCARD', identity }),
    review: () => actor.send({ type: 'UNIT.REVIEW', identity }),
    // Whether the current source diverges from the CAS base is the
    // transaction's own fact: it alone knows whether a post-commit observation
    // has completed.
    reviewNeeded: !!snapshot && requiresReview(snapshot),
    // Discard abandons browser authoring intent only. A definitive commit, and
    // the observation and review it still owes, are never offered as a draft.
    intent: !!snapshot && discardable(snapshot),
  };
}

/** The presentation frame every semantic-unit editor shares: native authored
 * and effective facts, the redaction notice, the CAS disclosure, the conflict
 * review and the Save / Override / Remove / Discard actions.
 *
 * The frame renders the actions; it never owns them. Every one of them is a
 * message to the unit's transaction actor. */
function UnitShell<T>({ title, unit, redacted = false, removable, removalNotice, children }: {
  title: string; unit: UnitEditing<T>; redacted?: boolean; removable: boolean;
  removalNotice?: ReactNode; children: ReactNode;
}) {
  const workspace = unit.scope === 'workspace';
  const preserved = unit.draft ? 'Your draft and original revision are preserved.'
    : unit.committed ? 'Your committed revision is no longer the current source.'
      : 'Your removal and its original revision are preserved.';
  return <form aria-label={title} className={css.unit} onSubmit={event => { event.preventDefault(); unit.submit(); }}>
    <fieldset disabled={unit.busy}><legend>{title}</legend>
      {unit.configUnit && unit.facts.authored.state !== 'unavailable' && <>
        <p className={css.hint} data-authored={unit.facts.authored.state} data-effective={unit.facts.effective.state}>
          {authoredStateLabel(unit.facts.authored, unit.scope)} · {effectiveStateLabel(unit.facts.effective)} · {provenanceLabel(unit.facts.origin)}
        </p>
        {unit.facts.authored.state === 'invalid' && <p role="alert">Authored source is invalid. {unit.facts.authored.diagnostic}</p>}
        {unit.facts.effective.state === 'invalid' && <p role="alert">Native effective resolution failed. {unit.facts.effective.diagnostic}</p>}
        {unit.inheritance && <Advanced title="Native resolved value (not Session adoption)">
          <pre>{unit.facts.effective.state === 'available' ? JSON.stringify(unit.facts.effective.value, null, 2) : effectiveStateLabel(unit.facts.effective)}</pre>
        </Advanced>}
      </>}
      {redacted && <p className={css.hint}>The authored value is never projected to the browser. Saving replaces it with exactly what you enter here.</p>}
      {children}
      <Advanced title="Source revision & replacement">
        <p className={css.hint}>Draft base revision: {unit.base}<br />Current revision: {unit.observed}</p>
        <p>Save replaces this native semantic unit. Remove omits it from this scope. Empty selections remain explicit.</p>
      </Advanced>
      {unit.reviewNeeded && <div className={css.review}>
        <p role="status">Source revision changed. {preserved} Review the current source before replacing it.</p>
      </div>}
      <div className={css.actions}>
        <Button variant="primary" type="submit" disabled={!unit.draft || !unit.admitted}>Save {title}</Button>
        {/* Authoring an absent unit is always an explicit gesture, never
            something rendering it does. For a Workspace it is an override of
            the inherited value; for User it starts this scope's own authored
            value from the neutral seed. It is what makes an explicit empty
            selection authorable without an incidental edit. */}
        {unit.configUnit && !unit.overriding && <Button type="button"
          title={workspace ? 'Author this unit in this Workspace. Nothing is written until you save.' : 'Author this unit in this source. Nothing is written until you save.'}
          onClick={() => unit.edit(unit.displayed)}>{workspace ? 'Override' : 'Author'} {title}</Button>}
        {removable && unit.authoredPresent && (workspace
          // A Workspace removal is inheritance, not destruction: it removes the
          // unit this Workspace authors, through exact CAS, and the native
          // inherited value becomes effective again. The effective resource
          // survives, so this is never presented as deleting it.
          ? <span data-removal="override-removal"><ConfirmAction label={`Use global default ${title}`} disabled={!unit.admitted}
            title={`Use the global default for ${title}?`} confirm={`Use global default ${title}`}
            description={<><p>This removes the semantic unit this Workspace authors, through exact CAS. The native inherited value becomes effective again.</p><p>Nothing is removed from the global source, and no other scope is changed.</p></>}
            onConfirm={() => unit.submit(true)} /></span>
          // A User removal really removes this scope's authored unit.
          : <span data-removal="authored-removal"><ConfirmAction label={`Remove ${title}`} disabled={!unit.admitted}
            title={`Remove ${title} from User configuration?`} confirm={`Remove ${title}`}
            description={<><p>This removes the value this User source authors, through exact CAS on its current revision.</p>{removalNotice ?? <p>The native default for this unit applies once it is absent.</p>}</>}
            onConfirm={() => unit.submit(true)} /></span>)}
        {unit.intent && <Button type="button" onClick={unit.discard}>Discard draft</Button>}
        {unit.reviewNeeded && <Button type="button" onClick={unit.review}>Use reviewed revision</Button>}
      </div>
      {/* Two different facts, reported separately: the native write is
          definitively committed, and the authoritative reread that settles it
          has been observed. Neither is ever presented as the other. */}
      {unit.committed && <p role="status">Saved. Native application proceeds automatically.</p>}
      <UnitOutcomeNotice title={title} outcome={unit.outcome} />
    </fieldset>
  </form>;
}

/** One native semantic unit's own outcome, named so it can never be read as a
 * neighbouring unit's. A definition that committed and a permission that
 * conflicted are reported separately, because they are two native mutations. */
function UnitOutcomeNotice({ title, outcome }: { title: string; outcome: MutationOutcome }) {
  switch (outcome.kind) {
    // A commit whose authoritative reread is still owed is announced by the
    // line above; it is not yet an observed settlement, so it is not repeated
    // here as one.
    case 'submitting': case 'committed': return null;
    case 'saved': return <p role="status">{title} saved. Native coordination owns application.</p>;
    case 'conflict': return <p role="alert">{title} was not saved: the source changed. Your draft and base revision are preserved.</p>;
    case 'rejected': return <p role="alert">{title} was not saved. {outcome.detail}</p>;
    case 'uncertain': return <p role="alert">The outcome of saving {title} is unknown. Authority is reread; the write is never replayed. Review the current source before saving again.</p>;
    default: return null;
  }
}

/** One native semantic unit's editing surface, with a plain value contract.
 *
 * Used where the unit is a single scalar or a small closed choice, so a typed
 * form layer would add mechanics without removing any. */
export function UnitForm<T>({ title, children, removable = true, removalNotice, ...options }: UnitOptions<T> & {
  title: string; children: (value: T, change: (value: T) => void) => ReactNode;
  removable?: boolean; removalNotice?: ReactNode;
}) {
  const unit = useUnitEditing(options);
  return <UnitShell title={title} unit={unit} redacted={options.redacted} removable={removable} removalNotice={removalNotice}>
    {children(unit.displayed, unit.edit)}
  </UnitShell>;
}

/** One native semantic unit's editing surface, with a typed TanStack Form
 * field layer.
 *
 * This is the ownership bridge, and it is the reason TanStack Form can be
 * adopted without becoming a second configuration authority:
 *
 * 1. Every field change is reflected into the unit's XState transaction actor,
 *    so the durable editing intent never lives only in this component.
 * 2. The form is seeded from the actor-owned displayed value, so remounting
 *    the detail — a page change, a list/detail change, closing and reopening
 *    Settings — rehydrates from the draft the actor still owns.
 * 3. An authoritative native revision update changes `authored`, never the
 *    draft, so a dirty form is never reset by one. The form is resynchronized
 *    only when the actor's own value stops being the one this form last
 *    reflected into it — a discard, a reviewed-revision gesture, or a
 *    definitive commit dropping a confirmed draft.
 * 4. `isDirty` / `isTouched` are presentation metadata and are disposable.
 * 5. Save submits through the unit transaction path, with the exact CAS base
 *    the transaction owns; this component never calls a native operation.
 *
 * There is therefore exactly one value called the draft, and the actor owns it. */
export function TypedUnitForm<T>({ title, children, removable = true, removalNotice, ...options }: UnitOptions<T> & {
  title: string; children: (form: TypedUnitForm<T>) => ReactNode;
  removable?: boolean; removalNotice?: ReactNode;
}) {
  const unit = useUnitEditing(options);
  // The value this form last pushed into the transaction actor. Comparing the
  // actor's current value against it — rather than against the form's own
  // state — is what tells an external change apart from this form's own edit.
  const reflected = useRef<T>(unit.displayed);
  const form = useForm({
    defaultValues: unit.displayed,
    listeners: {
      onChange: ({ formApi }) => {
        const values = formApi.state.values as T;
        reflected.current = values;
        unit.edit(values);
      },
    },
  }) as TypedUnitForm<T>;
  useEffect(() => {
    if (sameValue(reflected.current, unit.displayed)) return;
    // The actor's value moved for a reason that is not this form's editing:
    // a discarded draft, a reviewed revision, or a confirmed commit dropping
    // the authored draft. The actor is the owner, so the form follows it.
    reflected.current = unit.displayed;
    form.reset(unit.displayed);
  }, [form, unit.displayed]);
  return <UnitShell title={title} unit={unit} redacted={options.redacted} removable={removable} removalNotice={removalNotice}>
    {children(form)}
  </UnitShell>;
}
