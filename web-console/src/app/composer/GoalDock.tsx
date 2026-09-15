/* Copyright (c) 2026 DeepSeek. MIT. Source-derived; see PROVENANCE.md. */
// Adapted from DeepSeek Harness ui-goal GoalBar: the strip, phase labels, icon
// actions, single-flight controls and inline edit form. rustX GoalDomain owns
// phase, revision, budget, activation and CAS; this card owns only its draft and
// action feedback. There is no create, clear or complete control here.
import { useEffect, useRef, useState, type KeyboardEvent } from 'react';
import type { GoalMutation, GoalRef } from '../../../../protocol/app-server/v4';
import type { GoalDockState } from '../../bindings/composer-context';
import type { GoalControlOutcome } from '../../client/app-server';
import {
  IconCheckOutline14, IconCloseOutline16, IconEditOutline16, IconGoalOutline16, IconPauseOutline16, IconPlayOutline16,
} from '../../presentation/primitives/icons';
import css from './GoalDock.module.css';

/** Native autonomous-round bound (`goal::MAX_ROUND_BUDGET`); the owner validates again. */
const MAX_ROUND_BUDGET = 100;
type Field = 'objective' | 'budget';

const BudgetGlyph = () => <svg width={14} height={14} viewBox="0 0 14 14" fill="none" aria-hidden>
  <path d="M2.5 4h9M2.5 7h6M2.5 10h3.5" stroke="currentColor" strokeWidth="1.2" />
</svg>;

export function GoalDock({ state, observation, disabled, mutate }: {
  state: GoalDockState | undefined;
  /** Identity of the authoritative snapshot rendered; only a newer read settles uncertainty. */
  observation: object | undefined;
  disabled: boolean;
  mutate: (expected: GoalRef, mutation: GoalMutation) => Promise<GoalControlOutcome>;
}) {
  const goal = state?.goal;
  const [editing, setEditing] = useState<{ field: Field; draft: string }>();
  const [pending, setPending] = useState(false);
  const [error, setError] = useState<string>();
  const [uncertainAt, setUncertainAt] = useState<object>();
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
  const uncertain = uncertainAt !== undefined && uncertainAt === observation;
  const locked = disabled || pending || uncertain;
  const minimum = Math.max(1, goal.autonomous_rounds_consumed);
  const run = async (mutation: GoalMutation, field?: Field) => {
    if (inFlight.current) return;
    inFlight.current = true; setPending(true); setError(undefined);
    // The rendered authoritative GoalRef is the CAS token. Refusal rereads; nothing is retried.
    const outcome = await mutate(goal.reference, mutation);
    inFlight.current = false; setPending(false);
    if (outcome.status === 'applied') { if (field) { returnFocus.current = field; setEditing(undefined); } }
    else if (outcome.status === 'rejected') setError(outcome.reason);
    else if (outcome.status === 'uncertain') setUncertainAt(latestObservation.current);
  };
  const draftValid = !!editing && (editing.field === 'objective' ? editing.draft.trim() !== ''
    : /^\d+$/.test(editing.draft) && Number(editing.draft) >= minimum && Number(editing.draft) <= MAX_ROUND_BUDGET);
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
          {...editing.field === 'budget' ? { type: 'number', min: minimum, max: MAX_ROUND_BUDGET, step: 1, inputMode: 'numeric' as const } : { type: 'text' }}
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
    {uncertain && <p className={css.note} role="status">Goal control outcome uncertain. It was not replayed; waiting for an authoritative reread.</p>}
  </section>;
}
