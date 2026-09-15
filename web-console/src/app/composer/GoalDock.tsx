/* Copyright (c) 2026 DeepSeek. MIT. Source-derived; see PROVENANCE.md. */
// Adapted from DeepSeek Harness ui-goal GoalBar: the strip, phase labels, icon
// actions, single-flight controls and inline edit form. rustX GoalDomain owns
// phase, revision, budget, activation, CAS and every value/transition limit;
// this card owns only its draft, action feedback and a lock pending authority.
// There is no create, clear or complete control here.
import { useEffect, useRef, useState, type KeyboardEvent } from 'react';
import type { GoalMutation, GoalRef } from '../../../../protocol/app-server/v3';
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
  // Revision-only changes (autonomous admission, activation) keep the draft.
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
  // Input grammar only. GoalDomain owns the budget range, consumption and transitions.
  const draftValid = !!editing && (editing.field === 'objective' ? editing.draft.trim() !== '' : /^[1-9]\d*$/.test(editing.draft));
  const save = () => {
    if (!editing || !draftValid || locked) return;
    void run(editing.field === 'objective' ? { action: 'edit', objective: editing.draft.trim() } : { action: 'budget', rounds: Number(editing.draft) }, editing.field);
  };
  const cancel = () => { if (!editing || pending) return; returnFocus.current = editing.field; setEditing(undefined); };
  const open = (field: Field) => { setError(undefined); setEditing({ field, draft: field === 'objective' ? goal.objective : String(goal.autonomous_round_budget) }); };
  const onKeyDown = (event: KeyboardEvent<HTMLInputElement>) => {
    if (event.key === 'Escape') { event.preventDefault(); cancel(); }
    else if (event.key === 'Enter' && !event.nativeEvent.isComposing) { event.preventDefault(); save(); }
  };
  const label = goal.phase === 'active' ? state.armed ? 'Ongoing Goal' : 'Inactive Goal' : goal.phase === 'paused' ? 'Paused Goal' : 'Blocked Goal';

  return <section className={css.dock} aria-label="Goal" data-goal-phase={goal.phase} data-goal-armed={String(state.armed)}>
    <div className={css.bar} title={goal.phase === 'blocked' ? goal.blocked_reason ?? undefined : undefined}>
      <span className={css.glyph} aria-hidden><IconGoalOutline16 size={14} /></span>
      {editing
        ? <input className={css.input} autoFocus aria-label={editing.field === 'objective' ? 'Goal objective' : 'Autonomous round budget'}
          {...editing.field === 'budget' ? { type: 'number', min: 1, step: 1, inputMode: 'numeric' as const } : { type: 'text' }}
          value={editing.draft} disabled={pending} onChange={event => setEditing({ field: editing.field, draft: event.target.value })} onKeyDown={onKeyDown} />
        : <><span className={css.label}>{label}</span><span className={css.objective} title={goal.objective}>{goal.objective}</span></>}
      <span className={css.meta}>{goal.autonomous_rounds_consumed}/{goal.autonomous_round_budget} rounds · r{goal.reference.revision}</span>
      <div className={css.actions}>{editing ? <>
        <button type="button" className={css.iconButton} aria-label={editing.field === 'objective' ? 'Save goal objective' : 'Save round budget'} disabled={locked || !draftValid} onClick={save}><IconCheckOutline14 /></button>
        <button type="button" className={css.iconButton} aria-label="Cancel goal edit" disabled={pending} onClick={cancel}><IconCloseOutline16 size={14} /></button>
      </> : <>
        {goal.phase === 'active' && state.armed
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
