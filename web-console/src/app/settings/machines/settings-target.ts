import { assign, enqueueActions, fromPromise, raise, setup, type ActorRefFrom } from 'xstate';
import type { ConfigurationApplication, SourceMutation, SourceSettings } from '../../../../../protocol/app-server/v18';
import { isOutcomeUncertain, RpcFailure, type ConnectionState } from '../../../client/app-server';
import { WorkspaceHostError } from '../../../workspaces/host';
import { applicationScope, selectedRevision, type RevisionSelector, type SettingsTarget } from '../projection';
import type { ConfigurationPort, WriteOutcome } from './port';
import { unitTransactionMachine } from './unit-transaction';

export type UnitTransactionRef = ActorRefFrom<typeof unitTransactionMachine>;

/** Why the last submitted mutation did not commit. Conflict, native rejection
 * and an unknown outcome are three different facts and are never collapsed. */
type WriteFailureKind = 'conflict' | 'rejected' | 'uncertain';

export interface SettingsTargetContext {
  /** The one owner this actor is bound to for its whole lifetime. Session focus
   * never retargets it; only authority replacement retires it. */
  readonly target: SettingsTarget;
  /** The native authority of this exact target, fixed for the actor's life:
   * endpoint, target and Product Host access are all part of its identity. */
  readonly port: ConfigurationPort;
  connection: ConnectionState;
  /** The App Server connection generation. A new generation retires this
   * lifetime's observation but never its editing transactions. */
  generation: number;
  /** Native per-scope application publications, exactly as the client mirrors
   * them. A level, never an edge. */
  publications: Record<string, ConfigurationApplication> | undefined;
  /** The latest adopted authoritative projection of the *current* presentation
   * attachment and connection generation. Only authoritative reads reach it: a
   * write acknowledgement is not a projection. This field being present is
   * exactly what "authoritative observation established" means, so it is
   * written only by `adoptProjection` and demoted at every lifetime boundary. */
  observation?: SourceSettings;
  /** The last observation, demoted when the presentation attachment or the
   * connection generation that established it ended. Stale presentation data
   * only: no guard, action or convergence decision reads it. Retaining the last
   * observation for presentation is never declaring it the fresh authority of a
   * newly attached presentation. */
  staleObservation?: SourceSettings;
  /** Owned by the `authority` region alone. A successful read clears only this;
   * it never erases a write, conflict or application failure. */
  readError: string;
  /** Owned by the `mutation` region alone. */
  writeError: string;
  message: string;
  /** The publication version the read in flight was started to observe, so a
   * read that settles below it is reported as measurably stale exactly once
   * instead of driving another read. */
  chasing?: bigint;
  /** The transaction whose definitive commit no authoritative read issued after
   * it has observed yet. This is the post-commit read obligation; it is a
   * different fact from that transaction's own settlement. */
  unobservedCommit?: { identity: string };
  /** The mutation in flight: its owning transaction, the token that identifies
   * it, its non-sensitive revision selector and the exact CAS revision it is
   * fenced on. Deliberately never the authored payload. */
  submission?: { identity: string; token: number; selector: RevisionSelector; expected: string };
  /** One live transaction actor per native semantic unit touched in this
   * lifetime. Owned here, not by the editors that render them. */
  units: Record<string, UnitTransactionRef>;
  nextToken: number;
}

export type SettingsTargetEvent =
  | { type: 'TRANSPORT'; connection: ConnectionState; generation: number; publications: Record<string, ConfigurationApplication> | undefined }
  | { type: 'ATTACH' }
  | { type: 'DETACH' }
  | { type: 'REFRESH' }
  | { type: 'RECONCILE' }
  | { type: 'UNIT.EDIT'; identity: string; selector: RevisionSelector; revision: string; value: unknown }
  | { type: 'UNIT.REVIEW'; identity: string }
  | { type: 'UNIT.DISCARD'; identity: string }
  | { type: 'UNIT.SUBMIT'; identity: string; selector: RevisionSelector; revision: string; mutation: SourceMutation }
  | { type: 'UNIT.RETIRED'; identity: string }
  | { type: 'READ.FORCE' }
  | { type: 'READ.ADOPT'; projection: SourceSettings }
  | { type: 'READ.REREAD_FAILED'; error: unknown }
  | { type: 'WRITE.STARTED' }
  | { type: 'COMMIT.OBSERVED' }
  | { type: 'COMMIT.UNOBSERVED' }
  /** A definitive commit now awaits an authoritative observation. Raised by the
   * `mutation` region at the moment the acknowledgement is recorded, so the
   * `authority` region classifies it from whatever state it is actually in
   * rather than from the state it happened to be in earlier. */
  | { type: 'COMMIT.PENDING' }
  /** This connection generation has been replaced. */
  | { type: 'GENERATION.REPLACED' }
  | { type: 'TRIGGER' };

/** The outstanding native publication obligation of this target's scope.
 *
 * Level, not edge: until this target's own projection carries at least the
 * version published for its scope, the obligation stands — an acknowledgement
 * or an older projection landing in between can neither discharge nor cancel
 * it. With no current projection at all — which is the state every `ATTACH`
 * establishes, because it demotes the previous observation to stale
 * presentation data — any start owes the first authoritative read. */
function publicationObligation(context: SettingsTargetContext): bigint | undefined {
  const held = context.observation;
  if (!held) return 0n;
  const scopeKey = applicationScope(held.target);
  const publication = (context.publications ?? {})[scopeKey];
  if (!publication) return undefined;
  const settled = held.application?.scope === scopeKey ? held.application.version : undefined;
  return settled === undefined || BigInt(settled) < BigInt(publication.version) ? BigInt(publication.version) : undefined;
}

function classifyWriteFailure(cause: unknown): WriteFailureKind {
  if ((cause instanceof RpcFailure && cause.error.data?.kind === 'source_conflict')
    || (cause instanceof WorkspaceHostError && cause.kind === 'source_conflict')) return 'conflict';
  if (isOutcomeUncertain(cause) || (cause instanceof WorkspaceHostError && cause.uncertain)) return 'uncertain';
  return 'rejected';
}

function writeFailureMessage(kind: WriteFailureKind, cause: unknown): string {
  return kind === 'conflict' ? 'Source changed. Your draft and base revision are preserved.'
    : kind === 'uncertain' ? 'Save outcome uncertain. Rereading authority without replaying the write.'
      : String(cause);
}

/** The exact revision one unit's transaction is settled by, from a projection
 * this target just adopted. A projection whose scope view is unavailable
 * settles nothing rather than inventing a revision. */
function unitRevision(projection: SourceSettings, selector: RevisionSelector): string | undefined {
  try { return selectedRevision(projection, selector); } catch { return undefined; }
}

/** The Settings authority of exactly one target.
 *
 * Two facts are genuinely independent and are therefore two regions:
 *
 * - `authority` owns every authoritative read of this target and is the single
 *   convergence owner. Read ordering is structural, not a comparison: starting
 *   a newer read *stops* the older read actor, so an older response can never
 *   be delivered at all, whatever order the network answers in.
 * - `mutation` owns the one source mutation this presentation may have in
 *   flight, and records where it ended up.
 *
 * The lifetimes this actor sits between are explicit, and deliberately nested
 * rather than collapsed:
 *
 * - *App Server authority* — the actor is created per (endpoint, authority
 *   revision, target) and retired when that authority is replaced, so an old
 *   lifetime can never publish into its replacement.
 * - *connection generation* — inside one authority, `authority.attached` is
 *   entered by exactly one generation and re-entered by its replacement. An
 *   observation belongs to the generation that acquired it: once a newer
 *   generation is current, no read, read failure, Workspace-owned reread or
 *   convergence comparison from the older one can become authoritative, because
 *   the state that owned them has been left and their actor stopped.
 * - *transaction* — the per-unit actors in `units` live for the whole authority
 *   lifetime, *across* generation changes, so a definitive acknowledgement
 *   settles the exact transaction that submitted it even after its editor, the
 *   whole Settings dialog, or the connection it was submitted on is gone. What
 *   such a late acknowledgement may never do is publish its generation's
 *   observation into the new one.
 * - *presentation* — `ATTACH` / `DETACH`. A detached presentation reads
 *   nothing; a mutation already in flight still settles. `DETACH` retains
 *   editing transactions; `ATTACH` revalidates authoritative observation: the
 *   previous observation is demoted to stale presentation data, so every new
 *   presentation attachment must establish a fresh authoritative read before
 *   any observation is current again. */
export const settingsTargetMachine = setup({
  types: {
    context: {} as SettingsTargetContext,
    events: {} as SettingsTargetEvent,
    input: {} as {
      target: SettingsTarget; port: ConfigurationPort; connection: ConnectionState;
      generation: number; publications: Record<string, ConfigurationApplication> | undefined;
    },
  },
  actors: {
    readSource: fromPromise(({ input }: { input: { port: ConfigurationPort } }) => input.port.read()),
    writeSource: fromPromise(({ input }: { input: { port: ConfigurationPort; expected: string; mutation: SourceMutation } }) =>
      input.port.write(input.expected, input.mutation)),
    reconcileSource: fromPromise(({ input }: { input: { port: ConfigurationPort } }) => input.port.reconcile()),
    unitTransaction: unitTransactionMachine,
  },
  guards: {
    /** Level-triggered: a publication this projection has not reached, a
     * definitive commit no post-commit read has observed, or no current
     * projection at all — which is how every `ATTACH` establishes the one fresh
     * authoritative read its presentation attachment owes. Never a timer and
     * never a poll. */
    owesRead: ({ context }) => context.connection === 'connected'
      && (context.unobservedCommit !== undefined || publicationObligation(context) !== undefined),
    /** The publication half of the obligation alone — and exactly it. A write
     * holding the read order open is only superseded by a read that a *newer
     * native publication* owes: never by the commit obligation of its own
     * acknowledgement, and never by the bare "no current projection" first-read
     * obligation a reattached presentation carries — both of which the
     * reserved reread is exactly about to answer. */
    owesPublicationRead: ({ context }) => context.connection === 'connected'
      && context.observation !== undefined && publicationObligation(context) !== undefined,
    /** Native authority is the only gate on authoring. The browser may submit
     * only against an observed authoritative projection of a live connection —
     * and only against the observation of the current presentation attachment,
     * never the one demoted to stale presentation data. */
    canSubmit: ({ context }) => context.connection === 'connected' && !context.readError && context.observation !== undefined,
    /** No authoritative read of this target can be made at all right now. The
     * Product Host is an independent transport, so a Workspace write may commit
     * definitively while this connection is down — and then the evidence
     * available for that commit's observation is exactly none. */
    cannotObserve: ({ context }) => context.connection !== 'connected',
    convergenceSatisfied: ({ context }) => publicationObligation(context) === undefined,
    /** A newer publication arrived while the read was in flight: it is still
     * owed and drives exactly one more bounded read. */
    convergenceAdvanced: ({ context }) => {
      const remaining = publicationObligation(context);
      return remaining !== undefined && (context.chasing === undefined || remaining > context.chasing);
    },
    /** The read was issued after the publication and reports no comparable
     * application version at all: missing native evidence, not a reason to spin. */
    applicationUnmeasurable: ({ context }) => {
      const held = context.observation;
      return !held || held.application?.scope !== applicationScope(held.target);
    },
    rereadObserved: ({ event }) => (event as unknown as { output?: WriteOutcome }).output?.reread?.status === 'observed',
    rereadFailed: ({ event }) => (event as unknown as { output?: WriteOutcome }).output?.reread?.status === 'failed',
    /** A Workspace write performs its own authoritative reread, so that write
     * reserves the read order at its initiation. A User write owns no reread. */
    portOwnsReread: ({ context }) => context.port.ownsReread,
    /** The write-owned read reservation is still open: a Workspace mutation is
     * in flight and its own reread will come. A presentation bounce must not
     * steal that reserved read order with a competing read. */
    readReservationOpen: ({ context }) => context.port.ownsReread && context.submission !== undefined,
    generationChanged: ({ context, event }) => event.type === 'TRANSPORT' && event.generation !== context.generation,
    isConflict: ({ event }) => classifyWriteFailure((event as unknown as { error: unknown }).error) === 'conflict',
    isUncertain: ({ event }) => classifyWriteFailure((event as unknown as { error: unknown }).error) === 'uncertain',
  },
  actions: {
    applyTransport: assign({
      connection: ({ context, event }) => event.type === 'TRANSPORT' ? event.connection : context.connection,
      generation: ({ context, event }) => event.type === 'TRANSPORT' ? event.generation : context.generation,
      publications: ({ context, event }) => event.type === 'TRANSPORT' ? event.publications : context.publications,
    }),
    /** A new connection generation retires everything the previous generation
     * observed: its projection, the read failure that answered it, the
     * convergence target it was chasing and the notices it published. They are
     * all scoped to the generation that acquired them.
     *
     * Two things are deliberately not. Editing transactions belong to the
     * authority lifetime, not to the connection. And `unobservedCommit` is the
     * post-commit read obligation of a mutation that already crossed the native
     * submission boundary: clearing it would strand that commit's classification
     * on a lifetime that no longer reads, so the new generation's own
     * authoritative read inherits the obligation and settles it instead. The
     * previous generation's projection is retired outright — not even kept as
     * stale presentation data — because an observation belongs to exactly one
     * connection generation. */
    retireObservation: assign({
      observation: () => undefined, staleObservation: () => undefined,
      readError: () => '', writeError: () => '', message: () => '',
      chasing: () => undefined,
    }),
    /** The observation's authority ends with the presentation attachment that
     * established it. The value is demoted to stale presentation data, not
     * discarded: retaining the last observation for presentation is not
     * declaring it the fresh authority of the next attachment. What `ATTACH`
     * establishes is a fresh authoritative read obligation; what it must never
     * do is discard an editing transaction. */
    demoteObservation: assign({
      observation: () => undefined,
      staleObservation: ({ context }) => context.observation ?? context.staleObservation,
    }),
    recordChasing: assign({ chasing: ({ context }) => publicationObligation(context) }),
    reportStaleApplication: assign({
      writeError: ({ context }) => {
        const held = context.observation!;
        return `Native ${applicationScope(held.target)} published application version ${context.chasing}, but the authoritative read issued after it settled at ${held.application?.version}.`;
      },
    }),
    recordReadFailure: assign({ readError: ({ event }) => String((event as unknown as { error?: unknown }).error) }),
    recordRereadFailure: assign({
      readError: ({ event }) => `Saved, but the authoritative reread failed. Application status is uncertain. ${String((event as unknown as { error: unknown }).error)}`,
    }),
    /** A definitive commit that the authoritative read of this generation cannot
     * observe is exactly "committed, observation uncertain".
     *
     * It is announced from the one state that knows the authoritative read is
     * unavailable, and from that state alone — on entry when the commit was
     * already recorded, and on `COMMIT.PENDING` when it is recorded afterwards.
     * That is what makes the two delivery orders converge: neither the read
     * failure nor the acknowledgement has to arrive first. */
    classifyUnobservedCommit: enqueueActions(({ context, enqueue }) => {
      if (context.unobservedCommit) enqueue.raise({ type: 'COMMIT.UNOBSERVED' });
    }),

    /** The one commit point at which an authoritative projection becomes this
     * presentation's truth.
     *
     * Only an authoritative read reaches here — an ordinary read or the reread
     * a Workspace write owns — and only from a state that still owns the read
     * order. Adopting broadcasts the exact revision each live transaction is
     * settled by, so an acknowledged mutation retires even when the editor, or
     * the whole dialog, that submitted it is gone. Adopting also drops the
     * demoted stale copy: once a fresh observation exists, nothing presents
     * the old one. */
    adoptProjection: enqueueActions(({ context, event, enqueue }) => {
      const projection = (event as unknown as { output?: SourceSettings; projection?: SourceSettings }).output
        ?? (event as unknown as { projection: SourceSettings }).projection;
      enqueue.assign({ observation: () => projection, readError: () => '', staleObservation: () => undefined });
      for (const unit of Object.values(context.units)) {
        const revision = unitRevision(projection, unit.getSnapshot().context.selector);
        if (revision !== undefined) enqueue.sendTo(unit, { type: 'OBSERVED', revision });
      }
      // A read issued after a definitive commit discharges that commit's
      // observation obligation, whatever revision it happens to carry: the
      // transaction's own settlement is the separate fact broadcast above.
      if (context.unobservedCommit) {
        enqueue.assign({ unobservedCommit: () => undefined });
        enqueue.raise({ type: 'COMMIT.OBSERVED' });
      }
    }),

    ensureUnit: enqueueActions(({ context, event, enqueue }) => {
      const request = event as Extract<SettingsTargetEvent, { type: 'UNIT.EDIT' | 'UNIT.SUBMIT' }>;
      if (context.units[request.identity]) return;
      enqueue.assign({
        units: ({ context: current, spawn }) => ({
          ...current.units,
          [request.identity]: spawn('unitTransaction', {
            id: `unit:${request.identity}`,
            input: { identity: request.identity, selector: request.selector, revision: request.revision },
          }),
        }),
      });
    }),
    forwardEdit: enqueueActions(({ context, event, enqueue }) => {
      const request = event as Extract<SettingsTargetEvent, { type: 'UNIT.EDIT' }>;
      const unit = context.units[request.identity];
      if (unit) enqueue.sendTo(unit, { type: 'EDIT', value: request.value });
    }),
    forwardReview: enqueueActions(({ context, event, enqueue }) => {
      const unit = context.units[(event as Extract<SettingsTargetEvent, { type: 'UNIT.REVIEW' }>).identity];
      if (unit) enqueue.sendTo(unit, { type: 'REVIEW' });
    }),
    forwardDiscard: enqueueActions(({ context, event, enqueue }) => {
      const unit = context.units[(event as Extract<SettingsTargetEvent, { type: 'UNIT.DISCARD' }>).identity];
      if (unit) enqueue.sendTo(unit, { type: 'DISCARD' });
    }),
    /** A transaction that has nothing left to own retires exactly once. */
    retireUnit: enqueueActions(({ context, event, enqueue }) => {
      const identity = (event as Extract<SettingsTargetEvent, { type: 'UNIT.RETIRED' }>).identity;
      const unit = context.units[identity];
      if (!unit) return;
      enqueue.stopChild(unit);
      enqueue.assign({
        units: ({ context: current }) => Object.fromEntries(Object.entries(current.units).filter(([key]) => key !== identity)),
      });
    }),

    /** Open one submission against the exact CAS revision its transaction has
     * pinned, and hand the transaction the token that identifies it. */
    openSubmission: enqueueActions(({ context, event, enqueue }) => {
      const request = event as Extract<SettingsTargetEvent, { type: 'UNIT.SUBMIT' }>;
      const unit = context.units[request.identity]!;
      const token = context.nextToken;
      enqueue.assign({
        nextToken: () => token + 1,
        writeError: () => '',
        message: () => '',
        submission: () => ({ identity: request.identity, token, selector: request.selector, expected: unit.getSnapshot().context.base }),
      });
      enqueue.sendTo(unit, { type: 'SUBMIT', token });
      enqueue.raise({ type: 'WRITE.STARTED' });
    }),
    /** The definitive commit point.
     *
     * The acknowledgement confirms exactly this mutation and supplies its
     * committed revision. It is recorded on the transaction that submitted it
     * before anything interprets the reread, and it stays a committed fact even
     * if that reread fails, if this presentation has since been detached, or if
     * the whole Settings dialog was closed while it was in flight. */
    recordAcknowledgement: enqueueActions(({ context, event, enqueue }) => {
      const outcome = (event as unknown as { output: WriteOutcome }).output;
      const submission = context.submission!;
      const revision = unitRevision(outcome.acknowledgement, submission.selector);
      const unit = context.units[submission.identity];
      if (unit && revision !== undefined) enqueue.sendTo(unit, { type: 'COMMITTED', token: submission.token, revision });
      enqueue.assign({
        unobservedCommit: () => ({ identity: submission.identity }),
        submission: () => undefined,
      });
    }),
    /** The saved notice is truthful against the projection the presentation is
     * actually showing, so it is reported when the post-commit observation
     * settles — either because an authoritative read carried it, or because the
     * read that owed it failed and the observation is explicitly uncertain. The
     * commit itself was definitive either way. */
    reportSaved: assign({ message: () => 'Source saved. Native coordination owns application.' }),
    adoptOwnedReread: raise(({ event }) => ({
      type: 'READ.ADOPT' as const,
      projection: ((event as unknown as { output: WriteOutcome }).output.reread as { status: 'observed'; projection: SourceSettings }).projection,
    })),
    reportOwnedRereadFailure: raise(({ event }) => ({
      type: 'READ.REREAD_FAILED' as const,
      error: ((event as unknown as { output: WriteOutcome }).output.reread as { status: 'failed'; error: unknown }).error,
    })),
    /** The write did not commit. The transaction keeps its browser intent and
     * its exact reviewed base; the mutation is never replayed. */
    recordWriteFailure: enqueueActions(({ context, event, enqueue }) => {
      const cause = (event as unknown as { error: unknown }).error;
      const submission = context.submission!;
      const unit = context.units[submission.identity];
      if (unit) enqueue.sendTo(unit, { type: 'FAILED', token: submission.token });
      enqueue.assign({
        writeError: () => writeFailureMessage(classifyWriteFailure(cause), cause),
        submission: () => undefined,
      });
    }),
  },
}).createMachine({
  id: 'settingsTarget',
  context: ({ input }) => ({
    target: input.target,
    port: input.port,
    connection: input.connection,
    generation: input.generation,
    publications: input.publications,
    readError: '',
    writeError: '',
    message: '',
    units: {},
    nextToken: 1,
  }),
  type: 'parallel',
  states: {
    authority: {
      initial: 'suspended',
      on: {
        // Every `ATTACH` is one new presentation attachment and owes exactly
        // one fresh authoritative observation. It demotes the previous
        // observation to stale presentation data, and the transition re-enters
        // `attached` from whatever state is active, so the standing "no current
        // projection" obligation starts the validation read through the single
        // read owner, coalesced with any publication or commit obligation
        // already outstanding. The one reattachment that must not start a
        // competing read is one that lands in the middle of a Workspace write:
        // that write reserved the read order at its initiation, and the
        // reservation — like the mutation and its settlement — outlives the
        // presentation.
        ATTACH: [
          { guard: 'readReservationOpen', target: '.attached.awaitingWrite', actions: 'demoteObservation' },
          { target: '.attached', actions: 'demoteObservation' },
        ],
        DETACH: { target: '.suspended', actions: 'demoteObservation' },
      },
      states: {
        /** No presentation is attached. A detached lifetime reads nothing — it
         * neither polls nor keeps a background read alive — while a mutation
         * already in flight still settles. Editing transactions, dirty drafts,
         * pinned CAS bases and submitted mutation state are all retained. */
        suspended: {},

        /** The observation lifetime of exactly one connection generation.
         *
         * Every authoritative read of this target, every read failure, every
         * convergence decision and the projection they produce live inside this
         * state, and the state belongs to the generation that entered it. A new
         * generation re-enters it, which stops the read actor invoked under the
         * old generation and abandons the state that generation had reached. An
         * old read therefore has no completion path, an old read failure has
         * nothing to populate, and an old Workspace-owned reread arrives at a
         * state that does not accept it — structurally, with no comparison
         * anywhere. */
        attached: {
          initial: 'idle',
          on: { 'GENERATION.REPLACED': { target: 'attached', reenter: true } },
          states: {
            idle: {
              always: [
                { guard: 'owesRead', target: 'reading' },
                // Nothing can be read at all: that is the unobservable state,
                // not an idle one, and a definitive commit is classified there.
                { guard: 'cannotObserve', target: 'blocked' },
              ],
              on: {
                REFRESH: 'reading',
                'READ.FORCE': 'reading',
                'WRITE.STARTED': { guard: 'portOwnsReread', target: 'awaitingWrite' },
              },
            },
            /** Exactly one authoritative read is in flight. Starting another
             * read re-enters this state, which stops the older read actor: a
             * superseded read is cancelled, not compared, so it can never
             * publish a projection, a read failure or a commit observation. */
            reading: {
              entry: 'recordChasing',
              invoke: {
                src: 'readSource',
                input: ({ context }) => ({ port: context.port }),
                onDone: { target: 'settling', actions: 'adoptProjection' },
                onError: { target: 'blocked', actions: 'recordReadFailure' },
              },
              on: {
                REFRESH: { target: 'reading', reenter: true },
                'READ.FORCE': { target: 'reading', reenter: true },
                'WRITE.STARTED': { guard: 'portOwnsReread', target: 'awaitingWrite' },
                // A reread that no longer owns the read order is silent, in both
                // outcomes: the newer read in flight owns this presentation, and
                // the commit's own observation obligation stays with it.
              },
            },
            /** The reread a Workspace write owns is reserved here, at the moment
             * that write is initiated. Any authoritative read started afterwards
             * leaves this state, and the Host's reread is then silently
             * superseded however late it arrives — it neither replaces a newer
             * projection nor publishes a failure the newer read has retired.
             * The reservation survives `DETACH` / `ATTACH`: a reattachment while
             * the write is in flight rejoins this state, and the write's own
             * reread becomes the fresh observation of the new attachment. */
            awaitingWrite: {
              always: { guard: 'owesPublicationRead', target: 'reading' },
              on: {
                'READ.ADOPT': { target: 'settling', actions: 'adoptProjection' },
                'READ.REREAD_FAILED': { target: 'blocked', actions: 'recordRereadFailure' },
                REFRESH: 'reading',
                'READ.FORCE': 'reading',
              },
            },
            /** The convergence decision point: does the projection just adopted
             * discharge the obligation that started this read? */
            settling: {
              always: [
                { guard: 'convergenceSatisfied', target: 'idle' },
                { guard: 'convergenceAdvanced', target: 'idle' },
                { guard: 'applicationUnmeasurable', target: 'blocked' },
                { target: 'blocked', actions: 'reportStaleApplication' },
              ],
            },
            /** No authoritative observation is available: a read failed, native
             * reported a measurably stale application, or this connection
             * cannot read at all. The obligation stands, but nothing retries on
             * its own: a later publication, reconnect, reattach or explicit
             * refresh drives it.
             *
             * This is also the one state that knows an authoritative
             * observation is currently unavailable, so it is where a definitive
             * commit is classified as committed-but-unobserved — on entry if the
             * commit is already recorded, and on `COMMIT.PENDING` if it is
             * recorded while this state is already active. */
            blocked: {
              entry: 'classifyUnobservedCommit',
              on: {
                'COMMIT.PENDING': { actions: 'classifyUnobservedCommit' },
                TRIGGER: 'idle',
                REFRESH: 'reading',
                'READ.FORCE': 'reading',
              },
            },
          },
        },
      },
    },

    /** The one source mutation this presentation may have in flight, and where
     * it ended up. Acknowledgement, authoritative observation, conflict,
     * native rejection and an unknown outcome are five separate facts. */
    mutation: {
      initial: 'idle',
      states: {
        idle: { on: { 'UNIT.SUBMIT': { guard: 'canSubmit', target: 'submitting', actions: ['ensureUnit', 'openSubmission'] } } },
        submitting: {
          invoke: {
            src: 'writeSource',
            input: ({ context, event }) => ({
              port: context.port,
              expected: context.submission!.expected,
              mutation: (event as Extract<SettingsTargetEvent, { type: 'UNIT.SUBMIT' }>).mutation,
            }),
            // A write that owns a reread offers it to the authority region and
            // then announces the commit. The reread is only *offered*: the
            // region adopts it if it still owns the read order and ignores it
            // otherwise, and `COMMIT.PENDING` is what makes the commit reach a
            // terminal classification in the second case. A write that owns no
            // reread forces its own post-commit read instead, which is itself
            // the classification path, so it announces nothing.
            onDone: [
              { guard: 'rereadObserved', target: 'observing', actions: ['recordAcknowledgement', 'adoptOwnedReread', raise({ type: 'COMMIT.PENDING' })] },
              { guard: 'rereadFailed', target: 'observing', actions: ['recordAcknowledgement', 'reportOwnedRereadFailure', raise({ type: 'COMMIT.PENDING' })] },
              { target: 'observing', actions: ['recordAcknowledgement', raise({ type: 'READ.FORCE' })] },
            ],
            onError: [
              { guard: 'isConflict', target: 'conflicted', actions: ['recordWriteFailure', raise({ type: 'READ.FORCE' })] },
              { guard: 'isUncertain', target: 'uncertain', actions: ['recordWriteFailure', raise({ type: 'READ.FORCE' })] },
              { target: 'rejected', actions: ['recordWriteFailure', raise({ type: 'READ.FORCE' })] },
            ],
          },
        },
        /** The native write is definitive. What is still open is only the
         * authoritative projection and the native application that follow it —
         * never whether the write committed. */
        observing: {
          on: {
            'COMMIT.OBSERVED': { target: 'idle', actions: 'reportSaved' },
            'COMMIT.UNOBSERVED': { target: 'unobserved', actions: 'reportSaved' },
            'UNIT.SUBMIT': { guard: 'canSubmit', target: 'submitting', actions: ['ensureUnit', 'openSubmission'] },
          },
        },
        /** Committed, and the authoritative observation that would follow it is
         * explicitly unavailable. Never "unsaved", and never "applied". */
        unobserved: {
          on: {
            'COMMIT.OBSERVED': 'idle',
            'UNIT.SUBMIT': { guard: 'canSubmit', target: 'submitting', actions: ['ensureUnit', 'openSubmission'] },
          },
        },
        conflicted: { on: { 'UNIT.SUBMIT': { guard: 'canSubmit', target: 'submitting', actions: ['ensureUnit', 'openSubmission'] } } },
        rejected: { on: { 'UNIT.SUBMIT': { guard: 'canSubmit', target: 'submitting', actions: ['ensureUnit', 'openSubmission'] } } },
        uncertain: { on: { 'UNIT.SUBMIT': { guard: 'canSubmit', target: 'submitting', actions: ['ensureUnit', 'openSubmission'] } } },
      },
    },

    /** Explicit native rescan. It re-derives native state and is not a
     * projection, so the caller still owes an authoritative read afterwards. */
    maintenance: {
      initial: 'idle',
      states: {
        idle: { on: { RECONCILE: 'reconciling' } },
        reconciling: {
          invoke: {
            src: 'reconcileSource',
            input: ({ context }) => ({ port: context.port }),
            onDone: { target: 'idle', actions: raise({ type: 'READ.FORCE' }) },
            onError: { target: 'idle', actions: assign({ writeError: ({ event }) => String(event.error) }) },
          },
        },
      },
    },
  },
  on: {
    TRANSPORT: [
      // A replaced connection generation is not a trigger to re-read the old
      // observation lifetime: it ends that lifetime and starts a new one, which
      // obtains its own authoritative observation from nothing.
      { guard: 'generationChanged', actions: ['applyTransport', 'retireObservation', raise({ type: 'GENERATION.REPLACED' })] },
      { actions: ['applyTransport', raise({ type: 'TRIGGER' })] },
    ],
    'UNIT.EDIT': { actions: ['ensureUnit', 'forwardEdit'] },
    'UNIT.REVIEW': { actions: 'forwardReview' },
    'UNIT.DISCARD': { actions: 'forwardDiscard' },
    'UNIT.RETIRED': { actions: 'retireUnit' },
  },
});
