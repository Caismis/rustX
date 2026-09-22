import { assign, raise, sendParent, setup } from 'xstate';
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
 * The actor is owned by the Settings target actor, not by the editor that
 * renders it, so section changes and editor remounts cannot lose a pinned base,
 * and an acknowledgement settles a transaction whose editor is already gone.
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
   * submitted for, the exact pre-save base, and once confirmed the committed
   * revision — never a Provider credential or a literal environment value. */
  submitted?: { token: number; generation: number; savedFrom: string; committed?: string };
}

export type UnitTransactionEvent =
  | { type: 'EDIT'; value: unknown }
  | { type: 'DISCARD' }
  | { type: 'REVIEW' }
  | { type: 'SUBMIT'; token: number }
  | { type: 'COMMITTED'; token: number; revision: string }
  | { type: 'FAILED'; token: number }
  | { type: 'OBSERVED'; revision: string }
  | { type: 'SETTLED' };

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
    /** The browser intent has not moved on since the submitted mutation, so the
     * confirmed draft — the last place an authored secret lives — is dropped. */
    intentUnchanged: ({ context }) => context.submitted?.generation === context.generation,
    /** Nothing is left for this transaction to fence on. */
    nothingAuthored: ({ context }) => context.draft === undefined,
  },
  actions: {
    recordObservation: assign({ observed: ({ context, event }) => event.type === 'OBSERVED' ? event.revision : context.observed }),
    followObservation: assign({ base: ({ context, event }) => event.type === 'OBSERVED' ? event.revision : context.observed }),
    beginSubmission: assign({
      submitted: ({ context, event }) => event.type === 'SUBMIT'
        ? { token: event.token, generation: context.generation, savedFrom: context.base }
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
            // settlement of a transaction that authored nothing moves it.
            OBSERVED: { actions: 'recordObservation' },
            REVIEW: { actions: 'reviewObservation' },
            SETTLED: { guard: 'nothingAuthored', target: 'following', actions: 'followObservation' },
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
            FAILED: { guard: 'isCurrentSubmission', target: 'conflicted', actions: 'retireSubmission' },
          },
        },
        /** The native write is definitive. The transaction is retired only once
         * an authoritative projection carries exactly the committed revision. */
        acknowledged: {
          on: {
            OBSERVED: {
              guard: 'settlesCommit',
              target: 'settled',
              actions: ['recordObservation', 'retireSubmission', raise({ type: 'SETTLED' })],
            },
            SUBMIT: { target: 'submitting', actions: 'beginSubmission' },
          },
        },
        /** Committed and observed. Nothing about this unit is outstanding. */
        settled: {
          on: {
            EDIT: 'idle',
            SUBMIT: { target: 'submitting', actions: 'beginSubmission' },
          },
        },
        /** The write did not commit. The browser intent and the exact reviewed
         * base are preserved, and the mutation is never replayed from here. */
        conflicted: { on: { SUBMIT: { target: 'submitting', actions: 'beginSubmission' } } },
      },
    },
  },
  on: { DISCARD: { actions: 'announceRetirement' } },
});
