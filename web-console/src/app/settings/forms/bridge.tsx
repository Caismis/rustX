import { useContext, useEffect, useRef, type ReactNode } from 'react';
import { useForm, type ReactFormExtendedApi } from '@tanstack/react-form';
import { useSelector } from '@xstate/react';
import type { SourceMutation } from '../../../../../protocol/app-server/v25';
import { Button } from '../../../presentation/primitives/Button';
import { SourceContext } from '../source-context';
import { useSettingsActor, useUnitTransaction } from '../machines/react';
import { admitsSourceMutation, committedUnit, unitOutcome, type MutationOutcome } from '../machines/settings-target';
import { awaitingCommitObservation, committed as unitCommitted, discardable, requiresReview } from '../machines/unit-transaction';
import {
  authoredStateLabel, effectiveStateLabel, provenanceLabel, revisionSelector, shadowedDefinition, unitFacts, unitOwnership,
  type ShadowedDefinition,
} from '../projection';
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

/** Where one whole-identity resource definition's authoring stands in this
 * scope. Each state has its own wording and its own actions; none of them is
 * reached by rendering alone.
 *
 * - `new` — this scope authors nothing and inherits nothing: creating it was
 *   the explicit gesture that opened this editor;
 * - `inherited` — a lower scope's definition is in effect and is only
 *   inspected. No draft exists and the fields are not writable;
 * - `overriding` — an explicit Override began this scope's own definition from
 *   the inherited one. It is a draft; nothing is written until Save;
 * - `authored` — this scope authors the definition. */
export type DefinitionAuthoring = 'new' | 'inherited' | 'overriding' | 'authored';

/** Everything one semantic unit's editing surface needs, owned where it
 * belongs.
 *
 * These facts stay distinct and are never collapsed into one form value:
 *
 * - the native **effective** value, projected from `SourceSettings.resolved`;
 * - this scope's native **authored presence** (`authoredPresent`) — whether its
 *   source holds the unit at all;
 * - this scope's native **authored value** (`authored`) — what native parsed
 *   that unit into. A whole-identity resource lives in its own file, so a file
 *   that exists but does not parse is present with no value: it is still this
 *   scope's definition, replaced or removed on its own revision;
 * - this browser's **override intent**, which exists only after an explicit
 *   Override action or, for a value-inherited semantic unit, an actual edit —
 *   an inherited whole resource definition admits no edit before its explicit
 *   override (`DefinitionAuthoring`);
 * - the **dirty draft** carrying that intent's value;
 * - the exact **CAS base revision** the next write is fenced on.
 *
 * None of those live in a React component or in a TanStack Form instance.
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
 * `false`, `[]`, `{}` and `""` are authored values like any other — and never
 * inferred from a parse: an unparsed definition is present. */
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
  /** This scope's source holds the unit, parsed or not. */
  readonly authoredPresent: boolean;
  /** This scope's source holds the unit but native parsed no value from it. */
  readonly unparsed: boolean;
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
  /** The lifecycle of a whole-identity resource definition; `undefined` for a
   * value-inherited semantic unit. */
  readonly definition?: DefinitionAuthoring;
  /** The lower-scope definition this scope's definition shadows or would
   * shadow, if any. */
  readonly shadowed?: ShadowedDefinition;
  /** This unit's definitive commit still awaits the authoritative observation
   * that settles it. The presented source predates the commit, so it is no
   * base for a new draft: no field, Author or Override gesture edits the unit
   * until the observation arrives. */
  readonly awaitingObservation: boolean;
  /** Whether the fields may change this unit's draft now. */
  readonly writable: boolean;
  /** The one explicit transition from inspecting an inherited definition to
   * authoring this scope's own. Present only while `definition` is
   * `inherited`. */
  readonly override?: () => void;
}

export interface UnitOptions<T> {
  /** The exact value this scope authors for this unit, or `undefined` when it
   * authors none or native parsed none. Call sites pass the native projection
   * without a fallback. */
  authored?: T;
  /** Whether this scope's source holds the unit, independent of whether native
   * parsed it into `authored`. It defaults to `authored !== undefined`, which is
   * exact wherever presence and value come from one parsed document — every
   * `rustx.toml` unit and every MCP identity of a parsed `mcp.toml`. A named
   * Agent is its own file, so its call site passes the file's presence. */
  authoredPresent?: boolean;
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

export function useUnitEditing<T>({ authored, authoredPresent = authored !== undefined, blank, revision, mutation, inherited = value => value as T, redacted = false }: UnitOptions<T>): UnitEditing<T> {
  const actor = useSettingsActor();
  const source = useContext(SourceContext);
  const scope = source?.target.kind ?? 'user';
  const unitMutation = mutation(null);
  const facts = unitFacts(source, scope, unitMutation);
  const workspace = scope === 'workspace';
  const ownership = unitOwnership(unitMutation);
  const inheritance = workspace && ownership === 'value';
  // The lower-scope definition a whole-identity resource would shadow. It is
  // native authored fact of another document, projected without secrets.
  const shadowed = ownership === 'identity' ? shadowedDefinition(source, scope, unitMutation) : undefined;
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
  // This unit's own commit is definitive but not yet observed. The transaction
  // refuses every edit until it is; the controls say so rather than offering
  // the pre-commit projection as an editing base.
  const awaitingObservation = !!snapshot && awaitingCommitObservation(snapshot);
  // Whether the target admits any new source mutation now. It is one
  // target-wide fact owned by the Settings authority actor: while another
  // unit's mutation is submitting, or a settled mutation still awaits its
  // authoritative observation, this unit stays editable but cannot submit.
  const admitted = useSelector(actor, target => admitsSourceMutation(target.context));
  // The target reports one outcome at a time; this reads only the one that is
  // about this unit, so two independent sections never borrow each other's.
  const outcome = useSelector(actor, target => unitOutcome(target, identity), (left, right) => left.kind === right.kind && JSON.stringify(left) === JSON.stringify(right));
  const draft = transaction?.draft as { value: T } | undefined;
  const unparsed = authoredPresent && authored === undefined;
  const base = transaction?.base ?? revision;
  const observed = transaction?.observed ?? revision;
  // The native effective value, adapted to this control's authored shape. It is
  // displayed, never copied into authoring state. A redacted unit has no value
  // to adapt, so nothing can be copied out of it by construction.
  const inheritedValue = inheritance && facts.effective.state === 'available' ? inherited(facts.effective.value) : undefined;
  const definition: DefinitionAuthoring | undefined = ownership !== 'identity' ? undefined
    : authoredPresent ? 'authored'
      : !shadowed ? 'new'
        : draft ? 'overriding' : 'inherited';
  // The seed an explicit override begins from. It is displayed while the
  // inherited definition is inspected and is never authoring state until the
  // override transition copies it into a draft.
  const overrideSeed = (shadowed?.seed ?? blank) as T;
  // An unparsed definition of this scope is `authored`: it is displayed from
  // the neutral seed, never from the definition it shadows, and replacing it is
  // an ordinary edit fenced on its own revision.
  const displayed: T = draft ? draft.value
    : redacted ? blank
      : authored !== undefined ? authored
        : inheritedValue !== undefined ? inheritedValue
          : definition === 'inherited' ? overrideSeed : blank;
  const writable = definition !== 'inherited' && !awaitingObservation;
  const begin = (value: T) => actor.send({ type: 'UNIT.EDIT', identity, selector, revision, value });
  return {
    identity, displayed, outcome, draft: draft !== undefined, authoredPresent, unparsed,
    overriding: draft !== undefined || authoredPresent, inheritance, committed, busy, admitted,
    base, observed, scope, facts, configUnit: unitMutation.kind === 'config',
    definition, shadowed, awaitingObservation, writable,
    // For a value-inherited unit an edit is an unambiguous override
    // transition: it starts from whatever this control displays and becomes
    // this browser's authored intent. An inherited whole definition is only
    // inspected, so an edit of it is refused: viewing is never authoring. The
    // value goes to the transaction actor, never to component or form state
    // that a remount could lose.
    edit: (value: T) => { if (writable) begin(value); },
    override: definition === 'inherited' ? () => { if (!awaitingObservation) begin(overrideSeed); } : undefined,
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
  /** The unit's own card: the region that owns its in-flight mutation and
   * presents the outcome. A confirmed removal submits, which closes every
   * control in the card — its trigger included — until native answers, so
   * focus settles on the card itself, which stays mounted and enabled through
   * the whole write. */
  const card = useRef<HTMLFormElement>(null);
  const preserved = unit.draft ? 'Your draft and original revision are preserved.'
    : unit.committed ? 'Your committed revision is no longer the current source.'
      : 'Your removal and its original revision are preserved.';
  // A Workspace removal restores what it shadows: a semantic unit always has an
  // inherited value, and a resource definition has one exactly when a User
  // definition of the same identity exists. Removing a Workspace definition
  // that shadows nothing is a real deletion and is named as one.
  const restoresInherited = workspace && (unit.definition === undefined || unit.shadowed !== undefined);
  const owner = workspace ? 'Workspace' : 'User';
  return <form ref={card} tabIndex={-1} aria-label={title} className={css.unit} data-definition={unit.definition} data-draft={unit.draft || undefined}
    data-authored-value={unit.definition === 'authored' ? unit.unparsed ? 'unparsed' : 'parsed' : undefined}
    onSubmit={event => { event.preventDefault(); unit.submit(); }}>
    <fieldset disabled={unit.busy}><legend>{title}</legend>
      {unit.configUnit && unit.facts.authored.state !== 'unavailable' && <>
        <p className={css.hint} data-authored={unit.facts.authored.state} data-effective={unit.facts.effective.state}>
          {authoredStateLabel(unit.facts.authored, unit.scope)} · {effectiveStateLabel(unit.facts.effective)} · {provenanceLabel(unit.facts.origin)}
        </p>
        {unit.facts.authored.state === 'invalid' && <p role="alert" className={css.error}>Authored source is invalid. {unit.facts.authored.diagnostic}</p>}
        {unit.facts.effective.state === 'invalid' && <p role="alert" className={css.error}>Native effective resolution failed. {unit.facts.effective.diagnostic}</p>}
        {unit.inheritance && <Advanced title="Native resolved value (not Session adoption)">
          <pre>{unit.facts.effective.state === 'available' ? JSON.stringify(unit.facts.effective.value, null, 2) : effectiveStateLabel(unit.facts.effective)}</pre>
        </Advanced>}
      </>}
      {redacted && <p className={css.hint}>The authored value is never projected to the browser. Saving replaces it with exactly what you enter here.</p>}
      {unit.definition && <DefinitionNotice unit={unit} owner={owner} />}
      {/* Inspecting an inherited definition shows its safe native facts in the
          same fields, and none of them is writable until the explicit
          override transition. */}
      <fieldset className={css.fields} disabled={!unit.writable}>{children}</fieldset>
      <Advanced title="Source revision & replacement">
        <p className={css.hint}>Draft base revision: {unit.base}<br />Current revision: {unit.observed}</p>
        <p>Save replaces this native semantic unit. Remove omits it from this scope. Empty selections remain explicit.</p>
      </Advanced>
      {unit.reviewNeeded && <div className={css.review}>
        <p role="status">Source revision changed. {preserved} Review the current source before replacing it.</p>
      </div>}
      {/* Two different facts, reported separately: the native write is
          definitively committed, and the authoritative reread that settles it
          has been observed. Neither is ever presented as the other. Both sit
          above the actions, which close the card. */}
      {unit.committed && <p role="status">Saved. Native application proceeds automatically.</p>}
      <UnitOutcomeNotice title={title} outcome={unit.outcome} />
      <div className={css.actions}>
        <Button variant="primary" type="submit" disabled={!unit.draft || !unit.admitted}>Save {title}</Button>
        {/* Authoring an absent unit is always an explicit gesture, never
            something rendering it does. For a Workspace it is an override of
            the inherited value; for User it starts this scope's own authored
            value from the neutral seed. It is what makes an explicit empty
            selection authorable without an incidental edit. */}
        {unit.override && <Button type="button" variant="primary"
          title={`Begin a Workspace definition of ${title} from the inherited one. Nothing is written until you save.`}
          disabled={unit.awaitingObservation} onClick={unit.override}>Override {title} in this Workspace</Button>}
        {unit.configUnit && !unit.overriding && <Button type="button" disabled={unit.awaitingObservation}
          title={workspace ? 'Author this unit in this Workspace. Nothing is written until you save.' : 'Author this unit in this source. Nothing is written until you save.'}
          onClick={() => unit.edit(unit.displayed)}>{workspace ? 'Override' : 'Author'} {title}</Button>}
        {removable && unit.authoredPresent && (restoresInherited
          // A Workspace removal is inheritance, not destruction: it removes the
          // unit this Workspace authors, through exact CAS, and the native
          // inherited value becomes effective again. The effective resource
          // survives, so this is never presented as deleting it.
          ? <span data-removal="override-removal"><ConfirmAction tone="restore" label={`Use global default ${title}`} disabled={!unit.admitted}
            title={`Use the global default for ${title}?`} confirm={`Use global default ${title}`}
            description={<><p>This removes the semantic unit this Workspace authors, through exact CAS. The native inherited value becomes effective again.</p><p>Nothing is removed from the global source, and no other scope is changed.</p></>}
            settle={card} onConfirm={() => unit.submit(true)} /></span>
          // Otherwise the removal really removes this scope's authored unit.
          : <span data-removal="authored-removal"><ConfirmAction tone="destructive" label={`Remove ${title}`} disabled={!unit.admitted}
            title={`Remove ${title} from ${owner} configuration?`} confirm={`Remove ${title}`}
            description={<><p>This removes the value this {owner} source authors, through exact CAS on its current revision.</p>{removalNotice ?? <p>The native default for this unit applies once it is absent.</p>}</>}
            settle={card} onConfirm={() => unit.submit(true)} /></span>)}
        {unit.intent && <Button type="button" onClick={unit.discard}>Discard draft</Button>}
        {unit.reviewNeeded && <Button type="button" onClick={unit.review}>Use reviewed revision</Button>}
      </div>
    </fieldset>
  </form>;
}

/** What one whole-identity resource definition is in this scope, worded for
 * exactly its lifecycle state. */
function DefinitionNotice<T>({ unit, owner }: { unit: UnitEditing<T>; owner: string }) {
  const shadowed = unit.shadowed;
  const withheld = shadowed && shadowed.withheld.length > 0 && <p className={css.hint}>
    The User definition holds literal values for {shadowed.withheld.join(', ')}. Native never projects them, so an override does not copy them; enter them again if this Workspace needs them.
  </p>;
  switch (unit.definition) {
    case 'new': return <p role="status" data-definition-state="new">New {owner} definition. Nothing is written until you save.</p>;
    case 'inherited': return <>
      <p role="status" data-definition-state="inherited">Inherited from User ({shadowed!.path}). This Workspace authors no definition, so these fields are read-only. Override in this Workspace to author one that replaces the whole User definition.</p>
      {shadowed!.seed === undefined && <p role="status">The User definition's content is not available to this browser, so an override starts from an empty definition.</p>}
      {shadowed!.diagnostic && <p role="alert" className={css.error}>{shadowed!.diagnostic}</p>}
      {withheld}
    </>;
    case 'overriding': return <>
      <p role="status" data-definition-state="overriding">Workspace override draft. Saving creates a Workspace definition that replaces the whole User definition; discarding it keeps the User definition in effect.</p>
      {withheld}
    </>;
    case 'authored': return <p role="status" data-definition-state="authored">
      {owner} definition{shadowed ? ' — overrides the User definition of the same identity' : ''}.
      {unit.unparsed && ' Its file does not parse, so no field shows its content. Saving replaces the whole file with the definition entered here, on its current revision.'}
    </p>;
    default: return null;
  }
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
    case 'conflict': return <p role="alert" className={css.error}>{title} was not saved: the source changed. Your draft and base revision are preserved.</p>;
    case 'rejected': return <p role="alert" className={css.error}>{title} was not saved. {outcome.detail}</p>;
    case 'uncertain': return <p role="alert" className={css.error}>The outcome of saving {title} is unknown. Authority is reread; the write is never replayed. Review the current source before saving again.</p>;
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
  // The latest unit facts, for the change listener the form instance holds.
  const current = useRef(unit);
  current.current = unit;
  const form = useForm({
    defaultValues: unit.displayed,
    listeners: {
      onChange: ({ formApi }) => {
        const { writable, displayed, edit } = current.current;
        // An inspected inherited definition, or a unit whose commit awaits its
        // observation, is not writable: the transaction owner refuses the
        // edit, so the field returns to the owner's value at once rather than
        // holding a local value no draft backs.
        if (!writable) { formApi.reset(displayed); return; }
        const values = formApi.state.values as T;
        reflected.current = values;
        edit(values);
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
