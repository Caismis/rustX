import { assign, enqueueActions, fromPromise, raise, setup, stateIn, type ActorRefFrom, type SnapshotFrom } from 'xstate';
import type { ConfigurationApplication, SourceMutation, SourceSettings } from '../../../../../protocol/app-server/v20';
import { isOutcomeUncertain, RpcFailure, type ConnectionState } from '../../../client/app-server';
import { WorkspaceHostError } from '../../../workspaces/host';
import { applicationScope, selectedRevision, type RevisionSelector, type SettingsTarget } from '../projection';
import type { ConfigurationPort, WriteOutcome } from './port';
import { unitTransactionMachine } from './unit-transaction';

export type UnitTransactionRef = ActorRefFrom<typeof unitTransactionMachine>;

/** Why the last submitted mutation did not commit. Conflict, native rejection
 * and an unknown outcome are three different facts and are never collapsed. */
type WriteFailureKind = 'conflict' | 'rejected' | 'uncertain';

/** Where this target's last submitted mutation stands, projected from the
 * `mutation` region alone. It is a fact about the mutation, so neither a
 * connection generation replacement nor a successful read changes it; only a
 * new submission does. */
export type MutationOutcome =
  | { kind: 'none' }
  | { kind: 'submitting' }
  /** Definitively committed; its authoritative observation is still owed. */
  | { kind: 'committed' }
  /** Committed, and the observation that followed it is `observed` or
   * explicitly unavailable. Never "unsaved" and never "applied". */
  | { kind: 'saved'; observed: boolean }
  | { kind: 'conflict' }
  | { kind: 'rejected'; detail: string }
  | { kind: 'uncertain' };

export interface SettingsTargetContext {
  /** The one owner this actor is bound to for its whole lifetime. Session focus
   * never retargets it; only authority replacement retires it. */
  readonly target: SettingsTarget;
  /** The native authority of this exact target, fixed for the actor's life:
   * endpoint, target and Product Host access are all part of its identity. */
  readonly port: ConfigurationPort;
  connection: ConnectionState;
  /** The App Server connection generation. A new generation retires this
   * lifetime's observation but never its editing transactions or the outcome
   * of a mutation already submitted. */
  generation: number;
  /** The native application publication of exactly this target's source scope,
   * as its `ConfigurationPort` projects it from the client's publications. A
   * level, never an edge. No other scope's publication — a Session, another
   * Workspace, anything else native publishes — ever reaches this actor, so none
   * can retry, refresh, unblock or advance its reads. */
  publication: ConfigurationApplication | undefined;
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
  /** Owned by the `authority` region alone and scoped to the connection
   * generation that observed it. A successful read clears only this and the
   * convergence report it re-answers; it never erases a mutation outcome. */
  readError: string;
  /** Owned by the `authority` region: the authoritative read issued after a
   * native publication settled measurably below it. An observation fact of one
   * connection generation, re-answered by every adopted read. */
  convergenceError: string;
  /** Owned by the `mutation` region: native's rejection of the last submitted
   * mutation, present exactly while `mutation.rejected` is active. Conflict and
   * unknown outcome carry no detail beyond their state. The outcome belongs to
   * the mutation, not to the connection generation it was observed on, so only
   * a new submission, or discarding the intent it rejected, replaces it. */
  rejection?: string;
  /** Owned by the `maintenance` region: why the last explicit rescan failed. */
  maintenanceError: string;
  /** The publication version the read in flight was started to observe, so a
   * read that settles below it is reported as measurably stale exactly once
   * instead of driving another read. */
  chasing?: bigint;
  /** The last source mutation that crossed the native submission boundary and
   * settled — definitively committed, conflicted, rejected or with an unknown
   * outcome — while no authoritative read issued after that settlement has been
   * adopted yet. For a commit it carries the revision that commit produced.
   *
   * This is the post-settlement read obligation, and it is a different fact
   * from the settled transaction's own outcome. A read discharges it when it was
   * issued after the settlement, or — for a commit, whenever it was issued —
   * when it already carries exactly the committed revision, which is a direct
   * observation of the commit. While it stands, the observation this target
   * holds is known to predate the settlement, so it admits no new source
   * mutation (see `admitsSourceMutation`). */
  unobservedSettlement?: { identity: string; selector: RevisionSelector; committed: boolean; revision?: string };
  /** The mutation in flight: its owning transaction, the token that identifies
   * it, its non-sensitive revision selector and the exact CAS revision it is
   * fenced on. Deliberately never the authored payload. */
  submission?: { identity: string; token: number; selector: RevisionSelector; expected: string };
  /** The unit whose mutation the `mutation` region's current outcome is
   * about. Set by the submission that produces the outcome, so discarding that
   * unit's browser intent can retire a failed outcome that describes it. */
  outcomeUnit?: string;
  /** The read order a Workspace write reserved for its own authoritative
   * reread, named by the token of the submission that reserved it.
   *
   * This is observation publication authority, and it is a different fact from
   * `submission`, the write transaction that will eventually deliver the
   * reread. The transaction may outlive its connection generation and any
   * number of presentation attachments, and still settles. The reservation may
   * not: it is taken only when the `authority` region enters `awaitingWrite`
   * for that submission, and it is revoked by the first of a newer
   * authoritative read starting, its connection generation being replaced, or
   * the write ending. Revocation is permanent — nothing but a new submission
   * ever takes a reservation — so no later `ATTACH` can restore publication
   * authority that a replacement or a newer read took away. `DETACH` alone
   * neither takes nor revokes it.
   *
   * The reservation also records the native publication watermark it was
   * established against: the version published for this target's own source
   * scope at that moment. The
   * Host's reread is issued after the write commits, so it answers every
   * publication up to that watermark and no newer one. Whether a later
   * publication supersedes the reservation is decided against this watermark
   * alone — never against the current presentation observation, which every
   * `ATTACH` demotes. */
  rereadReservation?: { token: number; publication?: bigint };
  /** One live transaction actor per native semantic unit whose transaction
   * still owns something: browser intent, a pinned CAS base or a definitive
   * commit whose observation is owed. Owned here, not by the editors that
   * render them. A transaction that owns nothing announces its own retirement
   * and is stopped and removed at once, so touched units do not accumulate
   * over a long-lived target; a later edit starts a fresh transaction from the
   * revision the editor then presents. */
  units: Record<string, UnitTransactionRef>;
  nextToken: number;
}

export type SettingsTargetEvent =
  | { type: 'TRANSPORT'; connection: ConnectionState; generation: number; publication: ConfigurationApplication | undefined }
  | { type: 'ATTACH' }
  | { type: 'DETACH' }
  | { type: 'REFRESH' }
  | { type: 'RECONCILE' }
  | { type: 'UNIT.EDIT'; identity: string; selector: RevisionSelector; revision: string; value: unknown }
  | { type: 'UNIT.REVIEW'; identity: string }
  | { type: 'UNIT.DISCARD'; identity: string }
  | { type: 'UNIT.SUBMIT'; identity: string; selector: RevisionSelector; revision: string; mutation: SourceMutation }
  | { type: 'UNIT.RETIRED'; identity: string }
  /** Raised once `UNIT.DISCARD` has been forwarded to the unit, so the
   * `mutation` region answers the same gesture from whatever state it is in. */
  | { type: 'INTENT.DISCARDED'; identity: string }
  | { type: 'READ.FORCE' }
  | { type: 'READ.ADOPT'; projection: SourceSettings }
  | { type: 'READ.REREAD_FAILED'; error: unknown }
  | { type: 'WRITE.STARTED' }
  /** An authoritative read issued after the last definitive commit observed it. */
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
  const publication = context.publication;
  if (!publication) return undefined;
  const settled = held.application?.scope === publication.scope ? held.application.version : undefined;
  return settled === undefined || BigInt(settled) < BigInt(publication.version) ? BigInt(publication.version) : undefined;
}

/** The version native currently publishes for this target's source scope. */
function publishedVersion(context: SettingsTargetContext): bigint | undefined {
  return context.publication ? BigInt(context.publication.version) : undefined;
}

/** Whether two target-local publication levels are the same native fact. A
 * publication is identified by its scope and its version, never by the
 * identity of the object or of the map that carried it. */
function samePublication(a: ConfigurationApplication | undefined, b: ConfigurationApplication | undefined): boolean {
  return a?.scope === b?.scope && a?.version === b?.version;
}

type SubmissionAdmission = Pick<SettingsTargetContext, 'connection' | 'readError' | 'observation' | 'submission' | 'unobservedSettlement'>;

/** Whether this target admits a new source mutation now. This is the one
 * target-wide admission fact: every semantic unit's submission is decided by
 * it, the `UNIT.SUBMIT` guard and the presentation alike.
 *
 * A new source mutation may start only against a current authoritative
 * observation of a live connection that no read failure has invalidated, and
 * only once no earlier source mutation still owns the mutation / observation
 * barrier: none is natively submitting (`submission`), and none has settled
 * without an authoritative read issued after its settlement having been
 * adopted (`unobservedSettlement`). A unit refused here keeps its draft; the
 * browser neither queues nor replays its mutation. */
export function admitsSourceMutation(context: SubmissionAdmission): boolean {
  return context.connection === 'connected' && !context.readError && context.observation !== undefined
    && context.submission === undefined && context.unobservedSettlement === undefined;
}

function classifyWriteFailure(cause: unknown): WriteFailureKind {
  if ((cause instanceof RpcFailure && cause.error.data?.kind === 'source_conflict')
    || (cause instanceof WorkspaceHostError && cause.kind === 'source_conflict')) return 'conflict';
  if (isOutcomeUncertain(cause) || (cause instanceof WorkspaceHostError && cause.uncertain)) return 'uncertain';
  return 'rejected';
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
 *   lifetime can never publish into its replacement. A retired actor outlives
 *   its authority only while `mutationInFlight` holds.
 * - *connection generation* — inside one authority, `authority.attached` is
 *   entered by exactly one generation and re-entered by its replacement. An
 *   observation belongs to the generation that acquired it: once a newer
 *   generation is current, no read, read failure, Workspace-owned reread or
 *   convergence comparison from the older one can become authoritative, because
 *   the state that owned them has been left, their actor stopped and the
 *   write-owned reread reservation revoked — attached or not, and for good.
 * - *transaction* — the per-unit actors in `units` live until they own nothing,
 *   bounded by the authority lifetime and *across* generation changes, so a
 *   definitive acknowledgement
 *   settles the exact transaction that submitted it even after its editor, the
 *   whole Settings dialog, or the connection it was submitted on is gone. What
 *   such a late acknowledgement may never do is publish its generation's
 *   observation into the new one: the pending write (`submission`) and the
 *   authority of its reread to publish (`rereadReservation`) are separate
 *   facts, and only the first outlives the generation.
 * - *presentation* — `ATTACH` / `DETACH`. A detached presentation reads
 *   nothing; a mutation already in flight still settles. `DETACH` retains
 *   editing transactions; `ATTACH` revalidates authoritative observation: the
 *   previous observation is demoted to stale presentation data, so every new
 *   presentation attachment must establish a fresh authoritative read — its
 *   own, or a still-reserved write-owned reread — before any observation is
 *   current again. */
export const settingsTargetMachine = setup({
  types: {
    context: {} as SettingsTargetContext,
    events: {} as SettingsTargetEvent,
    input: {} as {
      target: SettingsTarget; port: ConfigurationPort; connection: ConnectionState;
      generation: number; publication: ConfigurationApplication | undefined;
    },
    tags: {} as 'mutationInFlight',
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
     * settled mutation no post-settlement read has observed, or no current
     * projection at all — which is how every `ATTACH` establishes the one fresh
     * authoritative read its presentation attachment owes. Never a timer and
     * never a poll. */
    owesRead: ({ context }) => context.connection === 'connected'
      && (context.unobservedSettlement !== undefined || publicationObligation(context) !== undefined),
    /** Native published a version of this target's scope newer than the
     * watermark the reservation was established against. This is the one
     * publication fact that supersedes a write-owned reread — and it is a fact
     * about native publication progress, so it holds whether or not a current
     * presentation observation exists. What never supersedes the reservation is
     * the commit obligation of its own acknowledgement, or the bare "no current
     * projection" first-read obligation a reattached presentation carries: the
     * reserved reread is exactly about to answer both. */
    publicationSupersedesReservation: ({ context }) => {
      const reservation = context.rereadReservation;
      if (context.connection !== 'connected' || !reservation) return false;
      const published = publishedVersion(context);
      return published !== undefined && (reservation.publication === undefined || published > reservation.publication);
    },
    /** The one target-wide source mutation admission fact. */
    admitsSourceMutation: ({ context }) => admitsSourceMutation(context),
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
    /** The write performed its own authoritative reread. Whether that reread
     * may publish is decided by the reservation, not by its presence. */
    carriesOwnedReread: ({ event }) => (event as unknown as { output?: WriteOutcome }).output?.reread !== undefined,
    /** A Workspace write performs its own authoritative reread, so that write
     * reserves the read order at its initiation. A User write owns no reread. */
    portOwnsReread: ({ context }) => context.port.ownsReread,
    /** The write-owned reread still holds publication authority. A pending
     * write alone is never enough: its reservation may already be revoked. */
    holdsRereadReservation: ({ context }) => context.rereadReservation !== undefined,
    generationChanged: ({ context, event }) => event.type === 'TRANSPORT' && event.generation !== context.generation,
    /** Inside one generation, the connection state or this target's own
     * publication level changed. The configuration system delivers the
     * transport at every client publication, and one that changes neither is
     * not an observation trigger: it must never retry a read that already
     * failed. */
    transportChanged: ({ context, event }) => event.type === 'TRANSPORT'
      && (event.connection !== context.connection || !samePublication(event.publication, context.publication)),
    isConflict: ({ event }) => classifyWriteFailure((event as unknown as { error: unknown }).error) === 'conflict',
    /** The read being adopted was issued before the definitive commit recorded
     * since, so it may have been served before that commit landed natively. */
    readPredatesCommit: stateIn({ authority: { attached: { reading: 'predatesCommit' } } }),
    isUncertain: ({ event }) => classifyWriteFailure((event as unknown as { error: unknown }).error) === 'uncertain',
    /** The discarded unit is the one whose mutation the current outcome is about. */
    discardsOutcomeUnit: ({ context, event }) => event.type === 'INTENT.DISCARDED' && event.identity === context.outcomeUnit,
  },
  actions: {
    applyTransport: assign({
      connection: ({ context, event }) => event.type === 'TRANSPORT' ? event.connection : context.connection,
      generation: ({ context, event }) => event.type === 'TRANSPORT' ? event.generation : context.generation,
      publication: ({ context, event }) => event.type === 'TRANSPORT' ? event.publication : context.publication,
    }),
    /** A new connection generation retires everything the previous generation
     * observed, and nothing else: its projection, the read failure that
     * answered it, the convergence report it produced and the convergence
     * target it was chasing. They are all scoped to the generation that
     * acquired them, and all owned by the `authority` region.
     *
     * Everything the `mutation` region and the transactions own is deliberately
     * not generation-scoped. Editing transactions, dirty intents and pinned CAS
     * bases belong to the authority lifetime. A mutation outcome — conflict,
     * rejection, unknown outcome, definitive commit — belongs to the mutation,
     * and must read the same whether it arrived before or after the
     * replacement. And `unobservedSettlement` is the post-settlement read
     * obligation of a mutation that already crossed the native submission
     * boundary: clearing it would strand that commit's classification — and the
     * submission barrier it holds — on a lifetime that no longer reads, so the
     * new generation's own authoritative read inherits the obligation and
     * discharges it instead. The previous generation's projection
     * is retired outright — not even kept as stale presentation data — because
     * an observation belongs to exactly one connection generation. */
    retireObservation: assign({
      observation: () => undefined, staleObservation: () => undefined,
      readError: () => '', convergenceError: () => '',
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
    /** The Workspace write just initiated reserves the read order for its own
     * reread, against this target's publication watermark at that moment. */
    reserveOwnedReread: assign({
      rereadReservation: ({ context }) => ({ token: context.submission!.token, publication: publishedVersion(context) }),
    }),
    /** Permanently end the write-owned reread's publication authority. The
     * write itself is untouched and still settles. */
    revokeRereadReservation: assign({ rereadReservation: () => undefined }),
    reportStaleApplication: assign({
      convergenceError: ({ context }) => {
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
      if (context.unobservedSettlement?.committed) enqueue.raise({ type: 'COMMIT.UNOBSERVED' });
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
     * the old one.
     *
     * `postCommit` states whether the read was issued after every mutation
     * settlement this target has recorded. Only such a read is the
     * post-settlement observation a settlement awaits: it discharges the
     * observation obligation — and with it the submission barrier — and it
     * tells each transaction that a revision other than its committed one is a
     * real divergence. A read issued before the latest commit is still this
     * target's authoritative observation; unless it already carries the
     * committed revision it is evidence about the source before that commit, so
     * it discharges nothing and the standing obligation drives the post-commit
     * read. A failed mutation's settlement forces a new read at once, which
     * stops every read issued before it, so no read that predates a failure
     * can be adopted after it. */
    adoptProjection: enqueueActions(({ context, event, enqueue }, params: { postCommit: boolean }) => {
      const projection = (event as unknown as { output?: SourceSettings; projection?: SourceSettings }).output
        ?? (event as unknown as { projection: SourceSettings }).projection;
      enqueue.assign({ observation: () => projection, readError: () => '', convergenceError: () => '', staleObservation: () => undefined });
      for (const unit of Object.values(context.units)) {
        const revision = unitRevision(projection, unit.getSnapshot().context.selector);
        if (revision !== undefined) enqueue.sendTo(unit, { type: 'OBSERVED', revision, postCommit: params.postCommit });
      }
      // A read issued after a settlement discharges that settlement's
      // observation obligation, whatever revision it happens to carry: the
      // transaction's own settlement is the separate fact broadcast above.
      const settlement = context.unobservedSettlement;
      if (settlement && (params.postCommit || (settlement.revision !== undefined && unitRevision(projection, settlement.selector) === settlement.revision))) {
        enqueue.assign({ unobservedSettlement: () => undefined });
        if (settlement.committed) enqueue.raise({ type: 'COMMIT.OBSERVED' });
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
    /** A transaction that has nothing left to own announces it from its
     * terminal `lifetime.retired` state, exactly once; it is stopped and
     * removed here. */
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
      // A new submission is the one event that replaces the previous
      // mutation's outcome.
      enqueue.assign({
        nextToken: () => token + 1,
        rejection: () => undefined,
        outcomeUnit: () => request.identity,
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
        unobservedSettlement: () => ({ identity: submission.identity, selector: submission.selector, committed: true, revision }),
        submission: () => undefined,
        rereadReservation: () => undefined,
      });
    }),
    /** Publish the write's own reread — in either outcome — only if this exact
     * submission still holds the reservation it took. A revoked reservation
     * publishes nothing, however the reread turned out: its projection is not
     * current observation and its failure is not the current read failure. */
    offerOwnedReread: enqueueActions(({ context, event, enqueue }) => {
      const { reread } = (event as unknown as { output: WriteOutcome }).output;
      if (!reread || context.rereadReservation === undefined || context.rereadReservation.token !== context.submission?.token) return;
      enqueue.raise(reread.status === 'observed'
        ? { type: 'READ.ADOPT', projection: reread.projection }
        : { type: 'READ.REREAD_FAILED', error: reread.error });
    }),
    /** The write ended without a definitive commit. The transaction keeps its
     * browser intent and its exact reviewed base; the mutation is never
     * replayed. Which of conflict, rejection or unknown outcome it was is the
     * state the `mutation` region enters. The source may have moved — or, for
     * an unknown outcome, this very mutation may have committed — so the
     * settlement still owes the authoritative read its transition forces. */
    recordWriteFailure: enqueueActions(({ context, enqueue }) => {
      const submission = context.submission!;
      const unit = context.units[submission.identity];
      if (unit) enqueue.sendTo(unit, { type: 'FAILED', token: submission.token });
      enqueue.assign({
        submission: () => undefined,
        rereadReservation: () => undefined,
        unobservedSettlement: () => ({ identity: submission.identity, selector: submission.selector, committed: false }),
      });
    }),
    recordRejection: assign({ rejection: ({ event }) => String((event as unknown as { error: unknown }).error) }),
    forgetOutcome: assign({ rejection: () => undefined, outcomeUnit: () => undefined }),
  },
}).createMachine({
  id: 'settingsTarget',
  context: ({ input }) => ({
    target: input.target,
    port: input.port,
    connection: input.connection,
    generation: input.generation,
    publication: input.publication,
    readError: '',
    convergenceError: '',
    maintenanceError: '',
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
        // competing read is one whose Workspace write still holds the read
        // order it reserved: `DETACH` does not revoke a reservation, so a
        // presentation bounce resumes it. A write that is merely still pending
        // is not enough — a replaced generation or a newer read has revoked
        // its reservation for good, and the new attachment reads for itself.
        ATTACH: [
          { guard: 'holdsRereadReservation', target: '.attached.awaitingWrite', actions: 'demoteObservation' },
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
                'WRITE.STARTED': { guard: 'portOwnsReread', target: 'awaitingWrite', actions: 'reserveOwnedReread' },
              },
            },
            /** Exactly one authoritative read is in flight. Starting another
             * read re-enters this state, which stops the older read actor: a
             * superseded read is cancelled, not compared, so it can never
             * publish a projection, a read failure or a commit observation.
             * Starting a read likewise revokes any write-owned reread
             * reservation: the newer read owns the order from here on.
             *
             * Whether the read in flight postdates every recorded commit is
             * the child state, not a comparison: a read is issued `current`,
             * and a definitive commit recorded while it is outstanding makes it
             * `predatesCommit`. Such a read is still adopted, and still fails
             * into `blocked` like any other — which is what lets its failure
             * and the acknowledgement arrive in either order — but it is never
             * the post-commit observation of that commit. */
            reading: {
              entry: ['revokeRereadReservation', 'recordChasing'],
              invoke: {
                src: 'readSource',
                input: ({ context }) => ({ port: context.port }),
                onDone: [
                  { guard: 'readPredatesCommit', target: 'settling', actions: { type: 'adoptProjection', params: { postCommit: false } } },
                  { target: 'settling', actions: { type: 'adoptProjection', params: { postCommit: true } } },
                ],
                onError: { target: 'blocked', actions: 'recordReadFailure' },
              },
              initial: 'current',
              states: {
                current: { on: { 'COMMIT.PENDING': 'predatesCommit' } },
                predatesCommit: {},
              },
              on: {
                REFRESH: { target: 'reading', reenter: true },
                'READ.FORCE': { target: 'reading', reenter: true },
                'WRITE.STARTED': { guard: 'portOwnsReread', target: 'awaitingWrite', actions: 'reserveOwnedReread' },
                // A reread that no longer owns the read order is silent, in both
                // outcomes: the newer read in flight owns this presentation, and
                // the commit's own observation obligation stays with it.
              },
            },
            /** The reread a Workspace write owns is reserved here, at the moment
             * that write is initiated, and `rereadReservation` names it. Any
             * authoritative read started afterwards leaves this state and
             * revokes the reservation, and the Host's reread is then silently
             * superseded however late it arrives — it neither replaces a newer
             * projection nor publishes a failure the newer read has retired.
             * An unrevoked reservation survives `DETACH` / `ATTACH`: a
             * reattachment of the same generation rejoins this state, and the
             * write's own reread becomes the fresh observation of the new
             * attachment.
             *
             * A native publication newer than the reservation's watermark
             * supersedes it — in this state or on re-entering it after a
             * reattach — by starting the publication-owned read, whose entry
             * revokes the reservation for good. */
            awaitingWrite: {
              always: { guard: 'publicationSupersedesReservation', target: 'reading' },
              on: {
                // The Host issues a write's own reread after that write
                // commits, and a reservation never survives another write.
                'READ.ADOPT': { target: 'settling', actions: { type: 'adoptProjection', params: { postCommit: true } } },
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
             * its own: a later publication of this target's own source scope,
             * a reconnect, a reattach or an explicit refresh drives it. No
             * other scope's publication reaches this actor at all.
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

    /** The one target-wide native source mutation lifecycle, and where its last
     * mutation ended up. Acknowledgement, authoritative observation, conflict,
     * native rejection and an unknown outcome are five separate facts, and each
     * is a state of this region that only a new submission leaves — or, for a
     * definitive non-commit, discarding the intent it was about: no connection
     * generation, presentation attachment or read outcome rewrites it.
     *
     * Every semantic unit submits through this one region, and only where
     * `admitsSourceMutation` holds. `submitting`, `observing` and `unobserved`
     * accept no submission at all: a mutation in flight, or a definitive commit
     * whose post-commit observation is still owed, owns the barrier. A refused
     * `UNIT.SUBMIT` changes nothing — the unit keeps its draft, and nothing
     * queues, replays or retries it. */
    mutation: {
      initial: 'idle',
      states: {
        idle: { on: { 'UNIT.SUBMIT': { guard: 'admitsSourceMutation', target: 'submitting', actions: ['ensureUnit', 'openSubmission'] } } },
        /** The mutation has crossed the native submission boundary and its
         * outcome is not known yet. Leaving this state — on the definitive
         * acknowledgement, a conflict, a native rejection or an unknown outcome
         * — is the mutation's settlement point: from then on the transaction
         * that submitted it records the outcome, and nothing is in flight. */
        submitting: {
          tags: 'mutationInFlight',
          invoke: {
            src: 'writeSource',
            input: ({ context, event }) => ({
              port: context.port,
              expected: context.submission!.expected,
              mutation: (event as Extract<SettingsTargetEvent, { type: 'UNIT.SUBMIT' }>).mutation,
            }),
            // A write that owns a reread offers it to the authority region and
            // then announces the commit. The reread is only *offered*: it is
            // published only while its reservation still stands, and the write
            // ending revokes that reservation either way. `COMMIT.PENDING` is
            // what makes the commit reach a terminal classification when the
            // reread is not adopted. A write that owns no reread forces its own
            // post-commit read instead, which is itself the classification
            // path, so it announces nothing.
            onDone: [
              { guard: 'carriesOwnedReread', target: 'observing', actions: ['offerOwnedReread', 'recordAcknowledgement', raise({ type: 'COMMIT.PENDING' })] },
              { target: 'observing', actions: ['recordAcknowledgement', raise({ type: 'READ.FORCE' })] },
            ],
            onError: [
              { guard: 'isConflict', target: 'conflicted', actions: ['recordWriteFailure', raise({ type: 'READ.FORCE' })] },
              { guard: 'isUncertain', target: 'uncertain', actions: ['recordWriteFailure', raise({ type: 'READ.FORCE' })] },
              { target: 'rejected', actions: ['recordWriteFailure', 'recordRejection', raise({ type: 'READ.FORCE' })] },
            ],
          },
        },
        /** The native write is definitive. What is still open is only the
         * authoritative projection and the native application that follow it —
         * never whether the write committed. The observation this target holds
         * predates the commit, so no second mutation may be fenced on it. */
        observing: {
          on: {
            'COMMIT.OBSERVED': 'saved',
            'COMMIT.UNOBSERVED': 'unobserved',
          },
        },
        /** Committed, and an authoritative read issued after the commit
         * observed the source. The saved notice is truthful against the
         * projection the presentation is actually showing. */
        saved: { on: { 'UNIT.SUBMIT': { guard: 'admitsSourceMutation', target: 'submitting', actions: ['ensureUnit', 'openSubmission'] } } },
        /** Committed, and the authoritative observation that would follow it is
         * explicitly unavailable. Never "unsaved", and never "applied". The
         * barrier stays: only an authoritative post-commit observation — from
         * an explicit refresh, a reattach, a reconnect or this target's own
         * publication — releases it, never the pre-commit projection. */
        unobserved: { on: { 'COMMIT.OBSERVED': 'saved' } },
        /** A conflict or a native rejection is a definitive non-commit whose
         * only remaining subject is the browser intent it was submitted for:
         * its draft and reviewed base, preserved for review. Discarding that
         * unit's intent therefore retires the outcome in the same gesture, so
         * the notice never claims a draft that no longer exists. */
        conflicted: {
          on: {
            'UNIT.SUBMIT': { guard: 'admitsSourceMutation', target: 'submitting', actions: ['ensureUnit', 'openSubmission'] },
            'INTENT.DISCARDED': { guard: 'discardsOutcomeUnit', target: 'idle', actions: 'forgetOutcome' },
          },
        },
        rejected: {
          on: {
            'UNIT.SUBMIT': { guard: 'admitsSourceMutation', target: 'submitting', actions: ['ensureUnit', 'openSubmission'] },
            'INTENT.DISCARDED': { guard: 'discardsOutcomeUnit', target: 'idle', actions: 'forgetOutcome' },
          },
        },
        /** An unknown outcome is a fact about native, not about the browser
         * intent: the write may have committed. Discarding the intent never
         * turns it into "definitely not committed", so only a new submission
         * leaves this state. */
        uncertain: { on: { 'UNIT.SUBMIT': { guard: 'admitsSourceMutation', target: 'submitting', actions: ['ensureUnit', 'openSubmission'] } } },
      },
    },

    /** Explicit native rescan. It re-derives native state and is not a
     * projection, so the caller still owes an authoritative read afterwards. */
    maintenance: {
      initial: 'idle',
      states: {
        idle: { on: { RECONCILE: 'reconciling' } },
        reconciling: {
          entry: assign({ maintenanceError: () => '' }),
          invoke: {
            src: 'reconcileSource',
            input: ({ context }) => ({ port: context.port }),
            onDone: { target: 'idle', actions: raise({ type: 'READ.FORCE' }) },
            onError: { target: 'idle', actions: assign({ maintenanceError: ({ event }) => String(event.error) }) },
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
      // Its write-owned reread reservation ends with it, whether or not a
      // presentation is attached to witness the replacement.
      { guard: 'generationChanged', actions: ['applyTransport', 'retireObservation', 'revokeRereadReservation', raise({ type: 'GENERATION.REPLACED' })] },
      { guard: 'transportChanged', actions: ['applyTransport', raise({ type: 'TRIGGER' })] },
    ],
    'UNIT.EDIT': { actions: ['ensureUnit', 'forwardEdit'] },
    'UNIT.REVIEW': { actions: 'forwardReview' },
    'UNIT.DISCARD': { actions: ['forwardDiscard', raise(({ event }) => ({ type: 'INTENT.DISCARDED' as const, identity: event.identity }))] },
    'UNIT.RETIRED': { actions: 'retireUnit' },
  },
});

/** Whether this target has a native mutation in flight: submitted across the
 * native submission boundary and not yet settled on the transaction that
 * submitted it. */
export function mutationInFlight(snapshot: SnapshotFrom<typeof settingsTargetMachine>): boolean {
  return snapshot.hasTag('mutationInFlight');
}

/** The semantic unit whose definitive commit is this target's current
 * mutation outcome. It is a fact of the `mutation` region, so it survives the
 * retirement of the transaction that submitted the commit; only a new
 * submission replaces it. */
export function committedUnit(snapshot: SnapshotFrom<typeof settingsTargetMachine>): string | undefined {
  const committed = snapshot.matches({ mutation: 'observing' }) || snapshot.matches({ mutation: 'saved' })
    || snapshot.matches({ mutation: 'unobserved' });
  return committed ? snapshot.context.outcomeUnit : undefined;
}

/** The `mutation` region's outcome, for presentation. */
export function mutationOutcome(snapshot: SnapshotFrom<typeof settingsTargetMachine>): MutationOutcome {
  if (snapshot.matches({ mutation: 'submitting' })) return { kind: 'submitting' };
  if (snapshot.matches({ mutation: 'observing' })) return { kind: 'committed' };
  if (snapshot.matches({ mutation: 'saved' })) return { kind: 'saved', observed: true };
  if (snapshot.matches({ mutation: 'unobserved' })) return { kind: 'saved', observed: false };
  if (snapshot.matches({ mutation: 'conflicted' })) return { kind: 'conflict' };
  if (snapshot.matches({ mutation: 'rejected' })) return { kind: 'rejected', detail: snapshot.context.rejection! };
  if (snapshot.matches({ mutation: 'uncertain' })) return { kind: 'uncertain' };
  return { kind: 'none' };
}

/** The `mutation` region's outcome, attributed to exactly the unit it is about.
 *
 * A target reports one outcome at a time because it admits one mutation at a
 * time, but a page shows several independent units at once. Attributing the
 * outcome to its own unit is what lets each of them report its own result: a
 * committed definition and a conflicted permission are two facts about two
 * native mutations, and neither may be presented as the other's. */
export function unitOutcome(snapshot: SnapshotFrom<typeof settingsTargetMachine>, identity: string): MutationOutcome {
  return snapshot.context.outcomeUnit === identity ? mutationOutcome(snapshot) : { kind: 'none' };
}
