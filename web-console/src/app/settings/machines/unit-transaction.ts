import { and, assign, not, raise, sendParent, setup, stateIn, type SnapshotFrom } from 'xstate';
import type { RevisionSelector } from '../projection';

/** The editing transaction of exactly one native semantic unit.
 *
 * Three facts about a unit are genuinely independent, so they are three regions
 * rather than a combination of `draft?` / `pinned` / `submitting?` flags:
 *
 * - `intent`   — does this browser hold a value override for the unit;
 * - `base`     — is the CAS revision the next mutation is fenced on frozen at a
 *                revision the user reviewed, or following native authority;
 * - `mutation` — where the last submitted mutation stands natively.
 *
 * A clean Remove is exactly the combination `intent.clean` + `base.pinned`:
 * delete intent pins the reviewed revision without manufacturing a value draft.
 * A late acknowledgement of an older intent is exactly `mutation.acknowledged`
 * while `intent.dirty` already holds a newer generation.
 *
 * Whether the current source diverges from the CAS base is a fact of this
 * actor, not of the editor that renders it: `requiresReview` answers it from
 * the regions above. A definitive commit advances the base before any
 * authoritative read has observed it, so while `mutation.acknowledged` is
 * `awaitingObservation` the difference is the commit not yet observed. Once an
 * authoritative observation issued after the commit has completed, any revision
 * other than the committed one is a real external change — whatever revision
 * value it happens to carry — and `acknowledged.diverged` says so.
 *
 * The actor is owned by the Settings target actor, not by the editor that
 * renders it, so section changes and editor remounts cannot lose a pinned base,
 * and an acknowledgement settles a transaction whose editor is already gone.
 *
 * `DISCARD` abandons browser authoring intent and nothing else: the value
 * draft, and a base pinned as intent — the reviewed revision of a clean Remove
 * or of a draft whose mutation did not commit. A definitive commit is not
 * intent. While `mutation.acknowledged` holds, the base carries the committed
 * revision and the transaction still owes that commit's authoritative
 * observation and any divergence it reveals, so the pinned base, the committed
 * revision and the review requirement all survive the gesture. While a
 * mutation is in flight the intent belongs to it, and the gesture is not
 * accepted.
 *
 * The fourth region, `lifetime`, is the transaction's own ownership decision.
 * The transaction retires once it owns nothing at all — no intent, no pinned
 * base and no commit left to observe — which is a fact about the other three
 * regions, not about the event that led there. Two events can release the last
 * thing it owns: the discard gesture, and the authoritative observation that
 * settles a definitive commit. Each is answered by every region in its own
 * microstep and then raises `RELEASE`, which XState processes as the next
 * microstep, after all of them. Only `RELEASE` evaluates ownership, so the
 * decision always sees the regions *after* the releasing event: a base pinned
 * by the settled commit has already followed native authority, and a newer
 * intent is still dirty. `lifetime.retired` has no way out, so its entry
 * announces the retirement to the owning target exactly once.
 *
 * Memory only. Nothing here is persisted, merged with sources, or used as
 * runtime state. */
export interface UnitTransactionContext {
  /** `JSON.stringify` of the unit's own mutation shape; the unit's identity. */
  readonly identity: string;
  /** The non-sensitive native descriptor naming which source revision settles
   * this unit's mutations. Never the authored payload. */
  readonly selector: RevisionSelector;
  /** The exact CAS revision the next mutation is fenced on. */
  base: string;
  /** The exact revision this unit's authoritative projection currently carries. */
  observed: string;
  /** This browser's value override. Present only after an explicit Override or
   * a real edit — never created by rendering, opening or navigating. It is the
   * only place an authored secret lives, and acknowledging its commit drops it. */
  draft?: { value: unknown };
  /** Monotonic browser-intent generation. Every edit bumps it, so an older
   * submitted mutation may advance the CAS base but can never retire a newer
   * intent. */
  generation: number;
  /** The last submitted mutation. Deliberately carries no authored payload:
   * settlement needs the token that identifies it, the intent generation it was
   * submitted for, and once confirmed the committed revision — never a Provider
   * credential or a literal environment value. */
  submitted?: { token: number; generation: number; committed?: string };
}

export type UnitTransactionEvent =
  | { type: 'EDIT'; value: unknown }
  | { type: 'DISCARD' }
  | { type: 'REVIEW' }
  | { type: 'SUBMIT'; token: number }
  | { type: 'COMMITTED'; token: number; revision: string }
  | { type: 'FAILED'; token: number }
  /** An authoritative projection carries `revision` for this unit.
   * `postCommit` is true exactly when the read that produced it was issued after
   * every definitive commit the owning target has recorded, so it is evidence
   * about this unit's commit rather than about the source before it. */
  | { type: 'OBSERVED'; revision: string; postCommit: boolean }
  /** Internal: a discard or a commit settlement has been answered by every
   * region; retire if the transaction now owns nothing. */
  | { type: 'RELEASE' };

export const unitTransactionMachine = setup({
  types: {
    context: {} as UnitTransactionContext,
    events: {} as UnitTransactionEvent,
    input: {} as { identity: string; selector: RevisionSelector; revision: string },
  },
  guards: {
    /** The acknowledgement or failure belongs to the mutation this unit owns. */
    isCurrentSubmission: ({ context, event }) =>
      (event.type === 'COMMITTED' || event.type === 'FAILED') && context.submitted?.token === event.token,
    /** The authoritative projection now carries exactly the committed revision.
     * This is the transaction's settlement point. */
    settlesCommit: ({ context, event }) =>
      event.type === 'OBSERVED' && context.submitted?.committed !== undefined && context.submitted.committed === event.revision,
    /** The observation was issued after the commit and does not carry it: the
     * source moved on from the committed revision. */
    divergesFromCommit: ({ context, event }) =>
      event.type === 'OBSERVED' && event.postCommit && context.submitted?.committed !== event.revision,
    /** The browser intent has not moved on since the submitted mutation, so the
     * confirmed draft — the last place an authored secret lives — is dropped. */
    intentUnchanged: ({ context }) => context.submitted?.generation === context.generation,
    /** Nothing is left for this transaction to fence on. */
    nothingAuthored: ({ context }) => context.draft === undefined,
    /** A submitted mutation owns the browser intent until its outcome. */
    inFlight: stateIn({ mutation: 'submitting' }),
    /** No mutation is in flight and no definitive commit still owes its
     * observation or carries its divergence, so a pinned base is browser intent
     * rather than the committed revision of a commit. */
    noMutationObligation: and([not('inFlight'), not(stateIn({ mutation: 'acknowledged' }))]),
    /** No browser intent, no pinned base and no native commit whose
     * observation is still owed: the transaction owns nothing. */
    ownsNothing: and([stateIn({ intent: 'clean' }), stateIn({ base: 'following' }), 'noMutationObligation']),
  },
  actions: {
    recordObservation: assign({ observed: ({ context, event }) => event.type === 'OBSERVED' ? event.revision : context.observed }),
    followObservation: assign({ base: ({ context, event }) => event.type === 'OBSERVED' ? event.revision : context.observed }),
    beginSubmission: assign({
      submitted: ({ context, event }) => event.type === 'SUBMIT'
        ? { token: event.token, generation: context.generation }
        : context.submitted,
    }),
    recordCommit: assign({
      base: ({ context, event }) => event.type === 'COMMITTED' ? event.revision : context.base,
      submitted: ({ context, event }) => event.type === 'COMMITTED' && context.submitted
        ? { ...context.submitted, committed: event.revision }
        : context.submitted,
    }),
    retireSubmission: assign({ submitted: () => undefined }),
    dropDraft: assign({ draft: () => undefined }),
    recordEdit: assign({
      draft: ({ context, event }) => event.type === 'EDIT' ? { value: event.value } : context.draft,
      generation: ({ context }) => context.generation + 1,
    }),
    reviewObservation: assign({ base: ({ context }) => context.observed }),
    announceRetirement: sendParent(({ context }) => ({ type: 'UNIT.RETIRED' as const, identity: context.identity })),
  },
}).createMachine({
  id: 'unitTransaction',
  context: ({ input }) => ({
    identity: input.identity,
    selector: input.selector,
    base: input.revision,
    observed: input.revision,
    generation: 0,
  }),
  type: 'parallel',
  states: {
    /** Does this browser hold a value override for the unit? */
    intent: {
      initial: 'clean',
      on: {
        // Every region answers the same discard in this microstep; the
        // retirement decision is taken once they all have.
        DISCARD: { guard: not('inFlight'), target: '.clean', actions: ['dropDraft', raise({ type: 'RELEASE' })] },
      },
      states: {
        clean: { on: { EDIT: { target: 'dirty', actions: 'recordEdit' } } },
        dirty: {
          on: {
            EDIT: { target: 'dirty', actions: 'recordEdit' },
            // A confirmed commit of exactly this intent drops the authored
            // draft. A newer intent survives its own older acknowledgement.
            COMMITTED: { guard: 'intentUnchanged', target: 'clean', actions: 'dropDraft' },
          },
        },
      },
    },
    /** Is the CAS base frozen at a revision the user reviewed, or following
     * native authority? A clean Remove pins without authoring a value. */
    base: {
      initial: 'following',
      states: {
        following: {
          on: {
            OBSERVED: { actions: ['recordObservation', 'followObservation'] },
            EDIT: 'pinned',
            SUBMIT: 'pinned',
            REVIEW: { target: 'pinned', actions: 'reviewObservation' },
          },
        },
        pinned: {
          on: {
            // A pinned base never advances on an observation alone. Only a
            // confirmed commit, the explicit reviewed-revision gesture, or the
            // observation that settles the commit of a transaction that
            // authored nothing since moves it. The settling observation is the
            // same event the `mutation` region settles on, so both regions
            // answer it in one microstep, before `RELEASE` is evaluated.
            OBSERVED: [
              { guard: and(['settlesCommit', 'nothingAuthored']), target: 'following', actions: ['recordObservation', 'followObservation'] },
              { actions: 'recordObservation' },
            ],
            REVIEW: { actions: 'reviewObservation' },
            // Abandoning a pin that is browser intent follows native authority
            // again. A committed revision awaiting its observation is not
            // intent and stays.
            DISCARD: { guard: 'noMutationObligation', target: 'following', actions: 'followObservation' },
          },
        },
      },
    },
    /** Where does the last submitted mutation stand natively? */
    mutation: {
      initial: 'idle',
      states: {
        idle: { on: { SUBMIT: { target: 'submitting', actions: 'beginSubmission' } } },
        submitting: {
          on: {
            COMMITTED: { guard: 'isCurrentSubmission', target: 'acknowledged', actions: 'recordCommit' },
            FAILED: { guard: 'isCurrentSubmission', target: 'unconfirmed', actions: 'retireSubmission' },
          },
        },
        /** The native write is definitive and `submitted.committed` names it.
         * The transaction may retire only once an authoritative projection
         * carries exactly the committed revision: that observation settles
         * the mutation, and `RELEASE` then decides whether anything is left. */
        acknowledged: {
          initial: 'awaitingObservation',
          on: {
            OBSERVED: {
              guard: 'settlesCommit',
              target: 'settled',
              actions: ['recordObservation', 'retireSubmission', raise({ type: 'RELEASE' })],
            },
            SUBMIT: { target: 'submitting', actions: 'beginSubmission' },
          },
          states: {
            /** No authoritative read issued after the commit has completed.
             * The observation this unit still holds predates the commit, so it
             * differing from the committed base is not a source change. */
            awaitingObservation: {
              // A child transition is selected before the parent's, so the guard
              // leaves the settling observation to `acknowledged` itself.
              on: { OBSERVED: { guard: 'divergesFromCommit', target: 'diverged' } },
            },
            /** An authoritative read issued after the commit completed and the
             * source no longer carries the committed revision. The commit stays
             * a definitive fact; the current source diverges from it and must be
             * reviewed before anything replaces it. A later observation of the
             * committed revision still settles the transaction. */
            diverged: {},
          },
        },
        /** Committed and observed. Nothing about the commit is outstanding; a
         * transaction still here holds a newer browser intent, or is retired. */
        settled: {
          on: {
            EDIT: 'idle',
            SUBMIT: { target: 'submitting', actions: 'beginSubmission' },
          },
        },
        /** The submission ended without a definitive commit: a conflict, a
         * native rejection or an unknown outcome, which the owning target's
         * mutation region tells apart. The browser intent and the exact reviewed
         * base are preserved, and the mutation is never replayed from here. */
        unconfirmed: { on: { SUBMIT: { target: 'submitting', actions: 'beginSubmission' } } },
      },
    },
    /** Whether the owning target still holds this transaction. */
    lifetime: {
      initial: 'live',
      states: {
        live: { on: { RELEASE: { guard: 'ownsNothing', target: 'retired' } } },
        /** Terminal: the announcement is made on the one entry this state can
         * ever have, and the owning target stops the actor on receiving it. */
        retired: { type: 'final', entry: 'announceRetirement' },
      },
    },
  },
});

export type UnitTransactionSnapshot = SnapshotFrom<typeof unitTransactionMachine>;

/** Whether the current source diverges from the exact CAS base the next
 * mutation is fenced on, so replacing it needs the explicit reviewed-revision
 * gesture. The one difference that is not a divergence is a definitive commit
 * no post-commit authoritative observation has completed for yet. */
export function requiresReview(snapshot: UnitTransactionSnapshot): boolean {
  return snapshot.context.base !== snapshot.context.observed
    && !snapshot.matches({ mutation: { acknowledged: 'awaitingObservation' } });
}

/** Whether the transaction holds browser authoring intent that `DISCARD`
 * would abandon: a value draft, or a base pinned as intent rather than as the
 * committed revision of a definitive commit. */
export function discardable(snapshot: UnitTransactionSnapshot): boolean {
  if (snapshot.matches({ mutation: 'submitting' })) return false;
  return snapshot.matches({ intent: 'dirty' })
    || (snapshot.matches({ base: 'pinned' }) && !snapshot.matches({ mutation: 'acknowledged' }));
}

/** Whether the last submitted mutation is natively confirmed. */
export function committed(snapshot: UnitTransactionSnapshot): boolean {
  return snapshot.matches({ mutation: 'acknowledged' }) || snapshot.matches({ mutation: 'settled' });
}
