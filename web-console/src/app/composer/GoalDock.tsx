/* Copyright (c) 2026 DeepSeek. MIT. Source-derived; see PROVENANCE.md. */
// Adapted from DeepSeek Harness ui-goal GoalBar: the strip, phase labels, icon
// actions, single-flight controls and inline edit form. rustX GoalDomain owns
// phase, revision, budget, CAS and every value/transition limit; this card owns
// only its draft, action feedback and a lock pending authority. Durable
// GoalPhase is the sole lifecycle authority (Issue #351), so an Active Goal
// offers Pause and a stopped one offers Resume — never both, and never a
// separate arm/play step. There is no create, clear or complete control here.
import { useEffect, useRef, useState, type KeyboardEvent } from 'react';
import type { GoalMutation, GoalRef, GoalSnapshot } from '../../../../protocol/app-server/v19';
import type { GoalDockState } from '../../bindings/composer-context';
import type { GoalControlOutcome } from '../../client/app-server';
import {
  IconCheckOutline14, IconCloseOutline16, IconEditOutline16, IconGoalOutline16, IconPauseOutline16, IconPlayOutline16,
} from '../../presentation/primitives/icons';
import css from './GoalDock.module.css';

type Field = 'objective' | 'budget';
/** A control outcome not yet followed by a successful authoritative snapshot read. */
type Awaiting = { at: object | undefined; kind: 'applied' | 'rejected' | 'uncertain' };

const BudgetGlyph = () => <svg width={14} height={14} viewBox="0 0 14 14" fill="none" aria-hidden>
  <path d="M2.5 4h9M2.5 7h6M2.5 10h3.5" stroke="currentColor" strokeWidth="1.2" />
</svg>;

/** `GoalMutation::Budget.rounds` is a wire `u32`. The draft is parsed exactly,
 * through `BigInt`, so a decimal the protocol integer cannot represent is refused
 * here rather than after a lossy conversion: `Number` would silently round a large
 * value, reach `Infinity`, and serialize as JSON `null`. This is representability
 * only — GoalDomain still owns the range, the consumption floor and legality. */
const U32_MAX = 0xffff_ffffn;
function parseBudget(draft: string): number | undefined {
  if (!/^[1-9]\d*$/.test(draft)) return undefined;
  const parsed = BigInt(draft);
  return parsed > U32_MAX ? undefined : Number(parsed);
}

/** The user-facing status word for each durable phase. `complete` never
 * reaches this card — the dock is hidden for a finished Goal — but the map is
 * total so a phase can never render as a blank label. */
const STATUS: Record<GoalSnapshot['phase'], string> = {
  active: 'Active', paused: 'Paused', blocked: 'Blocked', complete: 'Complete',
};

const AWAITING: Record<Awaiting['kind'], string> = {
  applied: 'Goal control applied. Controls stay locked until the authoritative Goal state is reread.',
  rejected: 'Controls stay locked until the authoritative Goal state is reread.',
  uncertain: 'Goal control outcome uncertain. It was not replayed; waiting for an authoritative reread.',
};

export function GoalDock({ state, observation, disabled, mutate }: {
  state: GoalDockState | undefined;
  /** Identity of the rendered authoritative snapshot. The client replaces it only after a successful read. */
  observation: object | undefined;
  disabled: boolean;
  mutate: (expected: GoalRef, mutation: GoalMutation) => Promise<GoalControlOutcome>;
}) {
  const goal = state?.goal;
  const [editing, setEditing] = useState<{ field: Field; draft: string }>();
  const [pending, setPending] = useState(false);
  const [error, setError] = useState<string>();
  const [awaiting, setAwaiting] = useState<Awaiting>();
  const inFlight = useRef(false);
  const latestObservation = useRef(observation);
  const returnFocus = useRef<Field>(undefined);
  const objectiveButton = useRef<HTMLButtonElement>(null);
  const budgetButton = useRef<HTMLButtonElement>(null);
  useEffect(() => { latestObservation.current = observation; }, [observation]);
  // A new identity invalidates the local form: a surviving draft must not
  // write over a different Goal.
  const id = goal?.reference.id;
  useEffect(() => { setEditing(undefined); setError(undefined); }, [id]);
  // A draft is never saved over an authoritative value its author did not see.
  // Revision-only changes (an admitted autonomous round) keep the draft.
  const objective = goal?.objective, budget = goal?.autonomous_round_budget;
  useEffect(() => { setEditing(current => current?.field === 'objective' ? undefined : current); }, [objective]);
  useEffect(() => { setEditing(current => current?.field === 'budget' ? undefined : current); }, [budget]);
  useEffect(() => {
    if (editing || !returnFocus.current) return;
    (returnFocus.current === 'objective' ? objectiveButton : budgetButton).current?.focus();
    returnFocus.current = undefined;
  });

  if (!state || !goal) return null;
  // Only a newer authoritative observation releases the lock: never request
  // completion, acknowledgement alone, a rerender or a timer.
  const locking = awaiting !== undefined && awaiting.at === observation ? awaiting : undefined;
  const locked = disabled || pending || !!locking;
  const run = async (mutation: GoalMutation, field?: Field) => {
    if (inFlight.current) return;
    inFlight.current = true; setPending(true); setError(undefined);
    // The rendered authoritative GoalRef is the CAS token. Nothing is retried.
    const outcome = await mutate(goal.reference, mutation);
    inFlight.current = false; setPending(false);
    if (outcome.status === 'obsolete') return;
    if (outcome.status === 'rejected') setError(outcome.reason);
    if (outcome.status === 'applied' && field) { returnFocus.current = field; setEditing(undefined); }
    if (outcome.status === 'uncertain' || !outcome.observed) setAwaiting({ at: latestObservation.current, kind: outcome.status });
  };
  // Input grammar and wire representability only. GoalDomain owns the budget
  // range, the consumption floor and transition legality.
  const budgetRounds = editing?.field === 'budget' ? parseBudget(editing.draft) : undefined;
  const draftValid = !!editing && (editing.field === 'objective' ? editing.draft.trim() !== '' : budgetRounds !== undefined);
  const save = () => {
    if (!editing || locked) return;
    if (editing.field === 'objective') {
      const objective = editing.draft.trim();
      if (objective) void run({ action: 'edit', objective }, 'objective');
      return;
    }
    // A Budget mutation is constructed only from a losslessly representable value.
    if (budgetRounds !== undefined) void run({ action: 'budget', rounds: budgetRounds }, 'budget');
  };
  const cancel = () => { if (!editing || pending) return; returnFocus.current = editing.field; setEditing(undefined); };
  const open = (field: Field) => { setError(undefined); setEditing({ field, draft: field === 'objective' ? goal.objective : String(goal.autonomous_round_budget) }); };
  const onKeyDown = (event: KeyboardEvent<HTMLInputElement>) => {
    if (event.key === 'Escape') { event.preventDefault(); cancel(); }
    else if (event.key === 'Enter' && !event.nativeEvent.isComposing) { event.preventDefault(); save(); }
  };
  // Issue #351: the status line is the durable phase and nothing else.
  // `Active` is authorization to continue, so there is no Inactive Goal and
  // no separate Play control beside it.
  const label = `${STATUS[goal.phase]} Goal`;

  return <section className={css.dock} aria-label="Goal" data-goal-phase={goal.phase}>
    <div className={css.bar} title={goal.phase === 'blocked' ? goal.blocked_reason ?? undefined : undefined}>
      <span className={css.glyph} aria-hidden><IconGoalOutline16 size={14} /></span>
      {editing
        ? <input className={css.input} autoFocus aria-label={editing.field === 'objective' ? 'Goal objective' : 'Autonomous round budget'}
          {...editing.field === 'budget' ? { type: 'number', min: 1, step: 1, inputMode: 'numeric' as const } : { type: 'text' }}
          value={editing.draft} disabled={pending} onChange={event => setEditing({ field: editing.field, draft: event.target.value })} onKeyDown={onKeyDown} />
        : <><span className={css.label}>{label}</span><span className={css.objective} title={goal.objective}>{goal.objective}</span></>}
      <span className={css.meta}>{goal.autonomous_rounds_consumed}/{goal.autonomous_round_budget} rounds</span>
      <div className={css.actions}>{editing ? <>
        <button type="button" className={css.iconButton} aria-label={editing.field === 'objective' ? 'Save goal objective' : 'Save round budget'} disabled={locked || !draftValid} onClick={save}><IconCheckOutline14 /></button>
        <button type="button" className={css.iconButton} aria-label="Cancel goal edit" disabled={pending} onClick={cancel}><IconCloseOutline16 size={14} /></button>
      </> : <>
        {goal.phase === 'active'
          ? <button type="button" className={css.iconButton} aria-label="Pause goal" disabled={locked} onClick={() => void run({ action: 'pause' })}><IconPauseOutline16 size={14} /></button>
          : <button type="button" className={css.iconButton} aria-label="Resume goal" disabled={locked} onClick={() => void run({ action: 'resume' })}><IconPlayOutline16 size={14} /></button>}
        <button ref={objectiveButton} type="button" className={css.iconButton} aria-label="Edit goal objective" disabled={locked} onClick={() => open('objective')}><IconEditOutline16 size={14} /></button>
        <button ref={budgetButton} type="button" className={css.iconButton} aria-label="Edit round budget" disabled={locked} onClick={() => open('budget')}><BudgetGlyph /></button>
      </>}</div>
    </div>
    {goal.phase === 'blocked' && goal.blocked_reason && <p className={css.note}>Blocked: {goal.blocked_reason}</p>}
    {error && <p className={css.error} role="alert">{error}</p>}
    {locking && <p className={css.note} role="status">{AWAITING[locking.kind]}</p>}
  </section>;
}
