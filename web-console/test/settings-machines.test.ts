import { expect, it, vi } from 'vitest';
import { assign, createActor, setup, type ActorRefFrom, type InspectionEvent } from 'xstate';
import type { ConfigurationApplication, SourceMutation, SourceSettings } from '../../protocol/app-server/v19';
import { admitsSourceMutation, mutationOutcome, settingsTargetMachine } from '../src/app/settings/machines/settings-target';
import { awaitingCommitObservation, discardable, requiresReview, unitTransactionMachine } from '../src/app/settings/machines/unit-transaction';
import { adoptionInFlight, sessionConfigurationMachine } from '../src/app/settings/machines/session-configuration';
import {
  admitsFocus, settingsNavigationMachine, settingsPages,
  type OwnerResolution, type SettingsFocus, type SettingsPage,
} from '../src/app/settings/machines/navigation';
import type { ConfigurationPort, WriteOutcome } from '../src/app/settings/machines/port';
import { ConfigurationSystem } from '../src/app/settings/machines/system';
import { revisionSelector, userSettingsTarget, workspaceSettingsTarget } from '../src/app/settings/projection';
import { OutcomeUncertain, RpcFailure, type AppServerClient, type ClientView, type ConnectionState } from '../src/client/app-server';
import type { ProductHostWorkspaces, WorkspaceConfigurationOperation, WorkspaceConfigurationResult } from '../src/workspaces/host';
import { cfg3Source, cfg3SourceApplication } from './cfg3-data';

/** Every ordering in this file is established by an explicit deferred promise
 * that the test itself releases. There is no sleep, timer, fake clock or
 * scheduling assumption anywhere. */
function deferred<T>() {
  let resolve!: (value: T) => void;
  let reject!: (reason?: unknown) => void;
  const promise = new Promise<T>((res, rej) => { resolve = res; reject = rej; });
  // An unobserved rejection is a test artefact, never a machine behaviour.
  promise.catch(() => {});
  return { promise, resolve, reject };
}
/** Drain promise continuations deterministically. */
const flush = async () => { for (let turn = 0; turn < 16; turn++) await Promise.resolve(); };

function projection(revision: string, application?: ConfigurationApplication | null): SourceSettings {
  const source = cfg3Source();
  source.user.revision = revision;
  source.application = application ?? null;
  return source;
}
const userApplication = (version: string) => ({ ...cfg3SourceApplication({ kind: 'user' }), version });
const toolsMutation: SourceMutation = { kind: 'config', mutation: { unit: 'native_tools', authored: ['read'] } };
const toolsIdentity = JSON.stringify({ kind: 'config', mutation: { unit: 'native_tools', authored: null } });

interface Scripted {
  port: ConfigurationPort;
  reads: { resolve: (value: SourceSettings) => void; reject: (reason?: unknown) => void }[];
  writes: { expected: string; mutation: SourceMutation; resolve: (value: WriteOutcome) => void; reject: (reason?: unknown) => void }[];
}
/** A native port whose every operation is released explicitly by the test. */
function scriptedPort(ownsReread = false): Scripted {
  const reads: Scripted['reads'] = [];
  const writes: Scripted['writes'] = [];
  return {
    reads, writes,
    port: {
      ownsReread,
      // A machine test hands the target its publication level directly.
      publication: () => undefined,
      read: () => { const gate = deferred<SourceSettings>(); reads.push(gate); return gate.promise; },
      write: (expected, mutation) => { const gate = deferred<WriteOutcome>(); writes.push({ expected, mutation, ...gate }); return gate.promise; },
      reconcile: async () => {},
    },
  };
}
function settingsActor(
  port: ConfigurationPort, publication?: ConfigurationApplication, workspace = false,
  inspect?: (inspection: InspectionEvent) => void,
) {
  const actor = createActor(settingsTargetMachine, {
    input: {
      target: workspace ? workspaceSettingsTarget('A', 'A') : userSettingsTarget,
      port, connection: 'connected', generation: 1, publication,
    },
    inspect,
  });
  actor.start();
  actor.send({ type: 'ATTACH' });
  return actor;
}
const unitOf = (actor: ReturnType<typeof settingsActor>, identity = toolsIdentity) => actor.getSnapshot().context.units[identity];
/** The unit's transaction settled and, owning nothing, was retired and removed. */
const expectRetired = (actor: ReturnType<typeof settingsActor>, identity = toolsIdentity) => expect(unitOf(actor, identity)).toBeUndefined();
const edit = (actor: ReturnType<typeof settingsActor>, value: unknown, revision: string) =>
  actor.send({ type: 'UNIT.EDIT', identity: toolsIdentity, selector: revisionSelector(toolsMutation), revision, value });
const submit = (actor: ReturnType<typeof settingsActor>, revision: string, mutation: SourceMutation = toolsMutation) =>
  actor.send({ type: 'UNIT.SUBMIT', identity: toolsIdentity, selector: revisionSelector(mutation), revision, mutation });

// ── 1. Read linearization ───────────────────────────────────────────────────

it.each(['A resolves first', 'A resolves last'] as const)('R01 only the newest initiated read may publish, when %s', async order => {
  const scripted = scriptedPort();
  const actor = settingsActor(scripted.port);
  await flush();
  expect(scripted.reads).toHaveLength(1);
  // A second read is initiated while the first is still outstanding. Starting
  // it *stops* the first read actor, so read A has no completion path at all.
  actor.send({ type: 'REFRESH' });
  await flush();
  expect(scripted.reads).toHaveLength(2);
  const [a, b] = scripted.reads;
  if (order === 'A resolves first') { a.resolve(projection('A')); await flush(); b.resolve(projection('B')); }
  else { b.resolve(projection('B')); await flush(); a.resolve(projection('A')); }
  await flush();
  expect(actor.getSnapshot().context.observation?.user.revision).toBe('B');
});

it('R01 a superseded read cannot publish a read failure either', async () => {
  const scripted = scriptedPort();
  const actor = settingsActor(scripted.port);
  await flush();
  actor.send({ type: 'REFRESH' });
  await flush();
  scripted.reads[0].reject(new Error('obsolete read failed'));
  scripted.reads[1].resolve(projection('B'));
  await flush();
  expect(actor.getSnapshot().context.readError).toBe('');
  expect(actor.getSnapshot().context.observation?.user.revision).toBe('B');
});

// ── 1b. Connection-generation ownership of observations ─────────────────────
//
// An observation belongs to exactly one connection generation. Once a newer
// generation is current, nothing the older one started — a read, a read
// failure, a Workspace-owned reread — may become authoritative for it. The
// mutation that was already submitted keeps its own transaction lifetime.

const reconnect = (actor: ReturnType<typeof settingsActor>, generation: number, publication?: ConfigurationApplication) =>
  actor.send({ type: 'TRANSPORT', connection: 'connected', generation, publication });

it.each(['A resolves first', 'A resolves last'] as const)('R16 a read of a replaced connection generation never becomes authoritative, when %s', async order => {
  const scripted = scriptedPort();
  const actor = settingsActor(scripted.port);
  await flush();
  expect(scripted.reads).toHaveLength(1);
  // Generation 2 replaces the observation lifetime generation 1 owned, and
  // obtains its own authoritative observation from nothing.
  reconnect(actor, 2);
  await flush();
  expect(scripted.reads).toHaveLength(2);
  const [a, b] = scripted.reads;
  if (order === 'A resolves first') { a.resolve(projection('old-generation')); await flush(); b.resolve(projection('new-generation')); }
  else { b.resolve(projection('new-generation')); await flush(); a.resolve(projection('old-generation')); }
  await flush();
  const snapshot = actor.getSnapshot();
  expect(snapshot.context.observation?.user.revision).toBe('new-generation');
  expect(snapshot.context.readError).toBe('');
  expect(mutationOutcome(snapshot)).toEqual({ kind: 'none' });
});

it('R16 a read failure of a replaced connection generation never populates the read failure', async () => {
  const scripted = scriptedPort();
  const actor = settingsActor(scripted.port);
  await flush();
  reconnect(actor, 2);
  await flush();
  expect(scripted.reads).toHaveLength(2);
  scripted.reads[0].reject(new Error('obsolete generation read failed'));
  scripted.reads[1].resolve(projection('new-generation'));
  await flush();
  const snapshot = actor.getSnapshot();
  expect(snapshot.context.readError).toBe('');
  expect(snapshot.context.observation?.user.revision).toBe('new-generation');
});

it.each(['observed', 'failed'] as const)('R17 a Workspace reread of a replaced generation settles its own transaction and publishes nothing, when the reread %s', async outcome => {
  const scripted = scriptedPort(true);
  const actor = settingsActor(scripted.port, undefined, true);
  await flush();
  scripted.reads[0].resolve(projection('r1'));
  await flush();
  edit(actor, ['read'], 'r1');
  submit(actor, 'r1');
  await flush();
  // The Workspace write reserves the read order at its initiation…
  expect(actor.getSnapshot().matches({ authority: { attached: 'awaitingWrite' } })).toBe(true);
  // …but that reservation belongs to the generation that made it. Generation 2
  // leaves it and starts its own authoritative read.
  reconnect(actor, 2);
  await flush();
  expect(actor.getSnapshot().matches({ authority: { attached: 'reading' } })).toBe(true);
  expect(scripted.reads).toHaveLength(2);
  scripted.writes[0].resolve({
    acknowledgement: projection('r2'),
    reread: outcome === 'observed'
      ? { status: 'observed', projection: projection('obsolete-reread') }
      : { status: 'failed', error: new Error('obsolete reread failed') },
  });
  await flush();
  // The mutation crossed the native submission boundary before the generation
  // changed, so it still records its definitive commit on its own transaction.
  expect(unitOf(actor).getSnapshot().context.submitted?.committed).toBe('r2');
  // What it may not do is publish its generation's observation into the new one.
  expect(actor.getSnapshot().context.observation).toBeUndefined();
  expect(actor.getSnapshot().context.readError).toBe('');
  // Only the new generation's own read is authoritative for it, and it
  // discharges the commit's observation obligation exactly once.
  scripted.reads[1].resolve(projection('r2'));
  await flush();
  const snapshot = actor.getSnapshot();
  expect(snapshot.context.observation?.user.revision).toBe('r2');
  expectRetired(actor);
  expect(snapshot.matches({ mutation: 'saved' })).toBe(true);
  expect(mutationOutcome(snapshot)).toEqual({ kind: 'saved', observed: true });
  expect(scripted.writes).toHaveLength(1);
  expect(scripted.reads).toHaveLength(2);
});

// ── 2. A successful read clears only the read failure it answers ────────────

it('R02 a later successful read clears the read failure and nothing else', async () => {
  const scripted = scriptedPort();
  const actor = settingsActor(scripted.port);
  await flush();
  scripted.reads[0].resolve(projection('r1'));
  await flush();
  // An independent write failure, then a read failure.
  submit(actor, 'r1');
  await flush();
  scripted.writes[0].reject(new RpcFailure({ code: -32000, message: 'Conflict', data: { kind: 'source_conflict', scope: 'user', expected: 'r1', actual: 'r-external' } }));
  await flush();
  expect(mutationOutcome(actor.getSnapshot())).toEqual({ kind: 'conflict' });
  scripted.reads.at(-1)!.reject(new Error('read unavailable'));
  await flush();
  expect(actor.getSnapshot().context.readError).toContain('read unavailable');
  actor.send({ type: 'REFRESH' });
  await flush();
  scripted.reads.at(-1)!.resolve(projection('r2'));
  await flush();
  expect(actor.getSnapshot().context.readError).toBe('');
  // The write failure is a different fact and survives untouched.
  expect(mutationOutcome(actor.getSnapshot())).toEqual({ kind: 'conflict' });
});

// ── 3. Commit vs. observation ───────────────────────────────────────────────

it('R03 a committed write whose reread fails stays committed with an uncertain observation', async () => {
  const scripted = scriptedPort(true);
  const actor = settingsActor(scripted.port, undefined, true);
  await flush();
  scripted.reads[0].resolve(projection('r1'));
  await flush();
  edit(actor, ['read'], 'r1');
  submit(actor, 'r1');
  await flush();
  scripted.writes[0].resolve({ acknowledgement: projection('r2'), reread: { status: 'failed', error: new Error('reread unavailable') } });
  await flush();
  const snapshot = actor.getSnapshot();
  // The commit is definitive and is recorded on the transaction that submitted it.
  expect(unitOf(actor).getSnapshot().matches({ mutation: 'acknowledged' })).toBe(true);
  expect(unitOf(actor).getSnapshot().context.submitted?.committed).toBe('r2');
  expect(mutationOutcome(snapshot)).toEqual({ kind: 'saved', observed: false });
  // The observation, and only the observation, is uncertain.
  expect(snapshot.context.readError).toContain('Saved, but the authoritative reread failed');
  expect(snapshot.matches({ authority: { attached: 'blocked' } })).toBe(true);
  expect(snapshot.matches({ mutation: 'unobserved' })).toBe(true);
  expect(scripted.writes).toHaveLength(1);
});

// ── 3b. A definitive commit always reaches a terminal classification ────────
//
// Once a definitive commit exists it is classified exactly once under the
// evidence available: observed if an authoritative projection can be had,
// committed-but-unobserved if none can. The read failure and the
// acknowledgement may arrive in either order and must converge to the same
// semantic result, with no replay, no second read and no polling.

it.each([
  ['the superseding read fails first', 'failed'],
  ['the write settles first', 'failed'],
  ['the superseding read fails first', 'observed'],
  ['the write settles first', 'observed'],
] as const)('R18 a commit whose superseding read fails settles as committed-but-unobserved, when %s and its own reread %s', async (order, reread) => {
  const scripted = scriptedPort(true);
  const actor = settingsActor(scripted.port, userApplication('1'), true);
  await flush();
  scripted.reads[0].resolve(projection('r1', userApplication('1')));
  await flush();
  expect(scripted.reads).toHaveLength(1);
  edit(actor, ['read'], 'r1');
  submit(actor, 'r1');
  await flush();
  expect(actor.getSnapshot().matches({ authority: { attached: 'awaitingWrite' } })).toBe(true);
  // A new native publication owes a read that takes presentation authority
  // away from the Workspace write's own reread.
  reconnect(actor, 1, userApplication('2'));
  await flush();
  expect(scripted.reads).toHaveLength(2);
  expect(actor.getSnapshot().matches({ authority: { attached: 'reading' } })).toBe(true);
  const failRead = () => scripted.reads[1].reject(new Error('superseding read failed'));
  const settleWrite = () => scripted.writes[0].resolve({
    acknowledgement: projection('r2'),
    reread: reread === 'observed'
      ? { status: 'observed', projection: projection('obsolete-reread') }
      : { status: 'failed', error: new Error('obsolete reread failed') },
  });
  if (order === 'the superseding read fails first') { failRead(); await flush(); settleWrite(); }
  else { settleWrite(); await flush(); failRead(); }
  await flush();
  const snapshot = actor.getSnapshot();
  // Terminal in either delivery order: never left waiting in `observing`.
  expect(snapshot.matches({ mutation: 'unobserved' })).toBe(true);
  // The save is definitively committed, and says so truthfully.
  expect(mutationOutcome(snapshot)).toEqual({ kind: 'saved', observed: false });
  expect(unitOf(actor).getSnapshot().matches({ mutation: 'acknowledged' })).toBe(true);
  expect(unitOf(actor).getSnapshot().context.submitted?.committed).toBe('r2');
  expect(snapshot.context.convergenceError).toBe('');
  // The observation, and only the observation, is uncertain — reported by the
  // read that owned the order, never by the superseded reread.
  expect(snapshot.matches({ authority: { attached: 'blocked' } })).toBe(true);
  expect(snapshot.context.readError).toContain('superseding read failed');
  expect(snapshot.context.readError).not.toContain('Saved, but the authoritative reread failed');
  expect(snapshot.context.observation?.user.revision).toBe('r1');
  // Exactly one write ever left the browser, and nothing retries or polls.
  expect(scripted.writes).toHaveLength(1);
  expect(scripted.reads).toHaveLength(2);
});

it('R18 a Product Host commit that lands while the connection is down settles, and the reconnect observes it', async () => {
  const scripted = scriptedPort(true);
  const actor = settingsActor(scripted.port, undefined, true);
  await flush();
  scripted.reads[0].resolve(projection('r1'));
  await flush();
  edit(actor, ['read'], 'r1');
  submit(actor, 'r1');
  await flush();
  // The App Server connection is lost, which is always a new generation. The
  // Product Host is an independent transport and the write is still in flight.
  actor.send({ type: 'TRANSPORT', connection: 'stale', generation: 2, publication: undefined });
  await flush();
  expect(scripted.reads).toHaveLength(1);
  expect(actor.getSnapshot().matches({ authority: { attached: 'blocked' } })).toBe(true);
  scripted.writes[0].resolve({ acknowledgement: projection('r2'), reread: { status: 'observed', projection: projection('obsolete-reread') } });
  await flush();
  // The commit is definitive and classified under the evidence available —
  // which is none — rather than left waiting for an unrelated future event.
  const disconnected = actor.getSnapshot();
  expect(disconnected.matches({ mutation: 'unobserved' })).toBe(true);
  expect(mutationOutcome(disconnected)).toEqual({ kind: 'saved', observed: false });
  expect(unitOf(actor).getSnapshot().context.submitted?.committed).toBe('r2');
  expect(disconnected.context.observation).toBeUndefined();
  expect(scripted.reads).toHaveLength(1);
  // Losing the connection is what bumps the generation; coming back up
  // publishes `connected` on that same generation. Its own authoritative read
  // is the only thing that may observe the commit.
  reconnect(actor, 2);
  await flush();
  expect(scripted.reads).toHaveLength(2);
  scripted.reads[1].resolve(projection('r2'));
  await flush();
  const reconnected = actor.getSnapshot();
  expect(reconnected.context.observation?.user.revision).toBe('r2');
  expectRetired(actor);
  expect(reconnected.matches({ mutation: 'saved' })).toBe(true);
  expect(scripted.writes).toHaveLength(1);
});

// ── 4./5. Lifetimes ─────────────────────────────────────────────────────────

it('R04 a definitive acknowledgement settles its transaction after the presentation is detached', async () => {
  const scripted = scriptedPort();
  const actor = settingsActor(scripted.port);
  await flush();
  scripted.reads[0].resolve(projection('r1'));
  await flush();
  edit(actor, ['read'], 'r1');
  submit(actor, 'r1');
  await flush();
  // The Settings dialog closes while the acknowledgement is in flight.
  actor.send({ type: 'DETACH' });
  expect(actor.getSnapshot().matches({ authority: 'suspended' })).toBe(true);
  // The observation is demoted to stale presentation data; the transaction is
  // retained in full.
  expect(actor.getSnapshot().context.observation).toBeUndefined();
  expect(actor.getSnapshot().context.staleObservation?.user.revision).toBe('r1');
  const readsBefore = scripted.reads.length;
  scripted.writes[0].resolve({ acknowledgement: projection('r2') });
  await flush();
  // The commit is recorded on the exact transaction that submitted it, and a
  // detached lifetime issues no read of its own.
  expect(unitOf(actor).getSnapshot().context.submitted?.committed).toBe('r2');
  expect(scripted.reads).toHaveLength(readsBefore);
  // Reattaching observes it, and the transaction settles exactly once. The
  // fresh observation every new attachment owes and the commit's post-commit
  // read are the same single read.
  actor.send({ type: 'ATTACH' });
  await flush();
  expect(scripted.reads).toHaveLength(readsBefore + 1);
  expect(actor.getSnapshot().context.observation).toBeUndefined();
  scripted.reads.at(-1)!.resolve(projection('r2'));
  await flush();
  expectRetired(actor);
  expect(scripted.writes).toHaveLength(1);
});

it('R05 an authority replacement leaves the old acknowledgement settling only its own lifetime', async () => {
  const old = scriptedPort();
  const replacement = scriptedPort();
  const oldActor = settingsActor(old.port);
  await flush();
  old.reads[0].resolve(projection('r1'));
  await flush();
  edit(oldActor, ['read'], 'r1');
  submit(oldActor, 'r1');
  await flush();
  // The authority is replaced: the old lifetime is detached, the replacement
  // starts from nothing.
  oldActor.send({ type: 'DETACH' });
  const newActor = settingsActor(replacement.port);
  await flush();
  replacement.reads[0].resolve(projection('fresh'));
  await flush();
  old.writes[0].resolve({ acknowledgement: projection('r2') });
  await flush();
  // The old transaction records its own commit; the replacement never sees it.
  expect(unitOf(oldActor).getSnapshot().context.submitted?.committed).toBe('r2');
  expect(newActor.getSnapshot().context.units).toEqual({});
  expect(newActor.getSnapshot().context.observation?.user.revision).toBe('fresh');
  expect(mutationOutcome(newActor.getSnapshot())).toEqual({ kind: 'none' });
  expect(replacement.writes).toHaveLength(0);
});

// ── 4b. Presentation attachment revalidates the observation ─────────────────
//
// Presentation lifetime, observation lifetime, editing transaction lifetime,
// native mutation lifetime, App Server authority lifetime and connection
// generation lifetime are six separate facts. `DETACH` retains editing
// transactions; `ATTACH` revalidates authoritative observation. The retained
// observation is stale presentation data: keeping it for presentation is never
// declaring it the fresh authority of a newly attached presentation.

it('R19 reopening rereads even without publication', async () => {
  const scripted = scriptedPort();
  const actor = settingsActor(scripted.port);
  await flush();
  scripted.reads[0].resolve(projection('r1'));
  await flush();
  expect(actor.getSnapshot().context.observation?.user.revision).toBe('r1');
  actor.send({ type: 'DETACH' });
  // The observation is demoted to stale presentation data: retained for
  // presentation, but no longer authority.
  const detached = actor.getSnapshot();
  expect(detached.context.observation).toBeUndefined();
  expect(detached.context.staleObservation?.user.revision).toBe('r1');
  // External authority becomes r2 with no publication change, no generation
  // change and no commit obligation — exactly the case where r1 used to stay
  // on screen as if it were current.
  actor.send({ type: 'ATTACH' });
  await flush();
  // The new attachment owes one fresh authoritative read, and the old r1 is
  // not silently treated as fresh authority while it is in flight: nothing is
  // read twice, and nothing may be submitted against the demoted value.
  expect(scripted.reads).toHaveLength(2);
  const attaching = actor.getSnapshot();
  expect(attaching.matches({ authority: { attached: 'reading' } })).toBe(true);
  expect(attaching.context.observation).toBeUndefined();
  submit(actor, 'r1');
  await flush();
  expect(scripted.writes).toHaveLength(0);
  scripted.reads[1].resolve(projection('r2'));
  await flush();
  const snapshot = actor.getSnapshot();
  expect(snapshot.context.observation?.user.revision).toBe('r2');
  expect(snapshot.context.staleObservation).toBeUndefined();
  // Exactly two reads occurred, and nothing polls.
  expect(scripted.reads).toHaveLength(2);
});

it('R20 reopening preserves a dirty draft and pinned base', async () => {
  const scripted = scriptedPort();
  const actor = settingsActor(scripted.port);
  await flush();
  scripted.reads[0].resolve(projection('r1'));
  await flush();
  edit(actor, ['read'], 'r1');
  const authored = unitOf(actor).getSnapshot();
  expect(authored.context.draft).toEqual({ value: ['read'] });
  expect(authored.context.base).toBe('r1');
  actor.send({ type: 'DETACH' });
  actor.send({ type: 'ATTACH' });
  await flush();
  scripted.reads.at(-1)!.resolve(projection('r2'));
  await flush();
  const unit = unitOf(actor).getSnapshot();
  // The draft survives the presentation bounce and is not recreated from the
  // new native value.
  expect(unit.context.draft).toEqual({ value: ['read'] });
  expect(unit.matches({ intent: 'dirty' })).toBe(true);
  // The pinned CAS base remains r1 while the unit's observed revision advances
  // to r2, so the transaction now requires review/conflict handling.
  expect(unit.context.base).toBe('r1');
  expect(unit.context.observed).toBe('r2');
  expect(unit.matches({ base: 'pinned' })).toBe(true);
  // Nothing is submitted automatically, and the next submission is fenced on
  // exactly the pinned base rather than on the new authority revision.
  expect(scripted.writes).toHaveLength(0);
  submit(actor, 'r2');
  await flush();
  expect(scripted.writes).toHaveLength(1);
  expect(scripted.writes[0].expected).toBe('r1');
});

it('R21 reopening after a read failure retries the observation', async () => {
  const scripted = scriptedPort();
  const actor = settingsActor(scripted.port);
  await flush();
  scripted.reads[0].resolve(projection('r1'));
  await flush();
  // An independent write failure first, then the read failure.
  submit(actor, 'r1');
  await flush();
  scripted.writes[0].reject(new RpcFailure({ code: -32000, message: 'Conflict', data: { kind: 'source_conflict', scope: 'user', expected: 'r1', actual: 'r-external' } }));
  await flush();
  expect(mutationOutcome(actor.getSnapshot())).toEqual({ kind: 'conflict' });
  scripted.reads.at(-1)!.reject(new Error('read unavailable'));
  await flush();
  expect(actor.getSnapshot().context.readError).toContain('read unavailable');
  actor.send({ type: 'DETACH' });
  actor.send({ type: 'ATTACH' });
  await flush();
  // Reattachment initiates a fresh authoritative read even though a previous
  // observation is retained.
  expect(scripted.reads).toHaveLength(3);
  scripted.reads[2].resolve(projection('r2'));
  await flush();
  const snapshot = actor.getSnapshot();
  // The successful read clears only the read failure it answers…
  expect(snapshot.context.readError).toBe('');
  // …the independent write failure is untouched…
  expect(mutationOutcome(snapshot)).toEqual({ kind: 'conflict' });
  // …and the observation is authoritative again.
  expect(snapshot.context.observation?.user.revision).toBe('r2');
  expect(snapshot.matches({ authority: { attached: 'idle' } })).toBe(true);
  expect(snapshot.matches({ mutation: 'conflicted' })).toBe(true);
});

// ── 4c. Attachment validation composes with every standing obligation ───────
//
// The attachment's validation read is one obligation among the ones already
// standing, and they all coalesce through the single read owner. There is no
// second convergence worker, no parallel refresh mechanism and no replayed
// write.

it('R22 reopening with an unobserved commit coalesces validation and post-commit into one read', async () => {
  const scripted = scriptedPort();
  const actor = settingsActor(scripted.port);
  await flush();
  scripted.reads[0].resolve(projection('r1'));
  await flush();
  edit(actor, ['read'], 'r1');
  submit(actor, 'r1');
  await flush();
  actor.send({ type: 'DETACH' });
  // The definitive commit lands while nothing is presented.
  scripted.writes[0].resolve({ acknowledgement: projection('r2') });
  await flush();
  expect(actor.getSnapshot().context.unobservedSettlement).toEqual({ identity: toolsIdentity, selector: revisionSelector(toolsMutation), committed: true, revision: 'r2' });
  actor.send({ type: 'ATTACH' });
  await flush();
  // The new attachment's validation and the commit's post-commit read are the
  // same single authoritative read.
  expect(scripted.reads).toHaveLength(2);
  scripted.reads[1].resolve(projection('r2'));
  await flush();
  expect(actor.getSnapshot().context.observation?.user.revision).toBe('r2');
  expectRetired(actor);
  expect(actor.getSnapshot().matches({ mutation: 'saved' })).toBe(true);
  // Nothing duplicates, replays or polls: one write ever, two reads ever.
  expect(scripted.reads).toHaveLength(2);
  expect(scripted.writes).toHaveLength(1);
});

it('R22 reopening with a newer publication coalesces validation and publication into one read', async () => {
  const scripted = scriptedPort();
  const actor = settingsActor(scripted.port, userApplication('1'));
  await flush();
  scripted.reads[0].resolve(projection('r1', userApplication('1')));
  await flush();
  actor.send({ type: 'DETACH' });
  // A newer native publication arrives while nothing is presented.
  reconnect(actor, 1, userApplication('2'));
  actor.send({ type: 'ATTACH' });
  await flush();
  // One read answers both the new attachment and the publication.
  expect(scripted.reads).toHaveLength(2);
  scripted.reads[1].resolve(projection('r2', userApplication('2')));
  await flush();
  expect(actor.getSnapshot().context.observation?.user.revision).toBe('r2');
  expect(scripted.reads).toHaveLength(2);
});

it('R22 reopening during a Workspace write keeps its reserved reread as the fresh observation', async () => {
  const scripted = scriptedPort(true);
  const actor = settingsActor(scripted.port, undefined, true);
  await flush();
  scripted.reads[0].resolve(projection('r1'));
  await flush();
  edit(actor, ['read'], 'r1');
  submit(actor, 'r1');
  await flush();
  expect(actor.getSnapshot().matches({ authority: { attached: 'awaitingWrite' } })).toBe(true);
  actor.send({ type: 'DETACH' });
  actor.send({ type: 'ATTACH' });
  await flush();
  // The write reserved the read order at its initiation; the presentation
  // bounce restores that reservation instead of starting a competing read.
  expect(scripted.reads).toHaveLength(1);
  expect(actor.getSnapshot().matches({ authority: { attached: 'awaitingWrite' } })).toBe(true);
  scripted.writes[0].resolve({ acknowledgement: projection('r2'), reread: { status: 'observed', projection: projection('r2') } });
  await flush();
  // The write's own reread is the new attachment's fresh authoritative
  // observation, and it settles the committed transaction exactly once.
  const snapshot = actor.getSnapshot();
  expect(snapshot.context.observation?.user.revision).toBe('r2');
  expect(snapshot.context.staleObservation).toBeUndefined();
  expectRetired(actor);
  expect(snapshot.matches({ mutation: 'saved' })).toBe(true);
  expect(scripted.reads).toHaveLength(1);
  expect(scripted.writes).toHaveLength(1);
});

it('R22 a commit landing during the reattach read supersedes it into one post-commit read', async () => {
  const scripted = scriptedPort();
  const actor = settingsActor(scripted.port);
  await flush();
  scripted.reads[0].resolve(projection('r1'));
  await flush();
  edit(actor, ['read'], 'r1');
  submit(actor, 'r1');
  await flush();
  actor.send({ type: 'DETACH' });
  actor.send({ type: 'ATTACH' });
  await flush();
  expect(scripted.reads).toHaveLength(2);
  // The validation read is in flight when the acknowledgement lands. It
  // predates the commit, so the post-commit read the commit forces supersedes
  // it rather than letting it discharge the commit's observation obligation.
  scripted.writes[0].resolve({ acknowledgement: projection('r2') });
  await flush();
  expect(scripted.reads).toHaveLength(3);
  // The superseded read can never publish, whatever it carries and whenever it
  // settles.
  scripted.reads[1].resolve(projection('stale'));
  await flush();
  expect(actor.getSnapshot().context.observation).toBeUndefined();
  scripted.reads[2].resolve(projection('r2'));
  await flush();
  const snapshot = actor.getSnapshot();
  expect(snapshot.context.observation?.user.revision).toBe('r2');
  expectRetired(actor);
  expect(snapshot.matches({ mutation: 'saved' })).toBe(true);
  expect(scripted.writes).toHaveLength(1);
});

// ── 4d. Reattachment around connection-generation replacement ───────────────

it.each(['the old-generation read resolves first', 'the old-generation read resolves last'] as const)(
  'R23 an old-generation read never publishes into the reattached presentation, when %s', async order => {
    const scripted = scriptedPort();
    const actor = settingsActor(scripted.port);
    await flush();
    scripted.reads[0].resolve(projection('r1'));
    await flush();
    // A same-generation read is issued and left outstanding across the close.
    actor.send({ type: 'REFRESH' });
    await flush();
    expect(scripted.reads).toHaveLength(2);
    actor.send({ type: 'DETACH' });
    // The connection generation is replaced while nothing is presented, which
    // retires the old generation's observation outright — not even stale
    // presentation data survives into the new generation.
    reconnect(actor, 2);
    const retired = actor.getSnapshot();
    expect(retired.context.observation).toBeUndefined();
    expect(retired.context.staleObservation).toBeUndefined();
    // The new generation's presentation attachment obtains its own observation
    // from nothing.
    actor.send({ type: 'ATTACH' });
    await flush();
    expect(scripted.reads).toHaveLength(3);
    const stale = scripted.reads[1], fresh = scripted.reads[2];
    if (order === 'the old-generation read resolves first') {
      stale.resolve(projection('old-generation'));
      await flush();
      expect(actor.getSnapshot().context.observation).toBeUndefined();
      fresh.resolve(projection('new-generation'));
    } else {
      fresh.resolve(projection('new-generation'));
      await flush();
      expect(actor.getSnapshot().context.observation?.user.revision).toBe('new-generation');
      stale.resolve(projection('old-generation'));
    }
    await flush();
    const snapshot = actor.getSnapshot();
    expect(snapshot.context.observation?.user.revision).toBe('new-generation');
    expect(snapshot.context.readError).toBe('');
    expect(mutationOutcome(snapshot)).toEqual({ kind: 'none' });
    expect(scripted.reads).toHaveLength(3);
  });

it('R23 a late failure of an old-generation read cannot populate the read failure', async () => {
  const scripted = scriptedPort();
  const actor = settingsActor(scripted.port);
  await flush();
  expect(scripted.reads).toHaveLength(1);
  actor.send({ type: 'DETACH' });
  reconnect(actor, 2);
  actor.send({ type: 'ATTACH' });
  await flush();
  expect(scripted.reads).toHaveLength(2);
  // The generation-1 read fails after the new generation's presentation is
  // already attached: its failure belongs to the retired lifetime.
  scripted.reads[0].reject(new Error('old-generation read failed'));
  await flush();
  expect(actor.getSnapshot().context.readError).toBe('');
  expect(actor.getSnapshot().context.observation).toBeUndefined();
  scripted.reads[1].resolve(projection('new-generation'));
  await flush();
  const snapshot = actor.getSnapshot();
  expect(snapshot.context.observation?.user.revision).toBe('new-generation');
  expect(snapshot.context.readError).toBe('');
});

// ── 4e. A write outlives its generation; its reread authority does not ──────
//
// A Workspace write that crossed the native submission boundary settles its own
// transaction whatever happens to the connection or the presentation. The read
// order it reserved for its own reread is a different fact: it is publication
// authority of the generation that took it, and once revoked no later
// `DETACH` / `ATTACH` restores it merely because the write is still pending.

/** The settlement facts of the actor system: every definitive commit delivered
 * to a transaction, and every state the target's `mutation` region passes
 * through, collapsed to changes. */
function settlementLog() {
  const log = { commits: 0, mutation: [] as string[] };
  const inspect = (inspection: InspectionEvent) => {
    if (inspection.type === '@xstate.event' && inspection.event.type === 'COMMITTED') log.commits += 1;
    if (inspection.type !== '@xstate.snapshot' || inspection.actorRef.sessionId !== inspection.rootId) return;
    const mutation = String((inspection.snapshot as ReturnType<ReturnType<typeof settingsActor>['getSnapshot']>).value.mutation);
    if (log.mutation.at(-1) !== mutation) log.mutation.push(mutation);
  };
  return { log, inspect };
}
const obsoleteReread = (outcome: 'observed' | 'failed'): WriteOutcome['reread'] => outcome === 'observed'
  ? { status: 'observed', projection: projection('obsolete-reread') }
  : { status: 'failed', error: new Error('obsolete reread failed') };
const canSubmit = (actor: ReturnType<typeof settingsActor>, revision: string) =>
  actor.getSnapshot().can({ type: 'UNIT.SUBMIT', identity: toolsIdentity, selector: revisionSelector(toolsMutation), revision, mutation: toolsMutation });

it.each([
  ['observed', 'established'],
  ['observed', 'in flight'],
  ['failed', 'established'],
  ['failed', 'in flight'],
] as const)('R24 reattaching after a generation replacement never resurrects the Workspace reread reservation, when the obsolete reread %s and the generation-2 read is %s', async (outcome, replacement) => {
  const scripted = scriptedPort(true);
  const settlement = settlementLog();
  const actor = settingsActor(scripted.port, undefined, true, settlement.inspect);
  await flush();
  scripted.reads[0].resolve(projection('r1'));
  await flush();
  edit(actor, ['read'], 'r1');
  submit(actor, 'r1');
  await flush();
  expect(actor.getSnapshot().matches({ authority: { attached: 'awaitingWrite' } })).toBe(true);
  // Generation 2 replaces generation 1 while W1 is still pending, which
  // revokes generation 1's reread authority and starts generation 2's own read.
  reconnect(actor, 2);
  await flush();
  expect(scripted.reads).toHaveLength(2);
  if (replacement === 'established') {
    scripted.reads[1].resolve(projection('generation-2'));
    await flush();
    expect(actor.getSnapshot().context.observation?.user.revision).toBe('generation-2');
  }
  // The Settings presentation bounces while W1 is still pending.
  actor.send({ type: 'DETACH' });
  actor.send({ type: 'ATTACH' });
  await flush();
  // The pending write does not restore the revoked reservation: the new
  // attachment validates through its own generation-2 read.
  expect(actor.getSnapshot().matches({ authority: { attached: 'reading' } })).toBe(true);
  expect(actor.getSnapshot().matches({ mutation: 'submitting' })).toBe(true);
  expect(scripted.reads).toHaveLength(3);
  scripted.writes[0].resolve({ acknowledgement: projection('r2'), reread: obsoleteReread(outcome) });
  await flush();
  const late = actor.getSnapshot();
  // W1 still records its definitive commit on the transaction that submitted it…
  expect(unitOf(actor).getSnapshot().context.submitted?.committed).toBe('r2');
  expect(late.matches({ mutation: 'observing' })).toBe(true);
  // …but its generation-1 reread publishes nothing: no projection, no read
  // failure, no block. The generation-2 read still owns the observation.
  expect(late.context.observation).toBeUndefined();
  expect(late.context.staleObservation?.user.revision).toBe(replacement === 'established' ? 'generation-2' : undefined);
  expect(late.context.readError).toBe('');
  expect(late.matches({ authority: { attached: 'reading' } })).toBe(true);
  scripted.reads[2].resolve(projection('r2'));
  await flush();
  const snapshot = actor.getSnapshot();
  expect(snapshot.context.observation?.user.revision).toBe('r2');
  expect(snapshot.context.readError).toBe('');
  expect(snapshot.matches({ authority: { attached: 'idle' } })).toBe(true);
  expectRetired(actor);
  expect(snapshot.matches({ mutation: 'saved' })).toBe(true);
  expect(mutationOutcome(snapshot)).toEqual({ kind: 'saved', observed: true });
  // Editing is governed by generation 2 alone.
  expect(canSubmit(actor, 'r2')).toBe(true);
  // W1 settled exactly once, and was never replayed.
  expect(settlement.log).toEqual({ commits: 1, mutation: ['idle', 'submitting', 'observing', 'saved'] });
  expect(scripted.writes).toHaveLength(1);
  expect(scripted.reads).toHaveLength(3);
});

it.each(['observed', 'failed'] as const)('R24 a generation replaced while Settings is detached revokes the Workspace reread reservation, when the obsolete reread %s', async outcome => {
  const scripted = scriptedPort(true);
  const settlement = settlementLog();
  const actor = settingsActor(scripted.port, undefined, true, settlement.inspect);
  await flush();
  scripted.reads[0].resolve(projection('r1'));
  await flush();
  edit(actor, ['read'], 'r1');
  submit(actor, 'r1');
  await flush();
  // The reservation is suspended, not revoked, by the close…
  actor.send({ type: 'DETACH' });
  // …and revoked by the replacement, which no presentation has to witness.
  reconnect(actor, 2);
  actor.send({ type: 'ATTACH' });
  await flush();
  expect(actor.getSnapshot().matches({ authority: { attached: 'reading' } })).toBe(true);
  expect(scripted.reads).toHaveLength(2);
  scripted.writes[0].resolve({ acknowledgement: projection('r2'), reread: obsoleteReread(outcome) });
  await flush();
  expect(actor.getSnapshot().context.observation).toBeUndefined();
  expect(actor.getSnapshot().context.readError).toBe('');
  expect(actor.getSnapshot().matches({ authority: { attached: 'reading' } })).toBe(true);
  scripted.reads[1].resolve(projection('r2'));
  await flush();
  const snapshot = actor.getSnapshot();
  expect(snapshot.context.observation?.user.revision).toBe('r2');
  expect(snapshot.context.readError).toBe('');
  expectRetired(actor);
  expect(snapshot.matches({ mutation: 'saved' })).toBe(true);
  expect(canSubmit(actor, 'r2')).toBe(true);
  expect(settlement.log).toEqual({ commits: 1, mutation: ['idle', 'submitting', 'observing', 'saved'] });
  expect(scripted.writes).toHaveLength(1);
  expect(scripted.reads).toHaveLength(2);
});

it('R24 a reservation superseded by a newer publication read is not resurrected by reattaching', async () => {
  const scripted = scriptedPort(true);
  const actor = settingsActor(scripted.port, userApplication('1'), true);
  await flush();
  scripted.reads[0].resolve(projection('r1', userApplication('1')));
  await flush();
  edit(actor, ['read'], 'r1');
  submit(actor, 'r1');
  await flush();
  // Same generation: a newer publication owes a read that supersedes the
  // write-owned reread.
  reconnect(actor, 1, userApplication('2'));
  await flush();
  expect(actor.getSnapshot().matches({ authority: { attached: 'reading' } })).toBe(true);
  actor.send({ type: 'DETACH' });
  actor.send({ type: 'ATTACH' });
  await flush();
  expect(actor.getSnapshot().matches({ authority: { attached: 'reading' } })).toBe(true);
  expect(scripted.reads).toHaveLength(3);
  scripted.writes[0].resolve({ acknowledgement: projection('r2'), reread: obsoleteReread('observed') });
  await flush();
  expect(actor.getSnapshot().context.observation).toBeUndefined();
  scripted.reads[2].resolve(projection('r2', userApplication('2')));
  await flush();
  const snapshot = actor.getSnapshot();
  expect(snapshot.context.observation?.user.revision).toBe('r2');
  expectRetired(actor);
  expect(snapshot.matches({ mutation: 'saved' })).toBe(true);
  expect(scripted.writes).toHaveLength(1);
});

// ── 4f. Publication progress is independent of presentation observation ─────
//
// A Workspace reread reservation records the native publication watermark it
// was established against. A publication newer than that watermark supersedes
// the reservation whether or not a current presentation observation exists —
// and every `ATTACH` deliberately demotes the observation. `DETACH` alone never
// revokes the reservation; a newer read or a replaced generation always does.

it.each([
  ['after the reattach', 'observed'],
  ['after the reattach', 'failed'],
  ['while detached', 'observed'],
  ['while detached', 'failed'],
] as const)('R25 a same-generation publication newer than the reservation watermark supersedes the reserved Workspace reread across a reattach, when it arrives %s and the late reread %s', async (arrival, outcome) => {
  const scripted = scriptedPort(true);
  const settlement = settlementLog();
  const actor = settingsActor(scripted.port, userApplication('1'), true, settlement.inspect);
  await flush();
  scripted.reads[0].resolve(projection('r1', userApplication('1')));
  await flush();
  edit(actor, ['read'], 'r1');
  submit(actor, 'r1');
  await flush();
  // The Workspace write reserves the read order at publication 1.
  expect(actor.getSnapshot().matches({ authority: { attached: 'awaitingWrite' } })).toBe(true);
  expect(actor.getSnapshot().context.rereadReservation).toEqual({ token: 1, publication: 1n });
  actor.send({ type: 'DETACH' });
  if (arrival === 'while detached') reconnect(actor, 1, userApplication('2'));
  else {
    // A presentation bounce alone neither revokes the reservation nor lets the
    // reattached presentation start a competing read: the publication the
    // reservation was established against is exactly what its reread answers.
    actor.send({ type: 'ATTACH' });
    await flush();
    const reattached = actor.getSnapshot();
    expect(reattached.matches({ authority: { attached: 'awaitingWrite' } })).toBe(true);
    expect(reattached.context.observation).toBeUndefined();
    expect(reattached.context.rereadReservation).toEqual({ token: 1, publication: 1n });
    expect(scripted.reads).toHaveLength(1);
    // With no current observation at all, publication 2 is still newer than
    // the reservation's watermark.
    reconnect(actor, 1, userApplication('2'));
  }
  if (arrival === 'while detached') actor.send({ type: 'ATTACH' });
  await flush();
  // The publication supersedes the reservation for good and the authority
  // region performs the publication-owned read.
  const superseded = actor.getSnapshot();
  expect(superseded.matches({ authority: { attached: 'reading' } })).toBe(true);
  expect(superseded.context.rereadReservation).toBeUndefined();
  expect(superseded.matches({ mutation: 'submitting' })).toBe(true);
  expect(scripted.reads).toHaveLength(2);
  scripted.writes[0].resolve({ acknowledgement: projection('r2'), reread: obsoleteReread(outcome) });
  await flush();
  // The late reread publishes nothing in either outcome: no projection, no
  // read failure, no block. The commit itself is recorded on its transaction.
  const late = actor.getSnapshot();
  expect(unitOf(actor).getSnapshot().context.submitted?.committed).toBe('r2');
  expect(late.context.observation).toBeUndefined();
  expect(late.context.readError).toBe('');
  expect(late.matches({ authority: { attached: 'reading' } })).toBe(true);
  // A second bounce cannot resurrect the revoked reservation either.
  actor.send({ type: 'DETACH' });
  actor.send({ type: 'ATTACH' });
  await flush();
  expect(actor.getSnapshot().matches({ authority: { attached: 'reading' } })).toBe(true);
  expect(scripted.reads).toHaveLength(3);
  // The publication-owned read becomes authoritative and settles the mutation.
  scripted.reads[2].resolve(projection('r2', userApplication('2')));
  await flush();
  const snapshot = actor.getSnapshot();
  expect(snapshot.context.observation?.user.revision).toBe('r2');
  expect(snapshot.context.readError).toBe('');
  expect(snapshot.matches({ authority: { attached: 'idle' } })).toBe(true);
  expectRetired(actor);
  expect(snapshot.matches({ mutation: 'saved' })).toBe(true);
  expect(mutationOutcome(snapshot)).toEqual({ kind: 'saved', observed: true });
  // Settled exactly once, never replayed, and nothing polls afterwards.
  expect(settlement.log).toEqual({ commits: 1, mutation: ['idle', 'submitting', 'observing', 'saved'] });
  expect(scripted.writes).toHaveLength(1);
  expect(scripted.reads).toHaveLength(3);
});

it('R25 a publication no newer than the reservation watermark never supersedes the reserved reread', async () => {
  const scripted = scriptedPort(true);
  const actor = settingsActor(scripted.port, userApplication('1'), true);
  await flush();
  scripted.reads[0].resolve(projection('r1', userApplication('1')));
  await flush();
  edit(actor, ['read'], 'r1');
  submit(actor, 'r1');
  await flush();
  actor.send({ type: 'DETACH' });
  actor.send({ type: 'ATTACH' });
  // The same publication delivered again is no publication progress.
  reconnect(actor, 1, userApplication('1'));
  await flush();
  expect(actor.getSnapshot().matches({ authority: { attached: 'awaitingWrite' } })).toBe(true);
  expect(scripted.reads).toHaveLength(1);
  scripted.writes[0].resolve({ acknowledgement: projection('r2'), reread: { status: 'observed', projection: projection('r2', userApplication('1')) } });
  await flush();
  // The reserved reread is the reattached presentation's fresh observation.
  const snapshot = actor.getSnapshot();
  expect(snapshot.context.observation?.user.revision).toBe('r2');
  expectRetired(actor);
  expect(snapshot.matches({ mutation: 'saved' })).toBe(true);
  expect(scripted.reads).toHaveLength(1);
  expect(scripted.writes).toHaveLength(1);
});

// ── 6./7./8. Per-unit CAS transactions ──────────────────────────────────────

it('R06 a clean Remove that conflicts keeps its pinned CAS base across an editor remount', async () => {
  const scripted = scriptedPort();
  const actor = settingsActor(scripted.port);
  await flush();
  scripted.reads[0].resolve(projection('r1'));
  await flush();
  // A clean Remove authors no value at all; it only pins the reviewed base.
  submit(actor, 'r1', { kind: 'config', mutation: { unit: 'native_tools', authored: null } });
  await flush();
  scripted.writes[0].reject(new RpcFailure({ code: -32000, message: 'Conflict', data: { kind: 'source_conflict', scope: 'user', expected: 'r1', actual: 'r-external' } }));
  await flush();
  // The reread observes the external revision. The pinned base does not move,
  // and no fake value draft was ever manufactured to keep the intent alive.
  scripted.reads.at(-1)!.resolve(projection('r-external'));
  await flush();
  const unit = unitOf(actor).getSnapshot();
  expect(unit.context.draft).toBeUndefined();
  expect(unit.context.base).toBe('r1');
  expect(unit.context.observed).toBe('r-external');
  expect(unit.matches({ base: 'pinned' })).toBe(true);
  // The transaction is owned by the target actor, so an editor remount — which
  // is only a React unmount — cannot reach it at all.
  submit(actor, 'r1', { kind: 'config', mutation: { unit: 'native_tools', authored: null } });
  await flush();
  expect(scripted.writes.at(-1)!.expected).toBe('r1');
});

it('R07 an explicitly reviewed revision conflicts again on the next external edit', async () => {
  const scripted = scriptedPort();
  const actor = settingsActor(scripted.port);
  await flush();
  scripted.reads[0].resolve(projection('R1'));
  await flush();
  edit(actor, ['read'], 'R1');
  actor.send({ type: 'UNIT.REVIEW', identity: toolsIdentity });
  // R2 is observed and explicitly reviewed.
  actor.send({ type: 'REFRESH' });
  await flush();
  scripted.reads.at(-1)!.resolve(projection('R2'));
  await flush();
  actor.send({ type: 'UNIT.REVIEW', identity: toolsIdentity });
  expect(unitOf(actor).getSnapshot().context.base).toBe('R2');
  // A second external edit advances the source to R3.
  actor.send({ type: 'REFRESH' });
  await flush();
  scripted.reads.at(-1)!.resolve(projection('R3'));
  await flush();
  const unit = unitOf(actor).getSnapshot();
  expect(unit.context.base).toBe('R2');
  expect(unit.context.observed).toBe('R3');
  // The next mutation is still fenced on exactly the reviewed revision, so
  // native conflicts again rather than silently overwriting R3.
  submit(actor, 'R3');
  await flush();
  expect(scripted.writes.at(-1)!.expected).toBe('R2');
});

it('R08 a late acknowledgement of an older intent never erases a newer edit', async () => {
  const scripted = scriptedPort();
  const actor = settingsActor(scripted.port);
  await flush();
  scripted.reads[0].resolve(projection('r1'));
  await flush();
  edit(actor, ['read'], 'r1');
  submit(actor, 'r1');
  await flush();
  // Intent B is authored while submission A is still in flight.
  edit(actor, ['read', 'write'], 'r1');
  scripted.writes[0].resolve({ acknowledgement: projection('r2') });
  await flush();
  const unit = unitOf(actor).getSnapshot();
  expect(unit.context.draft).toEqual({ value: ['read', 'write'] });
  // The older commit may advance the CAS base; it may not retire the newer intent.
  expect(unit.context.base).toBe('r2');
  expect(unit.matches({ intent: 'dirty' })).toBe(true);
  expect(scripted.writes).toHaveLength(1);
});

// ── 9./10. Convergence ──────────────────────────────────────────────────────

it('R09 a publication arriving during an outstanding read keeps the obligation', async () => {
  const scripted = scriptedPort();
  const actor = settingsActor(scripted.port, userApplication('1'));
  await flush();
  scripted.reads[0].resolve(projection('r1', userApplication('1')));
  await flush();
  expect(scripted.reads).toHaveLength(1);
  // Publication 2 starts exactly one read. Publication 3 arrives while it is
  // outstanding and starts none — the obligation is a level, not an edge.
  actor.send({ type: 'TRANSPORT', connection: 'connected', generation: 1, publication: userApplication('2') });
  await flush();
  expect(scripted.reads).toHaveLength(2);
  actor.send({ type: 'TRANSPORT', connection: 'connected', generation: 1, publication: userApplication('3') });
  await flush();
  expect(scripted.reads).toHaveLength(2);
  // The outstanding read settles at 2; the surviving obligation drives exactly
  // one more bounded read, and nothing polls afterwards.
  scripted.reads[1].resolve(projection('r2', userApplication('2')));
  await flush();
  expect(scripted.reads).toHaveLength(3);
  scripted.reads[2].resolve(projection('r3', userApplication('3')));
  await flush();
  expect(scripted.reads).toHaveLength(3);
});

it('R10 a newer publication cannot be discharged by an older projection', async () => {
  const scripted = scriptedPort();
  const actor = settingsActor(scripted.port, userApplication('5'));
  await flush();
  // A projection that predates the publication does not satisfy it.
  scripted.reads[0].resolve(projection('r1', userApplication('4')));
  await flush();
  expect(scripted.reads).toHaveLength(2);
  // A second reply still below the published version is measurably stale, and
  // is reported once instead of driving an unbounded chase.
  scripted.reads[1].resolve(projection('r1', userApplication('4')));
  await flush();
  expect(scripted.reads).toHaveLength(2);
  expect(actor.getSnapshot().context.convergenceError).toContain('published application version 5');
  expect(actor.getSnapshot().matches({ authority: { attached: 'blocked' } })).toBe(true);
});

// ── 11. Unknown write outcome ───────────────────────────────────────────────

it('R11 an unknown write outcome rereads authority and never replays the mutation', async () => {
  const scripted = scriptedPort();
  const actor = settingsActor(scripted.port);
  await flush();
  scripted.reads[0].resolve(projection('r1'));
  await flush();
  edit(actor, ['read'], 'r1');
  submit(actor, 'r1');
  await flush();
  const readsBefore = scripted.reads.length;
  scripted.writes[0].reject(new OutcomeUncertain());
  await flush();
  expect(actor.getSnapshot().matches({ mutation: 'uncertain' })).toBe(true);
  expect(mutationOutcome(actor.getSnapshot())).toEqual({ kind: 'uncertain' });
  expect(scripted.reads.length).toBe(readsBefore + 1);
  scripted.reads.at(-1)!.resolve(projection('r1'));
  await flush();
  // Exactly one write ever left the browser, and the draft is preserved.
  expect(scripted.writes).toHaveLength(1);
  expect(unitOf(actor).getSnapshot().context.draft).toEqual({ value: ['read'] });
});

// ── 11b. A mutation outcome belongs to the mutation, not to a generation ─────
//
// Replacing a connection generation retires what that generation *observed*.
// Conflict, native rejection and an unknown outcome are facts about a mutation
// that already crossed the native submission boundary, so they must read the
// same whichever of the outcome and the replacement arrived first, and only a
// new submission replaces them.

const conflict = () => new RpcFailure({ code: -32000, message: 'Conflict', data: { kind: 'source_conflict', scope: 'user', expected: 'r1', actual: 'r-external' } });
const rejection = () => new RpcFailure({ code: -32602, message: 'Invalid authored unit' });
const failures = {
  uncertain: { cause: () => new OutcomeUncertain(), outcome: { kind: 'uncertain' } },
  rejected: { cause: rejection, outcome: { kind: 'rejected', detail: String(rejection()) } },
  conflict: { cause: conflict, outcome: { kind: 'conflict' } },
} as const;
const mutationState = { uncertain: 'uncertain', rejected: 'rejected', conflict: 'conflicted' } as const;

it.each([
  ['uncertain', 'the outcome arrives before the generation replacement'],
  ['uncertain', 'the generation replacement arrives before the outcome'],
  ['rejected', 'the outcome arrives before the generation replacement'],
  ['rejected', 'the generation replacement arrives before the outcome'],
  ['conflict', 'the outcome arrives before the generation replacement'],
  ['conflict', 'the generation replacement arrives before the outcome'],
] as const)('R28 a %s mutation outcome survives connection generation replacement when %s', async (kind, order) => {
  const scripted = scriptedPort();
  const actor = settingsActor(scripted.port);
  await flush();
  scripted.reads[0].resolve(projection('r1'));
  await flush();
  edit(actor, ['read'], 'r1');
  submit(actor, 'r1');
  await flush();
  expect(scripted.writes).toHaveLength(1);
  const fail = async () => {
    scripted.writes[0].reject(failures[kind].cause());
    await flush();
    // The mutation region owns the outcome from the moment it is known.
    expect(actor.getSnapshot().matches({ mutation: mutationState[kind] })).toBe(true);
  };
  const replace = async () => {
    reconnect(actor, 2);
    await flush();
    // The replacement retired generation 1's observation, and only that.
    expect(actor.getSnapshot().context.observation).toBeUndefined();
  };
  if (order === 'the outcome arrives before the generation replacement') { await fail(); await replace(); }
  else { await replace(); await fail(); }
  // Both orders leave exactly one live read: the outcome's own authoritative
  // reread and the new generation's first read coalesce through the read owner.
  expect(scripted.reads).toHaveLength(3);
  expect(actor.getSnapshot().matches({ authority: { attached: 'reading' } })).toBe(true);
  scripted.reads[2].resolve(projection('r1'));
  await flush();
  const snapshot = actor.getSnapshot();
  expect(snapshot.context.observation?.user.revision).toBe('r1');
  expect(snapshot.context.readError).toBe('');
  // The user-visible outcome is identical in both delivery orders…
  expect(snapshot.matches({ mutation: mutationState[kind] })).toBe(true);
  expect(mutationOutcome(snapshot)).toEqual(failures[kind].outcome);
  // …and the transaction still holds the dirty intent and its exact CAS base:
  // the save never reads as an untouched draft.
  const unit = unitOf(actor).getSnapshot();
  expect(unit.matches({ intent: 'dirty', base: 'pinned', mutation: 'unconfirmed' })).toBe(true);
  expect(unit.context.draft).toEqual({ value: ['read'] });
  expect(unit.context.base).toBe('r1');
  expect(unit.context.submitted).toBeUndefined();
  // Nothing replays the mutation — not the reread, and not a further
  // generation replacement.
  reconnect(actor, 3);
  await flush();
  scripted.reads.at(-1)!.resolve(projection('r1'));
  await flush();
  expect(mutationOutcome(actor.getSnapshot())).toEqual(failures[kind].outcome);
  expect(scripted.writes).toHaveLength(1);
});

it('R28 a successful read after a reconnect clears only the read failure, never the mutation outcome', async () => {
  const scripted = scriptedPort();
  const actor = settingsActor(scripted.port);
  await flush();
  scripted.reads[0].resolve(projection('r1'));
  await flush();
  edit(actor, ['read'], 'r1');
  submit(actor, 'r1');
  await flush();
  scripted.writes[0].reject(conflict());
  await flush();
  reconnect(actor, 2);
  await flush();
  // The new generation's read fails: a read failure of generation 2.
  scripted.reads.at(-1)!.reject(new Error('generation 2 read unavailable'));
  await flush();
  expect(actor.getSnapshot().context.readError).toContain('generation 2 read unavailable');
  expect(mutationOutcome(actor.getSnapshot())).toEqual({ kind: 'conflict' });
  actor.send({ type: 'REFRESH' });
  await flush();
  scripted.reads.at(-1)!.resolve(projection('r-external'));
  await flush();
  const snapshot = actor.getSnapshot();
  expect(snapshot.context.readError).toBe('');
  expect(mutationOutcome(snapshot)).toEqual({ kind: 'conflict' });
  expect(requiresReview(unitOf(actor).getSnapshot())).toBe(true);
  expect(scripted.writes).toHaveLength(1);
});

it('R28 only a deliberate new submission replaces the previous mutation outcome', async () => {
  const scripted = scriptedPort();
  const actor = settingsActor(scripted.port);
  await flush();
  scripted.reads[0].resolve(projection('r1'));
  await flush();
  edit(actor, ['read'], 'r1');
  submit(actor, 'r1');
  await flush();
  scripted.writes[0].reject(rejection());
  await flush();
  reconnect(actor, 2);
  await flush();
  scripted.reads.at(-1)!.resolve(projection('r1'));
  await flush();
  expect(mutationOutcome(actor.getSnapshot())).toEqual(failures.rejected.outcome);
  // The user submits again. The mutation region, and nothing else, replaces
  // the rejection with the new submission's own lifecycle.
  submit(actor, 'r1');
  await flush();
  expect(scripted.writes).toHaveLength(2);
  expect(scripted.writes[1].expected).toBe('r1');
  expect(actor.getSnapshot().context.rejection).toBeUndefined();
  expect(mutationOutcome(actor.getSnapshot())).toEqual({ kind: 'submitting' });
  scripted.writes[1].resolve({ acknowledgement: projection('r2') });
  await flush();
  expect(mutationOutcome(actor.getSnapshot())).toEqual({ kind: 'committed' });
  scripted.reads.at(-1)!.resolve(projection('r2'));
  await flush();
  expect(mutationOutcome(actor.getSnapshot())).toEqual({ kind: 'saved', observed: true });
  expectRetired(actor);
});

// ── 11c. Post-commit observation vs. external divergence ───────────────────
//
// A definitive commit advances the CAS base before any authoritative read has
// observed it. Until a read issued after the commit completes, the difference
// is the commit not yet observed. Once one has, any other revision is a real
// external change — including the exact pre-save revision coming back.

it('R29 a post-commit read that observes the exact pre-save revision is an external divergence', async () => {
  const scripted = scriptedPort();
  const actor = settingsActor(scripted.port);
  await flush();
  scripted.reads[0].resolve(projection('r1'));
  await flush();
  edit(actor, ['read'], 'r1');
  submit(actor, 'r1');
  await flush();
  expect(scripted.writes[0].expected).toBe('r1');
  scripted.writes[0].resolve({ acknowledgement: projection('r2') });
  await flush();
  // Acknowledged; the post-commit read is issued but has not completed. The
  // held observation still says r1, and that is not a source change.
  let unit = unitOf(actor).getSnapshot();
  expect(unit.matches({ mutation: { acknowledged: 'awaitingObservation' } })).toBe(true);
  expect(unit.context.submitted?.committed).toBe('r2');
  expect(unit.context.base).toBe('r2');
  expect(unit.context.observed).toBe('r1');
  expect(requiresReview(unit)).toBe(false);
  expect(actor.getSnapshot().matches({ authority: { attached: { reading: 'current' } } })).toBe(true);
  expect(scripted.reads).toHaveLength(2);
  // An external writer restores the source to the pre-save bytes, and the
  // post-commit read observes exactly r1.
  scripted.reads[1].resolve(projection('r1'));
  await flush();
  unit = unitOf(actor).getSnapshot();
  expect(unit.matches({ mutation: { acknowledged: 'diverged' } })).toBe(true);
  expect(unit.matches({ mutation: 'settled' })).toBe(false);
  expect(unit.context.submitted?.committed).toBe('r2');
  expect(unit.context.observed).toBe('r1');
  expect(unit.context.base).toBe('r2');
  expect(requiresReview(unit)).toBe(true);
  expect(actor.getSnapshot().context.observation?.user.revision).toBe('r1');
  // Nothing is replayed and nothing reads again on its own.
  expect(scripted.writes).toHaveLength(1);
  expect(scripted.reads).toHaveLength(2);
  // Another save stays fenced on the committed revision until the user
  // explicitly reviews the current one: native answers with a conflict.
  edit(actor, ['read', 'write'], 'r1');
  submit(actor, 'r1');
  await flush();
  expect(scripted.writes).toHaveLength(2);
  expect(scripted.writes[1].expected).toBe('r2');
  scripted.writes[1].reject(conflict());
  await flush();
  scripted.reads.at(-1)!.resolve(projection('r1'));
  await flush();
  expect(requiresReview(unitOf(actor).getSnapshot())).toBe(true);
  actor.send({ type: 'UNIT.REVIEW', identity: toolsIdentity });
  expect(requiresReview(unitOf(actor).getSnapshot())).toBe(false);
  submit(actor, 'r1');
  await flush();
  expect(scripted.writes).toHaveLength(3);
  expect(scripted.writes[2].expected).toBe('r1');
});

it.each([
  ['r2', 'settles'],
  ['r1', 'diverges'],
] as const)('R29 a read issued before the acknowledgement never classifies the commit; the post-commit read observing %s %s', async (postCommit, verdict) => {
  const scripted = scriptedPort(true);
  const actor = settingsActor(scripted.port, userApplication('1'), true);
  await flush();
  scripted.reads[0].resolve(projection('r1', userApplication('1')));
  await flush();
  edit(actor, ['read'], 'r1');
  submit(actor, 'r1');
  await flush();
  // A newer publication supersedes the Workspace write's reserved reread
  // with a read issued while the write is still in flight.
  reconnect(actor, 1, userApplication('2'));
  await flush();
  expect(scripted.reads).toHaveLength(2);
  expect(actor.getSnapshot().matches({ authority: { attached: { reading: 'current' } } })).toBe(true);
  scripted.writes[0].resolve({ acknowledgement: projection('r2'), reread: obsoleteReread('observed') });
  await flush();
  // The commit is recorded while that read is outstanding, so the read now
  // predates it.
  expect(actor.getSnapshot().matches({ authority: { attached: { reading: 'predatesCommit' } } })).toBe(true);
  expect(unitOf(actor).getSnapshot().matches({ mutation: { acknowledged: 'awaitingObservation' } })).toBe(true);
  scripted.reads[1].resolve(projection('r1', userApplication('2')));
  await flush();
  // Adopted as the current observation, but it is evidence about the source
  // before the commit: no divergence, no review prompt, and the commit's own
  // observation is still owed.
  let unit = unitOf(actor).getSnapshot();
  expect(actor.getSnapshot().context.observation?.user.revision).toBe('r1');
  expect(unit.matches({ mutation: { acknowledged: 'awaitingObservation' } })).toBe(true);
  expect(unit.context.observed).toBe('r1');
  expect(requiresReview(unit)).toBe(false);
  expect(actor.getSnapshot().matches({ mutation: 'observing' })).toBe(true);
  expect(scripted.reads).toHaveLength(3);
  scripted.reads[2].resolve(projection(postCommit, userApplication('2')));
  await flush();
  expect(actor.getSnapshot().matches({ mutation: 'saved' })).toBe(true);
  if (verdict === 'settles') {
    expectRetired(actor);
  } else {
    unit = unitOf(actor).getSnapshot();
    expect(unit.matches({ mutation: { acknowledged: 'diverged' } })).toBe(true);
    expect(unit.context.submitted?.committed).toBe('r2');
    expect(requiresReview(unit)).toBe(true);
  }
  expect(scripted.writes).toHaveLength(1);
  expect(scripted.reads).toHaveLength(3);
});

// ── 11d. Discard abandons browser intent, never native transaction evidence ─
//
// "Discard draft" abandons exactly what the browser authored: a value draft, or
// the reviewed base a clean Remove or a failed mutation pinned as intent. A
// definitive commit is not a draft. Its committed revision, the observation it
// still owes and any divergence that observation reveals survive the gesture,
// and the transaction retires only once it owns nothing at all.

/** Observe r1, author a draft and have native definitively commit it as r2,
 * with the post-commit authoritative read still outstanding. */
async function committedAwaitingObservation() {
  const scripted = scriptedPort();
  const actor = settingsActor(scripted.port);
  await flush();
  scripted.reads[0].resolve(projection('r1'));
  await flush();
  edit(actor, ['read'], 'r1');
  submit(actor, 'r1');
  await flush();
  scripted.writes[0].resolve({ acknowledgement: projection('r2') });
  await flush();
  expect(scripted.reads).toHaveLength(2);
  return { scripted, actor };
}

it('R32 discarding during the post-commit observation keeps the commit, and the observation still reveals the divergence', async () => {
  const { scripted, actor } = await committedAwaitingObservation();
  let unit = unitOf(actor).getSnapshot();
  expect(unit.matches({ mutation: { acknowledged: 'awaitingObservation' } })).toBe(true);
  expect(unit.context.base).toBe('r2');
  expect(unit.context.observed).toBe('r1');
  // The confirmed draft is already gone; nothing left is browser intent.
  expect(discardable(unit)).toBe(false);
  actor.send({ type: 'UNIT.DISCARD', identity: toolsIdentity });
  await flush();
  unit = unitOf(actor).getSnapshot();
  expect(unit.matches({ mutation: { acknowledged: 'awaitingObservation' } })).toBe(true);
  expect(unit.context.submitted?.committed).toBe('r2');
  expect(unit.context.base).toBe('r2');
  // An external writer restores r1, and the post-commit read observes it.
  scripted.reads[1].resolve(projection('r1'));
  await flush();
  unit = unitOf(actor).getSnapshot();
  expect(unit.matches({ mutation: { acknowledged: 'diverged' } })).toBe(true);
  expect(unit.context.submitted?.committed).toBe('r2');
  expect(unit.context.base).toBe('r2');
  expect(unit.context.observed).toBe('r1');
  expect(requiresReview(unit)).toBe(true);
  expect(mutationOutcome(actor.getSnapshot())).toEqual({ kind: 'saved', observed: true });
  expect(scripted.writes).toHaveLength(1);
  expect(scripted.reads).toHaveLength(2);
});

it('R32 discarding after a post-commit divergence keeps the commit and the review requirement', async () => {
  const { scripted, actor } = await committedAwaitingObservation();
  scripted.reads[1].resolve(projection('r1'));
  await flush();
  expect(unitOf(actor).getSnapshot().matches({ mutation: { acknowledged: 'diverged' } })).toBe(true);
  actor.send({ type: 'UNIT.DISCARD', identity: toolsIdentity });
  await flush();
  let unit = unitOf(actor).getSnapshot();
  expect(unit.matches({ mutation: { acknowledged: 'diverged' } })).toBe(true);
  expect(unit.matches({ base: 'pinned' })).toBe(true);
  expect(unit.context.submitted?.committed).toBe('r2');
  expect(unit.context.base).toBe('r2');
  expect(requiresReview(unit)).toBe(true);
  // A newer draft authored over the divergence is intent, and discarding it
  // abandons that draft alone: the divergence evidence is still there.
  edit(actor, ['read', 'write'], 'r1');
  expect(discardable(unitOf(actor).getSnapshot())).toBe(true);
  actor.send({ type: 'UNIT.DISCARD', identity: toolsIdentity });
  await flush();
  unit = unitOf(actor).getSnapshot();
  expect(unit.context.draft).toBeUndefined();
  expect(unit.matches({ mutation: { acknowledged: 'diverged' } })).toBe(true);
  expect(unit.context.base).toBe('r2');
  expect(requiresReview(unit)).toBe(true);
  // The next save is still fenced on the committed revision.
  edit(actor, ['read', 'write'], 'r1');
  submit(actor, 'r1');
  await flush();
  expect(scripted.writes).toHaveLength(2);
  expect(scripted.writes[1].expected).toBe('r2');
});

it('R32 an ordinary dirty draft is discarded cleanly and the transaction retires', async () => {
  const scripted = scriptedPort();
  const actor = settingsActor(scripted.port);
  await flush();
  scripted.reads[0].resolve(projection('r1'));
  await flush();
  edit(actor, ['read'], 'r1');
  const unit = unitOf(actor).getSnapshot();
  expect(unit.matches({ intent: 'dirty', base: 'pinned' })).toBe(true);
  expect(discardable(unit)).toBe(true);
  actor.send({ type: 'UNIT.DISCARD', identity: toolsIdentity });
  await flush();
  expect(unitOf(actor)).toBeUndefined();
  expect(mutationOutcome(actor.getSnapshot())).toEqual({ kind: 'none' });
  expect(scripted.writes).toHaveLength(0);
  // A later edit starts a new transaction fenced on the current authority.
  edit(actor, ['write'], 'r1');
  expect(unitOf(actor).getSnapshot().context.base).toBe('r1');
});

it('R32 discarding a conflicted clean Remove abandons its pinned base without authoring a value', async () => {
  const scripted = scriptedPort();
  const actor = settingsActor(scripted.port);
  await flush();
  scripted.reads[0].resolve(projection('r1'));
  await flush();
  const remove: SourceMutation = { kind: 'config', mutation: { unit: 'native_tools', authored: null } };
  submit(actor, 'r1', remove);
  await flush();
  scripted.writes[0].reject(conflict());
  await flush();
  scripted.reads.at(-1)!.resolve(projection('r-external'));
  await flush();
  let unit = unitOf(actor).getSnapshot();
  expect(unit.matches({ intent: 'clean', base: 'pinned', mutation: 'unconfirmed' })).toBe(true);
  expect(unit.context.base).toBe('r1');
  expect(discardable(unit)).toBe(true);
  // A removal is intent too; discarding it never manufactures a value draft.
  const inspected: unknown[] = [];
  unitOf(actor).subscribe(snapshot => inspected.push(snapshot.context.draft));
  actor.send({ type: 'UNIT.DISCARD', identity: toolsIdentity });
  await flush();
  expect(inspected.every(draft => draft === undefined)).toBe(true);
  expect(unitOf(actor)).toBeUndefined();
  // The conflict notice described the discarded removal; it goes with it.
  expect(mutationOutcome(actor.getSnapshot())).toEqual({ kind: 'none' });
  // The next Remove is fenced on the revision the user now sees.
  submit(actor, 'r-external', remove);
  await flush();
  expect(scripted.writes).toHaveLength(2);
  expect(scripted.writes[1].expected).toBe('r-external');
  unit = unitOf(actor).getSnapshot();
  expect(unit.context.draft).toBeUndefined();
});

it.each(['conflict', 'rejected', 'uncertain'] as const)('R32 discarding the intent of a %s mutation keeps the outcome and the transaction mutually truthful', async failure => {
  const scripted = scriptedPort();
  const actor = settingsActor(scripted.port);
  await flush();
  scripted.reads[0].resolve(projection('r1'));
  await flush();
  edit(actor, ['read'], 'r1');
  submit(actor, 'r1');
  await flush();
  scripted.writes[0].reject(failures[failure].cause());
  await flush();
  scripted.reads.at(-1)!.resolve(projection('r-external'));
  await flush();
  expect(mutationOutcome(actor.getSnapshot())).toEqual(failures[failure].outcome);
  let unit = unitOf(actor).getSnapshot();
  expect(unit.context.draft).toEqual({ value: ['read'] });
  expect(unit.context.base).toBe('r1');
  expect(discardable(unit)).toBe(true);
  // Discarding a different unit touches neither this outcome nor this intent.
  actor.send({ type: 'UNIT.DISCARD', identity: JSON.stringify({ kind: 'config', mutation: { unit: 'runtime_identity', authored: null } }) });
  await flush();
  expect(mutationOutcome(actor.getSnapshot())).toEqual(failures[failure].outcome);
  expect(unitOf(actor).getSnapshot().context.draft).toEqual({ value: ['read'] });
  actor.send({ type: 'UNIT.DISCARD', identity: toolsIdentity });
  await flush();
  // The draft and the pinned base are gone, so nothing claims they are kept.
  expect(unitOf(actor)).toBeUndefined();
  if (failure === 'uncertain') {
    // Native may have committed: discarding the intent never reinterprets an
    // unknown outcome as a definite non-commit.
    expect(actor.getSnapshot().matches({ mutation: 'uncertain' })).toBe(true);
    expect(mutationOutcome(actor.getSnapshot())).toEqual({ kind: 'uncertain' });
  } else {
    // A definitive non-commit described only the intent just abandoned.
    expect(actor.getSnapshot().matches({ mutation: 'idle' })).toBe(true);
    expect(mutationOutcome(actor.getSnapshot())).toEqual({ kind: 'none' });
    expect(actor.getSnapshot().context.rejection).toBeUndefined();
  }
  // Nothing is replayed, and the next edit fences on the current revision.
  expect(scripted.writes).toHaveLength(1);
  edit(actor, ['write'], 'r-external');
  unit = unitOf(actor).getSnapshot();
  expect(unit.context.base).toBe('r-external');
  expect(requiresReview(unit)).toBe(false);
});

it('R32 a mutation in flight owns its intent, so a discard is not accepted until its outcome', async () => {
  const scripted = scriptedPort();
  const actor = settingsActor(scripted.port);
  await flush();
  scripted.reads[0].resolve(projection('r1'));
  await flush();
  edit(actor, ['read'], 'r1');
  submit(actor, 'r1');
  await flush();
  expect(discardable(unitOf(actor).getSnapshot())).toBe(false);
  actor.send({ type: 'UNIT.DISCARD', identity: toolsIdentity });
  await flush();
  const unit = unitOf(actor).getSnapshot();
  expect(unit.matches({ mutation: 'submitting', intent: 'dirty', base: 'pinned' })).toBe(true);
  scripted.writes[0].reject(conflict());
  await flush();
  // The conflict preserves exactly the intent that was submitted.
  expect(unitOf(actor).getSnapshot().context.draft).toEqual({ value: ['read'] });
  expect(unitOf(actor).getSnapshot().context.base).toBe('r1');
});

// ── 11e. A transaction that owns nothing retires exactly once ───────────────
//
// A unit transaction lives exactly as long as it owns something: browser
// intent, a pinned CAS base, or a definitive commit whose authoritative
// observation is owed. The observation that settles a commit is the last of
// those for a transaction with no newer intent, so it retires there — once —
// and its owning target stops and removes it. Touched units therefore never
// accumulate over a long-lived target.

/** Count every `UNIT.RETIRED` a target actor receives. */
function retirementCounter() {
  let count = 0;
  const inspect = (inspection: InspectionEvent) => {
    if (inspection.type === '@xstate.event' && inspection.event.type === 'UNIT.RETIRED') count += 1;
  };
  return { inspect, count: () => count };
}

it('R36 a successfully committed and observed transaction retires, and a later edit starts a fresh one at the committed revision', async () => {
  const retirements = retirementCounter();
  const scripted = scriptedPort();
  const actor = settingsActor(scripted.port, undefined, false, retirements.inspect);
  await flush();
  scripted.reads[0].resolve(projection('r1'));
  await flush();
  edit(actor, ['read'], 'r1');
  submit(actor, 'r1');
  await flush();
  const first = unitOf(actor);
  scripted.writes[0].resolve({ acknowledgement: projection('r2') });
  await flush();
  // Acknowledged, with its observation still owed: the transaction lives.
  expect(unitOf(actor)).toBe(first);
  expect(first.getSnapshot().matches({ mutation: { acknowledged: 'awaitingObservation' } })).toBe(true);
  expect(retirements.count()).toBe(0);
  scripted.reads[1].resolve(projection('r2'));
  await flush();
  expectRetired(actor);
  expect(first.getSnapshot().status).toBe('stopped');
  expect(retirements.count()).toBe(1);
  expect(mutationOutcome(actor.getSnapshot())).toEqual({ kind: 'saved', observed: true });
  // Editing the unit again is a new transaction, fenced on exactly the
  // committed revision the editor now presents.
  edit(actor, ['read', 'write'], 'r2');
  const second = unitOf(actor);
  expect(second).toBeDefined();
  expect(second).not.toBe(first);
  expect(second.getSnapshot().context).toMatchObject({ base: 'r2', observed: 'r2', draft: { value: ['read', 'write'] } });
  expect(second.getSnapshot().matches({ intent: 'dirty', mutation: 'idle', lifetime: 'live' })).toBe(true);
  submit(actor, 'r2');
  await flush();
  expect(scripted.writes[1].expected).toBe('r2');
});

it('R36 a newer browser intent survives the settlement of the older commit, so the transaction does not retire', async () => {
  const retirements = retirementCounter();
  const scripted = scriptedPort();
  const actor = settingsActor(scripted.port, undefined, false, retirements.inspect);
  await flush();
  scripted.reads[0].resolve(projection('r1'));
  await flush();
  edit(actor, ['read'], 'r1');
  submit(actor, 'r1');
  await flush();
  // Intent A2 is authored while A1 is still natively submitting.
  edit(actor, ['read', 'write'], 'r1');
  const unit = unitOf(actor);
  scripted.writes[0].resolve({ acknowledgement: projection('r2') });
  await flush();
  scripted.reads[1].resolve(projection('r2'));
  await flush();
  // A1 is committed and observed; A2 is still exactly this browser's intent.
  expect(unitOf(actor)).toBe(unit);
  const snapshot = unit.getSnapshot();
  expect(snapshot.status).toBe('active');
  expect(snapshot.matches({ intent: 'dirty', base: 'pinned', mutation: 'settled', lifetime: 'live' })).toBe(true);
  expect(snapshot.context.draft).toEqual({ value: ['read', 'write'] });
  expect(snapshot.context).toMatchObject({ base: 'r2', observed: 'r2', submitted: undefined });
  expect(retirements.count()).toBe(0);
  // A2 is submitted against the committed revision A1 produced.
  submit(actor, 'r2');
  await flush();
  expect(scripted.writes[1].expected).toBe('r2');
});

it('R36 one terminal settlement retires the transaction exactly once, however many observations and gestures follow', async () => {
  const retirements = retirementCounter();
  const scripted = scriptedPort();
  const actor = settingsActor(scripted.port, undefined, false, retirements.inspect);
  await flush();
  scripted.reads[0].resolve(projection('r1'));
  await flush();
  edit(actor, ['read'], 'r1');
  submit(actor, 'r1');
  await flush();
  const unit = unitOf(actor);
  scripted.writes[0].resolve({ acknowledgement: projection('r2') });
  await flush();
  scripted.reads[1].resolve(projection('r2'));
  await flush();
  expectRetired(actor);
  expect(unit.getSnapshot().status).toBe('stopped');
  // Later authoritative observations of the same revision and gestures naming
  // the unit reach no retired transaction and remove nothing twice.
  actor.send({ type: 'REFRESH' });
  await flush();
  scripted.reads[2].resolve(projection('r2'));
  await flush();
  actor.send({ type: 'UNIT.DISCARD', identity: toolsIdentity });
  actor.send({ type: 'UNIT.REVIEW', identity: toolsIdentity });
  await flush();
  expectRetired(actor);
  expect(retirements.count()).toBe(1);
});

it('R36 the retirement announcement is a terminal state of the transaction itself, even if its owner never stops it', () => {
  // An owner that only counts announcements, and never stops the child.
  const owner = setup({
    types: { context: {} as { retired: number; unit?: ActorRefFrom<typeof unitTransactionMachine> } },
    actors: { unitTransaction: unitTransactionMachine },
  }).createMachine({
    context: { retired: 0 },
    entry: assign({
      unit: ({ spawn }) => spawn('unitTransaction', { input: { identity: toolsIdentity, selector: revisionSelector(toolsMutation), revision: 'r1' } }),
    }),
    on: { 'UNIT.RETIRED': { actions: assign({ retired: ({ context }) => context.retired + 1 }) } },
  });
  const parent = createActor(owner);
  parent.start();
  const unit = parent.getSnapshot().context.unit!;
  unit.send({ type: 'EDIT', value: ['read'] });
  unit.send({ type: 'SUBMIT', token: 1 });
  unit.send({ type: 'COMMITTED', token: 1, revision: 'r2' });
  // A post-commit observation of anything else is a divergence, not a
  // settlement: the commit's obligation still stands.
  unit.send({ type: 'OBSERVED', revision: 'r1', postCommit: true });
  expect(unit.getSnapshot().matches({ lifetime: 'live' })).toBe(true);
  expect(parent.getSnapshot().context.retired).toBe(0);
  unit.send({ type: 'OBSERVED', revision: 'r2', postCommit: true });
  expect(unit.getSnapshot().matches({ intent: 'clean', base: 'following', mutation: 'settled', lifetime: 'retired' })).toBe(true);
  expect(unit.getSnapshot().context.base).toBe('r2');
  expect(parent.getSnapshot().context.retired).toBe(1);
  // Nothing that reaches the retired transaction announces it again.
  unit.send({ type: 'OBSERVED', revision: 'r2', postCommit: true });
  unit.send({ type: 'DISCARD' });
  unit.send({ type: 'EDIT', value: ['write'] });
  unit.send({ type: 'DISCARD' });
  expect(unit.getSnapshot().matches({ lifetime: 'retired' })).toBe(true);
  expect(parent.getSnapshot().context.retired).toBe(1);
});

// ── 12./13. Session configuration ───────────────────────────────────────────

const candidate = { identity: { input_revision: 'input-2', attempt: '2' }, expected_binding: '1', impact: 'prefix_changed' as const };

/** A Session actor whose reads and adoptions are released explicitly by the
 * test. Created on a connected generation, it starts the one read that
 * connected span owes by itself: `reads[0]`. */
function scriptedSession(connection: ConnectionState = 'connected', generation = 1) {
  const reads: ReturnType<typeof deferred<ConfigurationApplication | null>>[] = [];
  const adoptions: ReturnType<typeof deferred<void>>[] = [];
  const actor = createActor(sessionConfigurationMachine, {
    input: {
      port: {
        read: () => { const gate = deferred<ConfigurationApplication | null>(); reads.push(gate); return gate.promise; },
        adopt: () => { const gate = deferred<void>(); adoptions.push(gate); return gate.promise; },
      },
      connection, generation,
    },
  });
  actor.start();
  return { actor, reads, adoptions };
}
const transport = (actor: ReturnType<typeof scriptedSession>['actor'], connection: ConnectionState, generation: number) =>
  actor.send({ type: 'TRANSPORT', connection, generation });

it('R12 a failed reread after a failed adoption strands no in-flight guard', async () => {
  const { actor, reads, adoptions } = scriptedSession();
  await flush();
  reads[0].resolve({ ...cfg3SourceApplication(), candidate });
  await flush();
  actor.send({ type: 'ADOPT', candidate });
  await flush();
  adoptions[0].reject(new Error('NotReady'));
  await flush();
  // The adoption region leaves `submitting` on the adoption response alone.
  expect(actor.getSnapshot().matches({ adoption: 'rejected' })).toBe(true);
  expect(actor.getSnapshot().context.adoptionError).toContain('NotReady');
  // The authoritative reread it triggers is independent cleanup: however it
  // settles it cannot strand the guard, and it never replays the adoption.
  reads.at(-1)!.reject(new Error('configuration read unavailable'));
  await flush();
  expect(actor.getSnapshot().matches({ adoption: 'rejected' })).toBe(true);
  expect(actor.getSnapshot().matches({ observation: { connected: 'failed' } })).toBe(true);
  expect(adoptions).toHaveLength(1);
  // With no authoritative observation there is nothing to adopt against.
  actor.send({ type: 'ADOPT', candidate });
  await flush();
  expect(actor.getSnapshot().matches({ adoption: 'rejected' })).toBe(true);
  expect(adoptions).toHaveLength(1);
  // A later observation makes the same candidate actionable again.
  actor.send({ type: 'REFRESH' });
  await flush();
  reads.at(-1)!.resolve({ ...cfg3SourceApplication(), candidate });
  await flush();
  actor.send({ type: 'ADOPT', candidate });
  await flush();
  expect(actor.getSnapshot().matches({ adoption: 'submitting' })).toBe(true);
  expect(adoptions).toHaveLength(2);
});

it('R13 a successful observation clears the read failure and preserves the independent adoption failure', async () => {
  const { actor, reads, adoptions } = scriptedSession();
  await flush();
  reads[0].resolve({ ...cfg3SourceApplication(), candidate });
  await flush();
  actor.send({ type: 'ADOPT', candidate });
  await flush();
  adoptions[0].reject(new Error('Conflict'));
  await flush();
  reads.at(-1)!.reject(new Error('configuration read unavailable'));
  await flush();
  expect(actor.getSnapshot().context.readError).toContain('configuration read unavailable');
  expect(actor.getSnapshot().context.adoptionError).toContain('Conflict');
  actor.send({ type: 'REFRESH' });
  await flush();
  reads.at(-1)!.resolve({ ...cfg3SourceApplication(), version: '9', candidate });
  await flush();
  // Exactly the read state it answers is cleared.
  expect(actor.getSnapshot().context.readError).toBe('');
  expect(actor.getSnapshot().matches({ observation: { connected: 'ready' } })).toBe(true);
  expect(actor.getSnapshot().context.adoptionError).toContain('Conflict');
});

// ── 13b. Session observation is owned by the transport, not by a presentation
//
// A connected span of one connection generation owes exactly one authoritative
// read, started by the transition into `connected` itself. These regressions
// drive nothing but transport transitions: no presentation, no Session
// attachment and no snapshot change is needed for recovery.

const otherCandidate = { identity: { input_revision: 'input-9', attempt: '9' }, expected_binding: '2', impact: 'prefix_changed' as const };

it('R13b a reconnect reads exactly once, on connected, and the new generation establishes its own application-version baseline', async () => {
  const { actor, reads } = scriptedSession();
  // The actor owes, and starts, the first read of its connected span itself.
  expect(reads).toHaveLength(1);
  reads[0].resolve({ ...cfg3SourceApplication(), version: '100', candidate });
  await flush();
  expect(actor.getSnapshot().context.application?.version).toBe('100');
  expect(actor.getSnapshot().matches({ observation: { connected: 'ready' } })).toBe(true);
  // The transport is lost: generation 2 is published while it cannot read.
  // Generation 1's observation is retired into stale presentation data at once.
  transport(actor, 'stale', 2);
  await flush();
  expect(actor.getSnapshot().matches({ observation: 'offline' })).toBe(true);
  expect(actor.getSnapshot().context.application).toBeUndefined();
  expect(actor.getSnapshot().context.staleApplication?.version).toBe('100');
  expect(reads).toHaveLength(1);
  // Reconnecting and resynchronizing stay inside generation 2 and still cannot
  // read: nothing is fabricated, nothing is polled.
  transport(actor, 'reconnecting', 2);
  await flush();
  expect(reads).toHaveLength(1);
  transport(actor, 'resynchronizing', 2);
  await flush();
  expect(reads).toHaveLength(1);
  expect(actor.getSnapshot().matches({ observation: 'offline' })).toBe(true);
  // Generation 2 becomes connected: it owes, and starts, exactly one read.
  transport(actor, 'connected', 2);
  await flush();
  expect(reads).toHaveLength(2);
  expect(actor.getSnapshot().matches({ observation: { connected: { loading: 'owed' } } })).toBe(true);
  // Application versions are a runtime counter of one process, so generation
  // 2's version 3 is not "older" than generation 1's 100: were they compared,
  // 100 would win. There is nothing to compare across the boundary at all.
  reads[1].resolve({ ...cfg3SourceApplication(), version: '3', candidate: otherCandidate });
  await flush();
  const snapshot = actor.getSnapshot();
  expect(snapshot.context.application?.version).toBe('3');
  expect(snapshot.context.application?.candidate).toEqual(otherCandidate);
  expect(snapshot.context.staleApplication).toBeUndefined();
  expect(snapshot.matches({ observation: { connected: 'ready' } })).toBe(true);
  // Settled: no further read without a new trigger.
  await flush();
  expect(reads).toHaveLength(2);
});

it('R13b a generation published already connected ends the old span and starts exactly one read of the new one', async () => {
  const { actor, reads } = scriptedSession();
  reads[0].resolve({ ...cfg3SourceApplication(), version: '100', candidate });
  await flush();
  transport(actor, 'connected', 2);
  await flush();
  expect(reads).toHaveLength(2);
  expect(actor.getSnapshot().context.application).toBeUndefined();
  expect(actor.getSnapshot().context.staleApplication?.version).toBe('100');
  reads[1].resolve({ ...cfg3SourceApplication(), version: '3', candidate: otherCandidate });
  await flush();
  expect(actor.getSnapshot().context.application?.version).toBe('3');
  expect(reads).toHaveLength(2);
});

it('R13b triggers absorbed while the transport cannot read coalesce into the one read the connected span owes', async () => {
  const { actor, reads } = scriptedSession('reconnecting', 2);
  expect(reads).toHaveLength(0);
  // An explicit refresh, a native publication and a Session snapshot change,
  // all while nothing can read: each is answered by the read the span owes.
  actor.send({ type: 'REFRESH' });
  actor.send({ type: 'TRANSPORT', connection: 'resynchronizing', generation: 2, publication: '7' });
  actor.send({ type: 'TRANSPORT', connection: 'resynchronizing', generation: 2, publication: '7', snapshot: {} as never });
  await flush();
  expect(reads).toHaveLength(0);
  // Becoming connected carries a newer publication in the same transport: one
  // read owner, not a reconnect read plus a publication read.
  actor.send({ type: 'TRANSPORT', connection: 'connected', generation: 2, publication: '8' });
  await flush();
  expect(reads).toHaveLength(1);
  reads[0].resolve({ ...cfg3SourceApplication(), version: '8' });
  await flush();
  expect(actor.getSnapshot().matches({ observation: { connected: 'ready' } })).toBe(true);
  expect(reads).toHaveLength(1);
});

it('R13b inside one connected span a newer publication supersedes the read in flight structurally, and nothing polls', async () => {
  const { actor, reads } = scriptedSession();
  expect(reads).toHaveLength(1);
  // The same publication delivered again is not a trigger.
  actor.send({ type: 'TRANSPORT', connection: 'connected', generation: 1 });
  await flush();
  expect(reads).toHaveLength(1);
  actor.send({ type: 'TRANSPORT', connection: 'connected', generation: 1, publication: '5' });
  await flush();
  expect(reads).toHaveLength(2);
  // The superseded read was stopped: it resolving later publishes nothing.
  reads[1].resolve({ ...cfg3SourceApplication(), version: '5' });
  await flush();
  reads[0].resolve({ ...cfg3SourceApplication(), version: '4', candidate });
  await flush();
  expect(actor.getSnapshot().context.application?.version).toBe('5');
  expect(actor.getSnapshot().context.application?.candidate).toBeNull();
  expect(reads).toHaveLength(2);
});

it.each(['resolves', 'rejects'] as const)('R13b an old-generation read that %s after the replacement publishes neither an application nor a read failure', async outcome => {
  const { actor, reads } = scriptedSession();
  reads[0].resolve({ ...cfg3SourceApplication(), version: '100' });
  await flush();
  actor.send({ type: 'REFRESH' });
  await flush();
  expect(reads).toHaveLength(2);
  transport(actor, 'stale', 2);
  await flush();
  if (outcome === 'resolves') reads[1].resolve({ ...cfg3SourceApplication(), version: '101', candidate });
  else reads[1].reject(new Error('old-generation read failed'));
  await flush();
  const snapshot = actor.getSnapshot();
  expect(snapshot.matches({ observation: 'offline' })).toBe(true);
  expect(snapshot.context.application).toBeUndefined();
  expect(snapshot.context.staleApplication?.version).toBe('100');
  expect(snapshot.context.readError).toBe('');
  expect(reads).toHaveLength(2);
});

it.each(['resolves', 'rejects'] as const)('R13b an adoption submitted on a replaced generation is an unknown outcome at the replacement, and its late reply that %s settles nothing', async outcome => {
  const { actor, reads, adoptions } = scriptedSession();
  reads[0].resolve({ ...cfg3SourceApplication(), version: '100', candidate });
  await flush();
  actor.send({ type: 'ADOPT', candidate });
  await flush();
  expect(adoptionInFlight(actor.getSnapshot())).toBe(true);
  transport(actor, 'stale', 2);
  await flush();
  // The replacement itself classifies the adoption: it may have committed, and
  // its reply can no longer arrive on this connection.
  expect(actor.getSnapshot().matches({ adoption: 'uncertain' })).toBe(true);
  expect(actor.getSnapshot().context.adoptionError).toContain('uncertain');
  expect(adoptionInFlight(actor.getSnapshot())).toBe(false);
  if (outcome === 'resolves') adoptions[0].resolve();
  else adoptions[0].reject(new Error('NotReady'));
  await flush();
  // No adoption settlement, no reread raised by it and no read of any kind
  // while the transport cannot read.
  expect(actor.getSnapshot().matches({ adoption: 'uncertain', observation: 'offline' })).toBe(true);
  expect(actor.getSnapshot().context.adoptionError).not.toContain('NotReady');
  expect(reads).toHaveLength(1);
  // The reconnected span's own owed read is the authoritative reread; the
  // adoption is never replayed.
  transport(actor, 'connected', 2);
  await flush();
  expect(reads).toHaveLength(2);
  reads[1].resolve({ ...cfg3SourceApplication(), version: '1' });
  await flush();
  expect(actor.getSnapshot().matches({ adoption: 'uncertain', observation: { connected: 'ready' } })).toBe(true);
  expect(adoptions).toHaveLength(1);
});

it('R13b the end of a connected span never re-enters the adoption region: a settled adoption outcome survives it', async () => {
  const { actor, reads, adoptions } = scriptedSession();
  reads[0].resolve({ ...cfg3SourceApplication(), version: '100', candidate });
  await flush();
  actor.send({ type: 'ADOPT', candidate });
  await flush();
  adoptions[0].reject(new Error('NotReady'));
  await flush();
  reads[1].resolve({ ...cfg3SourceApplication(), version: '101', candidate });
  await flush();
  expect(actor.getSnapshot().matches({ adoption: 'rejected' })).toBe(true);
  transport(actor, 'stale', 2);
  transport(actor, 'connected', 2);
  await flush();
  expect(actor.getSnapshot().matches({ adoption: 'rejected', observation: { connected: 'loading' } })).toBe(true);
  expect(actor.getSnapshot().context.adoptionError).toContain('NotReady');
  expect(adoptions).toHaveLength(1);
});

it('R13b an adoption reread of a replaced generation settles nothing, and its adoption transaction ends at the replacement', async () => {
  const { actor, reads, adoptions } = scriptedSession();
  reads[0].resolve({ ...cfg3SourceApplication(), version: '100', candidate });
  await flush();
  actor.send({ type: 'ADOPT', candidate });
  await flush();
  adoptions[0].resolve();
  await flush();
  expect(actor.getSnapshot().matches({ observation: { connected: { loading: 'adoptionReread' } } })).toBe(true);
  expect(adoptionInFlight(actor.getSnapshot())).toBe(true);
  transport(actor, 'stale', 2);
  await flush();
  expect(adoptionInFlight(actor.getSnapshot())).toBe(false);
  reads[1].resolve({ ...cfg3SourceApplication(), version: '101' });
  await flush();
  expect(actor.getSnapshot().context.application).toBeUndefined();
  expect(actor.getSnapshot().context.staleApplication?.version).toBe('100');
  expect(reads).toHaveLength(2);
});

it('R13c an obsolete result of the same connection generation still cannot regress the observation', async () => {
  const { actor, reads } = scriptedSession();
  reads[0].resolve({ ...cfg3SourceApplication(), version: '3', candidate: otherCandidate });
  await flush();
  expect(actor.getSnapshot().context.application?.version).toBe('3');
  // Same connected span, same application-version domain: version 2 is
  // genuinely older and never replaces version 3.
  actor.send({ type: 'REFRESH' });
  await flush();
  reads[1].resolve({ ...cfg3SourceApplication(), version: '2', candidate });
  await flush();
  const snapshot = actor.getSnapshot();
  expect(snapshot.context.application?.version).toBe('3');
  expect(snapshot.context.application?.candidate).toEqual(otherCandidate);
  expect(snapshot.matches({ observation: { connected: 'ready' } })).toBe(true);
});

// ── 13c. The configuration system owns actor lifetime ───────────────────────
//
// An actor retained only for a transaction in flight is released exactly at
// that transaction's terminal point, observed by the system — never by
// polling, and never by waiting for some later, unrelated lifetime change.

/** An App Server client whose every request is released explicitly by the
 * test, and whose App Server authority and connection generation the test
 * replaces. Like the real client, each replacement publishes a new snapshot
 * and synchronously notifies every subscriber, so the configuration system
 * observes it at that publication — the test never has to look an actor up to
 * make a lifetime change visible. Every request records the authority it was
 * issued under. */
function scriptedClient() {
  let view: ClientView = {
    endpoint: 'ws://native.invalid', authorityRevision: 1, connection: 'connected', generation: 1,
    sessions: [], views: {}, uncertain: [], interactionOperations: {},
  };
  const listeners = new Set<() => void>();
  const publish = (patch: Partial<ClientView>) => {
    view = { ...view, ...patch };
    for (const listener of listeners) listener();
  };
  const requests: { method: string; authority?: number; resolve: (value: unknown) => void; reject: (reason?: unknown) => void }[] = [];
  const client = {
    getSnapshot: () => view,
    subscribe: (listener: () => void) => { listeners.add(listener); return () => { listeners.delete(listener); }; },
    request: ({ method }: { method: string }) => {
      const gate = deferred<unknown>();
      requests.push({ method, authority: view.authorityRevision, ...gate });
      return gate.promise;
    },
  } as unknown as AppServerClient;
  return {
    system: new ConfigurationSystem(client),
    pending: (method: string) => requests.filter(request => request.method === method),
    /** Every request issued under one exact authority revision. */
    issuedUnder: (authority: number) => requests.filter(request => request.authority === authority).map(request => request.method),
    subscribers: () => listeners.size,
    replaceAuthority: () => publish({ authorityRevision: (view.authorityRevision ?? 0) + 1 }),
    replaceGeneration: () => publish({ generation: view.generation + 1 }),
    /** Publish one transport transition exactly as the real client does. */
    publish,
    /** The real client's reconnect sequence: the lost transport publishes the
     * next generation already stale, and that same generation then reconnects,
     * resynchronizes and finally becomes connected. `before` runs just before
     * the generation is published connected. */
    reconnect: (before: () => void = () => {}) => {
      publish({ connection: 'stale', generation: view.generation + 1 });
      publish({ connection: 'reconnecting', configuration: {} });
      publish({ connection: 'resynchronizing' });
      before();
      publish({ connection: 'connected' });
    },
    transport: () => view,
  };
}

const PROVIDER_SECRET = 'R26-PROVIDER-SECRET';
const providerIdentity = JSON.stringify({ kind: 'config', mutation: { unit: 'provider', id: 'secret', authored: null } });
const providerWrite = { base_url: 'https://native.invalid', credential: { kind: 'literal', value: PROVIDER_SECRET } } as const;
const providerMutation: SourceMutation = { kind: 'config', mutation: { unit: 'provider', id: 'secret', authored: providerWrite } };
const retainedBy = (system: ConfigurationSystem) => JSON.stringify(system.transactionOwners().map(owner => owner.retainedState()));

/** One User Settings target of the current authority, attached and observed at
 * r1, holding a dirty Provider literal credential draft. */
async function observedTarget(native: ReturnType<typeof scriptedClient>) {
  const actor = native.system.settingsTarget(userSettingsTarget, () => undefined);
  actor.send({ type: 'ATTACH' });
  await flush();
  native.pending('configuration/sourcesRead').at(-1)!.resolve({ projection: projection('r1') });
  await flush();
  actor.send({ type: 'UNIT.EDIT', identity: providerIdentity, selector: revisionSelector(providerMutation), revision: 'r1', value: providerWrite });
  return actor;
}

it('R26 the authority replacement itself stops and drops a target with no native mutation in flight, its dirty secret-bearing draft with it', async () => {
  const native = scriptedClient();
  const old = await observedTarget(native);
  const oldUnit = old.getSnapshot().context.units[providerIdentity];
  // The unsaved literal credential is the live editing draft of the old authority.
  const [oldOwner] = native.system.transactionOwners();
  expect(retainedBy(native.system)).toContain(PROVIDER_SECRET);
  // Settings is closed: nothing holds the target but the system.
  old.send({ type: 'DETACH' });
  native.replaceAuthority();
  // Linearization point: the authority publication. No actor is looked up, no
  // Settings is opened and no promise turn is drained before these assertions,
  // so they hold only if the transition itself retired the old lifetime.
  // Nothing of it crossed the native submission boundary, so nothing of it may
  // outlive the replacement: it is stopped, its transactions with it.
  expect(old.getSnapshot().status).toBe('stopped');
  expect(oldUnit.getSnapshot().status).toBe('stopped');
  // The old transaction owner is unreachable, and the secret-bearing draft is
  // retained nowhere.
  expect(native.system.transactionOwners()).toHaveLength(0);
  expect(native.system.transactionOwners()).not.toContain(oldOwner);
  expect(retainedBy(native.system)).not.toContain(PROVIDER_SECRET);
  // A later lookup finds a replacement that starts from nothing: the draft is
  // never migrated.
  const replacement = native.system.settingsTarget(userSettingsTarget, () => undefined);
  expect(replacement).not.toBe(old);
  expect(replacement.getSnapshot().context.units).toEqual({});
  expect(retainedBy(native.system)).toBe('[[]]');
  expect(native.pending('configuration/sourceWrite')).toHaveLength(0);
  expect(native.issuedUnder(2)).toEqual([]);
});

it.each(['acknowledged', 'conflicted', 'rejected', 'uncertain'] as const)('R26 an authority replacement retains a target with a native mutation in flight only until that mutation settles, when it is %s', async outcome => {
  const native = scriptedClient();
  const old = await observedTarget(native);
  const commits = { count: 0 };
  old.system.inspect(inspection => { if (inspection.type === '@xstate.event' && inspection.event.type === 'COMMITTED') commits.count += 1; });
  old.send({ type: 'UNIT.SUBMIT', identity: providerIdentity, selector: revisionSelector(providerMutation), revision: 'r1', mutation: providerMutation });
  await flush();
  expect(native.pending('configuration/sourceWrite')).toHaveLength(1);
  const oldUnit = old.getSnapshot().context.units[providerIdentity];
  const [oldOwner] = native.system.transactionOwners();
  native.replaceAuthority();
  // Linearization point: the authority publication, with no replacement actor
  // requested. The mutation already crossed the native submission boundary, so
  // the old lifetime is retained — detached and inert — for exactly that
  // settlement and nothing else.
  expect(old.getSnapshot().status).toBe('active');
  expect(old.getSnapshot().matches({ authority: 'suspended' })).toBe(true);
  expect(old.getSnapshot().matches({ mutation: 'submitting' })).toBe(true);
  expect(native.system.transactionOwners()).toEqual([oldOwner]);
  const write = native.pending('configuration/sourceWrite')[0];
  if (outcome === 'acknowledged') write.resolve({ projection: projection('r2') });
  else if (outcome === 'conflicted') write.reject(new RpcFailure({ code: -32000, message: 'Conflict', data: { kind: 'source_conflict', scope: 'user', expected: 'r1', actual: 'r-external' } }));
  else if (outcome === 'rejected') write.reject(new RpcFailure({ code: -32602, message: 'Invalid provider' }));
  else write.reject(new OutcomeUncertain());
  await flush();
  // The old outcome settles the exact transaction that submitted it…
  if (outcome === 'acknowledged') {
    expect(oldUnit.getSnapshot().matches({ mutation: 'acknowledged' })).toBe(true);
    expect(oldUnit.getSnapshot().context.submitted?.committed).toBe('r2');
    expect(commits.count).toBe(1);
  } else {
    expect(oldUnit.getSnapshot().matches({ mutation: 'unconfirmed' })).toBe(true);
    expect(commits.count).toBe(0);
  }
  expect(mutationOutcome(old.getSnapshot()).kind).toBe({ acknowledged: 'committed', conflicted: 'conflict', rejected: 'rejected', uncertain: 'uncertain' }[outcome]);
  // …and that settlement is the retained lifetime's terminal point: the system
  // stops and drops it at once, with no lookup and no further replacement.
  expect(old.getSnapshot().status).toBe('stopped');
  expect(oldUnit.getSnapshot().status).toBe('stopped');
  expect(native.system.transactionOwners()).toHaveLength(0);
  expect(retainedBy(native.system)).not.toContain(PROVIDER_SECRET);
  // The mutation left the browser exactly once, and the settling old lifetime
  // issued nothing at all — no post-commit read, no unknown-outcome reread —
  // through the replacement authority.
  expect(native.pending('configuration/sourceWrite')).toHaveLength(1);
  expect(native.pending('configuration/sourcesRead')).toHaveLength(1);
  expect(native.issuedUnder(2)).toEqual([]);
  // A replacement looked up afterwards is isolated from the old outcome.
  const isolated = native.system.settingsTarget(userSettingsTarget, () => undefined).getSnapshot();
  expect(isolated.context.units).toEqual({});
  expect(mutationOutcome(isolated)).toEqual({ kind: 'none' });
  expect(isolated.context.rejection).toBeUndefined();
});

it('R26 the configuration system observes its client through exactly one subscription, and an unchanged authority retires nothing', async () => {
  const native = scriptedClient();
  expect(native.subscribers()).toBe(1);
  const target = await observedTarget(native);
  const session = native.system.sessionConfiguration('session-1');
  // Publications that leave (endpoint, authorityRevision) unchanged are not
  // lifetime changes.
  native.replaceGeneration();
  expect(target.getSnapshot().status).toBe('active');
  expect(session.getSnapshot().status).toBe('active');
  expect(native.system.settingsTarget(userSettingsTarget, () => undefined)).toBe(target);
  expect(native.system.sessionConfiguration('session-1')).toBe(session);
  native.replaceAuthority();
  expect(target.getSnapshot().status).toBe('stopped');
  expect(session.getSnapshot().status).toBe('stopped');
  expect(native.subscribers()).toBe(1);
});

/** A Session actor of the current authority, held by one presentation, observed
 * with a candidate and with an adoption of it in flight. */
async function adoptingSession(native: ReturnType<typeof scriptedClient>) {
  const actor = native.system.sessionConfiguration('session-1');
  native.system.retainSession(actor);
  await flush();
  native.pending('session/configuration')[0].resolve({ application: { ...cfg3SourceApplication(), candidate } });
  await flush();
  actor.send({ type: 'ADOPT', candidate });
  await flush();
  expect(native.pending('session/adoptConfiguration')).toHaveLength(1);
  return actor;
}

it.each([
  ['accepted', 'observed'],
  ['accepted', 'failed'],
  ['uncertain', 'observed'],
  ['rejected', 'failed'],
] as const)('R27 a Session actor no presentation holds is released exactly at its adoption transaction terminal point, when adoption is %s and its reread %s', async (adoption, reread) => {
  const native = scriptedClient();
  const actor = await adoptingSession(native);
  // The last presentation leaves while the adoption is in flight.
  native.system.releaseSession(actor);
  expect(actor.getSnapshot().status).toBe('active');
  expect(native.system.sessionConfiguration('session-1')).toBe(actor);
  const response = native.pending('session/adoptConfiguration')[0];
  if (adoption === 'accepted') response.resolve({});
  else if (adoption === 'uncertain') response.reject(new OutcomeUncertain());
  else response.reject(new Error('NotReady'));
  await flush();
  // The native response alone is not the terminal point: the authoritative
  // reread it owes is part of the same adoption transaction and still settles
  // under this Session lifetime.
  const answered = actor.getSnapshot();
  expect(answered.status).toBe('active');
  expect(answered.matches({ adoption: adoption === 'accepted' ? 'idle' : adoption })).toBe(true);
  expect(answered.matches({ observation: { connected: { loading: 'adoptionReread' } } })).toBe(true);
  expect(native.pending('session/configuration')).toHaveLength(2);
  const rereadGate = native.pending('session/configuration')[1];
  if (reread === 'observed') rereadGate.resolve({ application: cfg3SourceApplication() });
  else rereadGate.reject(new Error('configuration read unavailable'));
  await flush();
  // The reread settled: that is the terminal point, and the system stops and
  // removes the actor at once, with no second release.
  const terminal = actor.getSnapshot();
  expect(terminal.status).toBe('stopped');
  expect(terminal.matches({ observation: { connected: reread === 'observed' ? 'ready' : 'failed' } })).toBe(true);
  // Adoption was never replayed, whatever its outcome, and nothing reread twice.
  expect(native.pending('session/adoptConfiguration')).toHaveLength(1);
  expect(native.pending('session/configuration')).toHaveLength(2);
  // A later lookup is a new Session lifetime, which owes its own first read.
  expect(native.system.sessionConfiguration('session-1')).not.toBe(actor);
  expect(native.pending('session/configuration')).toHaveLength(3);
});

it.each(['stays attached', 'leaves again'] as const)('R27 a presentation that attaches before the adoption settles and %s governs release', async holder => {
  const native = scriptedClient();
  const actor = await adoptingSession(native);
  native.system.releaseSession(actor);
  // A new presentation holds the actor before the adoption settles.
  native.system.retainSession(actor);
  if (holder === 'leaves again') native.system.releaseSession(actor);
  native.pending('session/adoptConfiguration')[0].resolve({});
  await flush();
  native.pending('session/configuration')[1].resolve({ application: cfg3SourceApplication() });
  await flush();
  if (holder === 'stays attached') {
    // A held actor outlives the adoption's terminal point…
    expect(actor.getSnapshot().status).toBe('active');
    expect(native.system.sessionConfiguration('session-1')).toBe(actor);
    // …and its last holder leaving with nothing in flight releases it at once.
    native.system.releaseSession(actor);
  }
  expect(actor.getSnapshot().status).toBe('stopped');
  expect(native.system.sessionConfiguration('session-1')).not.toBe(actor);
  expect(native.pending('session/adoptConfiguration')).toHaveLength(1);
});

it.each([
  ['accepted', 'released'],
  ['uncertain', 'released'],
  ['rejected', 'released'],
  ['accepted', 'held'],
  ['answered, its reread pending,', 'released'],
] as const)('R30 an authority replacement stops a Session actor at once, and its late %s adoption response starts nothing through the replacement, when the actor is %s', async (adoption, holder) => {
  const native = scriptedClient();
  const actor = await adoptingSession(native);
  if (adoption === 'answered, its reread pending,') {
    native.pending('session/adoptConfiguration')[0].resolve({});
    await flush();
    expect(actor.getSnapshot().matches({ observation: { connected: { loading: 'adoptionReread' } } })).toBe(true);
  }
  // The last presentation leaves: the actor is alive only because its adoption
  // transaction is in flight.
  if (holder === 'released') native.system.releaseSession(actor);
  expect(actor.getSnapshot().status).toBe('active');
  expect(adoptionInFlight(actor.getSnapshot())).toBe(true);
  const reads = native.pending('session/configuration').length;
  native.replaceAuthority();
  // Linearization point: the authority publication, with no replacement Session
  // actor requested. The old Session actor is stopped at once, and with it the
  // invoked adoption and reread actors.
  expect(actor.getSnapshot().status).toBe('stopped');
  // The obsolete authority answers late. A stopped actor has no completion path
  // for it: no `ADOPTION.REREAD`, so no `session/configuration`, and never an
  // adoption replay — nothing at all is issued through the replacement.
  if (adoption === 'accepted') native.pending('session/adoptConfiguration')[0].resolve({});
  else if (adoption === 'uncertain') native.pending('session/adoptConfiguration')[0].reject(new OutcomeUncertain());
  else if (adoption === 'rejected') native.pending('session/adoptConfiguration')[0].reject(new Error('NotReady'));
  else native.pending('session/configuration').at(-1)!.resolve({ application: cfg3SourceApplication() });
  await flush();
  expect(native.pending('session/configuration')).toHaveLength(reads);
  expect(native.pending('session/adoptConfiguration')).toHaveLength(1);
  expect(native.issuedUnder(2)).toEqual([]);
  // A presentation leaving afterwards finds nothing left to release.
  if (holder === 'held') native.system.releaseSession(actor);
  expect(native.issuedUnder(2)).toEqual([]);
  // The replacement lifetime starts from nothing: its own actor owes, and
  // issues, exactly the one read of its own connected span.
  const replacement = native.system.sessionConfiguration('session-1');
  expect(replacement).not.toBe(actor);
  expect(replacement.getSnapshot().matches({ adoption: 'idle', observation: { connected: 'loading' } })).toBe(true);
  expect(native.issuedUnder(2)).toEqual(['session/configuration']);
});

it('R31 a connection generation replacement inside one authority keeps every lifetime and retires only generation-scoped observation', async () => {
  const native = scriptedClient();
  const target = await observedTarget(native);
  const unit = target.getSnapshot().context.units[providerIdentity];
  target.send({ type: 'UNIT.SUBMIT', identity: providerIdentity, selector: revisionSelector(providerMutation), revision: 'r1', mutation: providerMutation });
  await flush();
  const session = await adoptingSession(native);
  const held = native.system.sessionConfiguration('session-2');
  native.system.retainSession(held);
  await flush();
  native.pending('session/configuration')[1].resolve({ application: cfg3SourceApplication() });
  await flush();
  native.system.releaseSession(session);
  native.replaceGeneration();
  // Same (endpoint, authorityRevision): the system replaces and detaches
  // nothing. Authority, presentation and transaction lifetimes all survive.
  expect(target.getSnapshot().status).toBe('active');
  expect(target.getSnapshot().matches({ authority: 'attached', mutation: 'submitting' })).toBe(true);
  expect(unit.getSnapshot().status).toBe('active');
  expect(held.getSnapshot().status).toBe('active');
  expect(native.system.settingsTarget(userSettingsTarget, () => undefined)).toBe(target);
  expect(native.system.sessionConfiguration('session-2')).toBe(held);
  // The system delivers the replacement to every live actor at the client
  // publication itself — no presentation forwards it — and each actor's own
  // generation fencing retires exactly what the old generation observed.
  await flush();
  expect(target.getSnapshot().context.observation).toBeUndefined();
  expect(native.pending('configuration/sourcesRead')).toHaveLength(2);
  expect(held.getSnapshot().context.application).toBeUndefined();
  expect(held.getSnapshot().context.staleApplication).toBeDefined();
  // The adoption submitted on the replaced connection is an unknown outcome
  // at the replacement: nothing holds its Session actor and nothing is in
  // flight, so the actor is released at that same publication.
  expect(session.getSnapshot().status).toBe('stopped');
  expect(session.getSnapshot().matches({ adoption: 'uncertain' })).toBe(true);
  expect(native.system.sessionConfiguration('session-2')).toBe(held);
  // The write submitted on the old generation still settles its exact
  // transaction, and the actor stays live for its whole authority.
  native.pending('configuration/sourceWrite')[0].resolve({ projection: projection('r2') });
  await flush();
  expect(unit.getSnapshot().matches({ mutation: 'acknowledged' })).toBe(true);
  expect(unit.getSnapshot().context.submitted?.committed).toBe('r2');
  expect(target.getSnapshot().status).toBe('active');
  expect(native.system.transactionOwners()).toHaveLength(1);
  // A late reply to the old-generation adoption settles and rereads nothing.
  const reads = native.pending('session/configuration').length;
  native.pending('session/adoptConfiguration')[0].resolve({});
  await flush();
  expect(native.pending('session/configuration')).toHaveLength(reads);
  expect(native.pending('configuration/sourceWrite')).toHaveLength(1);
  expect(native.pending('session/adoptConfiguration')).toHaveLength(1);
  expect(native.issuedUnder(2)).toEqual([]);
});

// ── 13d. Reconnect ownership is structural and needs no presentation ────────

it('R33 a headless Session observation recovers after a reconnect with no presentation, no Session attachment and no snapshot change', async () => {
  const native = scriptedClient();
  // Nothing holds this actor and the Session is not attached: no view, no
  // snapshot, no presentation of any kind.
  const actor = native.system.sessionConfiguration('session-1');
  expect(native.transport().views).toEqual({});
  expect(native.pending('session/configuration')).toHaveLength(1);
  native.pending('session/configuration')[0].resolve({ application: { ...cfg3SourceApplication(), version: '100' } });
  await flush();
  expect(actor.getSnapshot().context.application?.version).toBe('100');
  // Transport lost: generation 2 is published stale. No read.
  native.publish({ connection: 'stale', generation: 2 });
  await flush();
  expect(actor.getSnapshot().context.application).toBeUndefined();
  expect(actor.getSnapshot().context.staleApplication?.version).toBe('100');
  expect(native.pending('session/configuration')).toHaveLength(1);
  native.publish({ connection: 'reconnecting', configuration: {} });
  await flush();
  expect(native.pending('session/configuration')).toHaveLength(1);
  native.publish({ connection: 'resynchronizing' });
  await flush();
  expect(native.pending('session/configuration')).toHaveLength(1);
  // Connected: exactly one read, at that publication.
  native.publish({ connection: 'connected' });
  expect(native.pending('session/configuration')).toHaveLength(2);
  native.pending('session/configuration')[1].resolve({ application: { ...cfg3SourceApplication(), version: '3' } });
  await flush();
  expect(actor.getSnapshot().context.application?.version).toBe('3');
  expect(actor.getSnapshot().matches({ observation: { connected: 'ready' } })).toBe(true);
  // Unrelated client publications afterwards are not triggers: nothing polls.
  native.publish({ sessions: [] });
  native.publish({ uncertain: [] });
  await flush();
  expect(native.pending('session/configuration')).toHaveLength(2);
  expect(native.transport().views).toEqual({});
});

it('R33 a native publication and a Session snapshot change delivered during the reconnect coalesce into the one connected read', async () => {
  const native = scriptedClient();
  const actor = native.system.sessionConfiguration('session-1');
  native.pending('session/configuration')[0].resolve({ application: { ...cfg3SourceApplication(), version: '100' } });
  await flush();
  native.reconnect(() => {
    // While resynchronizing: a publication for this Session and its
    // re-attachment snapshot. Neither can read yet; both are answered by the
    // one read the connected span owes.
    native.publish({ configuration: { 'session-1': { ...cfg3SourceApplication(), version: '4' } } });
    native.publish({ views: { 'session-1': { id: 'session-1', attachmentIntent: 'wanted', attachment: 'attached', snapshot: {} as never } } });
    actor.send({ type: 'REFRESH' });
  });
  expect(native.pending('session/configuration')).toHaveLength(2);
  native.pending('session/configuration')[1].resolve({ application: { ...cfg3SourceApplication(), version: '4' } });
  await flush();
  expect(actor.getSnapshot().context.application?.version).toBe('4');
  expect(native.pending('session/configuration')).toHaveLength(2);
  // Inside the connected span a newer publication is a trigger again.
  native.publish({ configuration: { 'session-1': { ...cfg3SourceApplication(), version: '5' } } });
  expect(native.pending('session/configuration')).toHaveLength(3);
});

it('R33 a Settings target attached before a reconnect reads exactly once when its generation becomes connected, and no unrelated client publication retries a failed read', async () => {
  const native = scriptedClient();
  const target = native.system.settingsTarget(userSettingsTarget, () => undefined);
  target.send({ type: 'ATTACH' });
  await flush();
  native.pending('configuration/sourcesRead')[0].resolve({ projection: projection('r1') });
  await flush();
  native.reconnect();
  expect(native.pending('configuration/sourcesRead')).toHaveLength(2);
  expect(target.getSnapshot().context.generation).toBe(2);
  // That read fails while connected: the failure stands until a real trigger.
  native.pending('configuration/sourcesRead')[1].reject(new Error('configuration read unavailable'));
  await flush();
  expect(target.getSnapshot().context.readError).toContain('configuration read unavailable');
  native.publish({ sessions: [] });
  native.publish({ views: {} });
  // A Session's configuration publication replaces the client's whole
  // configuration map, and still concerns no source target.
  publishApplication(native, sessionApplication('session-1', '4'));
  await flush();
  expect(native.pending('configuration/sourcesRead')).toHaveLength(2);
  // This target's own source publication is one.
  publishApplication(native, { ...cfg3SourceApplication(), version: '9' });
  await flush();
  expect(native.pending('configuration/sourcesRead')).toHaveLength(3);
});

// ── 34. Target-local publication ownership ──────────────────────────────────
//
// A native publication obligation is a level-triggered fact about one exact
// application scope. The client mirrors every scope's publication in one map
// and replaces that map on each of them; only the publication of the exact
// source scope a Settings target owns may retry, refresh, unblock or advance
// that target's source reads.

/** Publish one native application exactly as the real client does: the entry
 * for its scope is replaced and the whole configuration map is a new object. */
function publishApplication(native: ReturnType<typeof scriptedClient>, application: ConfigurationApplication) {
  native.publish({ configuration: { ...native.transport().configuration, [application.scope]: application } });
}
const sessionApplication = (session: string, version: string): ConfigurationApplication =>
  ({ ...cfg3SourceApplication(), scope: session, version });
const workspaceApplication = (directory: string, version: string): ConfigurationApplication =>
  ({ ...cfg3SourceApplication({ kind: 'workspace', directory }), version });
/** A native projection of the Workspace source at `/workspace/A`: native names
 * the exact canonical target it read, and the application version it holds. */
const workspaceProjection = (revision: string, version: string) => {
  const source = projection(revision, workspaceApplication('/workspace/A', version));
  source.target = { kind: 'workspace', directory: '/workspace/A' };
  return source;
};

it('R34 a failed User Settings read is retried by its own source publication alone', async () => {
  const native = scriptedClient();
  const target = native.system.settingsTarget(userSettingsTarget, () => undefined);
  target.send({ type: 'ATTACH' });
  await flush();
  const reads = () => native.pending('configuration/sourcesRead');
  reads()[0].reject(new Error('configuration read unavailable'));
  await flush();
  expect(target.getSnapshot().matches({ authority: { attached: 'blocked' } })).toBe(true);
  expect(target.getSnapshot().context.observation).toBeUndefined();
  // A Session application, another Workspace's source application and a newer
  // Session version each replace the client's configuration map. None of them
  // is this target's publication, so none wakes the blocked read.
  publishApplication(native, sessionApplication('session-1', '1'));
  publishApplication(native, workspaceApplication('/workspace/A', '3'));
  publishApplication(native, sessionApplication('session-1', '2'));
  await flush();
  expect(reads()).toHaveLength(1);
  expect(target.getSnapshot().matches({ authority: { attached: 'blocked' } })).toBe(true);
  expect(target.getSnapshot().context.publication).toBeUndefined();
  // The exact User source publication creates the obligation: one read.
  publishApplication(native, { ...cfg3SourceApplication(), version: '2' });
  await flush();
  expect(reads()).toHaveLength(2);
  // Publications arriving while that read is in flight coalesce as a level:
  // unrelated ones change nothing, and a newer own version starts no second
  // read now — it survives the settlement below it instead.
  publishApplication(native, sessionApplication('session-2', '1'));
  publishApplication(native, { ...cfg3SourceApplication(), version: '3' });
  await flush();
  expect(reads()).toHaveLength(2);
  reads()[1].resolve({ projection: projection('r1', userApplication('2')) });
  await flush();
  expect(reads()).toHaveLength(3);
  reads()[2].resolve({ projection: projection('r2', userApplication('3')) });
  await flush();
  expect(reads()).toHaveLength(3);
  expect(target.getSnapshot().context.observation?.user.revision).toBe('r2');
  // Converged: unrelated publications still start nothing.
  publishApplication(native, sessionApplication('session-1', '3'));
  publishApplication(native, workspaceApplication('/workspace/B', '1'));
  await flush();
  expect(reads()).toHaveLength(3);
  // Explicit refresh still reads, as before.
  target.send({ type: 'REFRESH' });
  await flush();
  expect(reads()).toHaveLength(4);
});

/** A Product Host whose registration resolution and configuration operations
 * are each released explicitly by the test. */
function scriptedHost() {
  const resolutions: { id: string; resolve: (value: { cwd: string }) => void; reject: (reason?: unknown) => void }[] = [];
  const operations: { operation: WorkspaceConfigurationOperation; resolve: (value: WorkspaceConfigurationResult) => void; reject: (reason?: unknown) => void }[] = [];
  const host = {
    resolveWorkspace: (id: string) => { const gate = deferred<{ cwd: string }>(); resolutions.push({ id, ...gate }); return gate.promise; },
    configureWorkspace: (_id: string, _endpoint: string, operation: WorkspaceConfigurationOperation) => {
      const gate = deferred<WorkspaceConfigurationResult>(); operations.push({ operation, ...gate }); return gate.promise;
    },
  } as unknown as ProductHostWorkspaces;
  return { host, resolutions, operations, reads: () => operations.filter(entry => entry.operation.kind === 'read') };
}

it('R34 a failed Workspace Settings read is retried by the publication of its own Host-resolved source scope alone', async () => {
  const native = scriptedClient();
  const scripted = scriptedHost();
  const target = native.system.settingsTarget(workspaceSettingsTarget('wA', 'A'), () => scripted.host);
  target.send({ type: 'ATTACH' });
  await flush();
  // The first read asks the Product Host registration resolution — the one
  // authority that names this Workspace's canonical source directory — for
  // the source scope, before the read settles.
  expect(scripted.resolutions.map(entry => entry.id)).toEqual(['wA']);
  expect(scripted.reads()).toHaveLength(1);
  publishApplication(native, workspaceApplication('/workspace/A', '1'));
  await flush();
  // Unnamed, nothing is attributed to this target.
  expect(target.getSnapshot().context.publication).toBeUndefined();
  scripted.resolutions[0].resolve({ cwd: '/workspace/A' });
  await flush();
  // Named, its own standing publication level is delivered at once.
  expect(target.getSnapshot().context.publication?.scope).toBe('source:workspace:/workspace/A');
  scripted.reads()[0].reject(new Error('Workspace read unavailable'));
  await flush();
  expect(target.getSnapshot().matches({ authority: { attached: 'blocked' } })).toBe(true);
  // Another Workspace's source, a Session and the User source are all other
  // scopes' publications.
  publishApplication(native, workspaceApplication('/workspace/B', '5'));
  publishApplication(native, sessionApplication('session-1', '1'));
  publishApplication(native, { ...cfg3SourceApplication(), version: '7' });
  await flush();
  expect(scripted.reads()).toHaveLength(1);
  expect(target.getSnapshot().matches({ authority: { attached: 'blocked' } })).toBe(true);
  // The exact Workspace source publication creates the obligation.
  publishApplication(native, workspaceApplication('/workspace/A', '2'));
  await flush();
  expect(scripted.reads()).toHaveLength(2);
  // A named scope is never resolved again.
  expect(scripted.resolutions).toHaveLength(1);
  // While that read is in flight, other scopes change nothing and a newer own
  // version coalesces into exactly one follow-up read.
  publishApplication(native, workspaceApplication('/workspace/B', '6'));
  publishApplication(native, workspaceApplication('/workspace/A', '3'));
  await flush();
  expect(scripted.reads()).toHaveLength(2);
  scripted.reads()[1].resolve({ kind: 'read', projection: workspaceProjection('ws-1', '2') });
  await flush();
  expect(scripted.reads()).toHaveLength(3);
  scripted.reads()[2].resolve({ kind: 'read', projection: workspaceProjection('ws-2', '3') });
  await flush();
  expect(scripted.reads()).toHaveLength(3);
  publishApplication(native, workspaceApplication('/workspace/B', '7'));
  await flush();
  expect(scripted.reads()).toHaveLength(3);
});

it.each(['rejects transiently', 'is still unanswered'] as const)('R34 a successful Workspace read names its own publication scope when the Host resolution %s, so its exact source publication alone drives convergence', async resolution => {
  const native = scriptedClient();
  const scripted = scriptedHost();
  const target = native.system.settingsTarget(workspaceSettingsTarget('wA', 'A'), () => scripted.host);
  target.send({ type: 'ATTACH' });
  await flush();
  // Native has already published version 1 of the Workspace source; nothing
  // is attributed to the target while no answer names its scope.
  publishApplication(native, workspaceApplication('/workspace/A', '1'));
  await flush();
  expect(scripted.resolutions).toHaveLength(1);
  expect(scripted.reads()).toHaveLength(1);
  expect(target.getSnapshot().context.publication).toBeUndefined();
  if (resolution === 'rejects transiently') {
    scripted.resolutions[0].reject(new Error('Host resolution temporarily unavailable'));
    await flush();
    expect(target.getSnapshot().context.publication).toBeUndefined();
  }
  // The read itself succeeds. Native answered it with the exact canonical
  // target it read, and that — not the unanswered or failed naming RPC — names
  // the scope before the projection becomes authoritative.
  scripted.reads()[0].resolve({ kind: 'read', projection: workspaceProjection('ws-1', '1') });
  await flush();
  let snapshot = target.getSnapshot();
  expect(snapshot.context.observation?.user.revision).toBe('ws-1');
  expect(snapshot.context.publication).toEqual(workspaceApplication('/workspace/A', '1'));
  expect(snapshot.matches({ authority: { attached: 'idle' } })).toBe(true);
  expect(scripted.reads()).toHaveLength(1);
  if (resolution === 'is still unanswered') {
    // A late answer naming the same scope changes nothing.
    scripted.resolutions[0].resolve({ cwd: '/workspace/A' });
    await flush();
    expect(scripted.reads()).toHaveLength(1);
  }
  // Other scopes' publications never wake it.
  publishApplication(native, workspaceApplication('/workspace/B', '5'));
  publishApplication(native, sessionApplication('session-1', '1'));
  publishApplication(native, { ...cfg3SourceApplication(), version: '7' });
  await flush();
  expect(scripted.reads()).toHaveLength(1);
  // The exact Workspace source publishes version 2: exactly one authoritative
  // reread, with no refresh, reattach, reconnect or new naming request.
  const generation = target.getSnapshot().context.generation;
  publishApplication(native, workspaceApplication('/workspace/A', '2'));
  await flush();
  expect(scripted.reads()).toHaveLength(2);
  expect(scripted.resolutions).toHaveLength(1);
  expect(target.getSnapshot().context.generation).toBe(generation);
  // Bounded: the read that reaches version 2 converges and starts nothing.
  scripted.reads()[1].resolve({ kind: 'read', projection: workspaceProjection('ws-2', '2') });
  await flush();
  snapshot = target.getSnapshot();
  expect(snapshot.context.observation?.user.revision).toBe('ws-2');
  expect(snapshot.matches({ authority: { attached: 'idle' } })).toBe(true);
  expect(scripted.reads()).toHaveLength(2);
});

it('R34 a Workspace nothing has named attributes no publication, and the next successful read names it without the Host resolution', async () => {
  const native = scriptedClient();
  const scripted = scriptedHost();
  const target = native.system.settingsTarget(workspaceSettingsTarget('wA', 'A'), () => scripted.host);
  target.send({ type: 'ATTACH' });
  await flush();
  scripted.resolutions[0].reject(new Error('Host unavailable'));
  scripted.reads()[0].reject(new Error('Host unavailable'));
  await flush();
  expect(target.getSnapshot().matches({ authority: { attached: 'blocked' } })).toBe(true);
  // No scope is guessed from the Workspace id, its display name or a path, so
  // no publication — not even the one that would be its own — is attributed.
  publishApplication(native, workspaceApplication('/workspace/A', '1'));
  publishApplication(native, workspaceApplication('/workspace/wA', '1'));
  await flush();
  expect(scripted.reads()).toHaveLength(1);
  expect(target.getSnapshot().context.publication).toBeUndefined();
  // With no successful answer at all, recovery is explicit. The refresh asks
  // the Host to name the scope again, which fails again; the read succeeds and
  // names it itself.
  target.send({ type: 'REFRESH' });
  await flush();
  expect(scripted.reads()).toHaveLength(2);
  expect(scripted.resolutions).toHaveLength(2);
  scripted.resolutions[1].reject(new Error('Host unavailable'));
  scripted.reads()[1].resolve({ kind: 'read', projection: workspaceProjection('ws-1', '1') });
  await flush();
  expect(target.getSnapshot().context.publication).toEqual(workspaceApplication('/workspace/A', '1'));
  publishApplication(native, workspaceApplication('/workspace/A', '2'));
  await flush();
  expect(scripted.reads()).toHaveLength(3);
});

// ── 35. Target-wide source mutation admission ───────────────────────────────
//
// One target-wide native source mutation lifecycle. Once a mutation crosses
// the native submission boundary, no second one — from any semantic unit — may
// start until the target again owns a current authoritative observation issued
// after that mutation settled. Drafts stay independent and editable; nothing
// queues, replays or retries a refused submission.

const statusMutation: SourceMutation = { kind: 'config', mutation: { unit: 'agent_status', authored: { enabled: true } } };
const statusIdentity = JSON.stringify({ kind: 'config', mutation: { unit: 'agent_status', authored: null } });
const editStatus = (actor: ReturnType<typeof settingsActor>, revision: string) =>
  actor.send({ type: 'UNIT.EDIT', identity: statusIdentity, selector: revisionSelector(statusMutation), revision, value: { enabled: true } });
const submitStatus = (actor: ReturnType<typeof settingsActor>, revision: string) =>
  actor.send({ type: 'UNIT.SUBMIT', identity: statusIdentity, selector: revisionSelector(statusMutation), revision, mutation: statusMutation });
const admitted = (actor: ReturnType<typeof settingsActor>) => admitsSourceMutation(actor.getSnapshot().context);
/** Unit B still holds exactly its own untouched draft and never submitted. */
function expectStatusDraftIntact(actor: ReturnType<typeof settingsActor>) {
  const status = unitOf(actor, statusIdentity).getSnapshot();
  expect(status.matches({ intent: 'dirty' })).toBe(true);
  expect(status.matches({ mutation: 'idle' })).toBe(true);
  expect(status.context.draft).toEqual({ value: { enabled: true } });
  expect(status.context.base).toBe('r1');
}

it('R35 a mutation natively submitting refuses every other unit, which keeps its draft', async () => {
  const scripted = scriptedPort();
  const actor = settingsActor(scripted.port);
  await flush();
  scripted.reads[0].resolve(projection('r1'));
  await flush();
  edit(actor, ['read'], 'r1');
  editStatus(actor, 'r1');
  expect(admitted(actor)).toBe(true);
  submit(actor, 'r1');
  await flush();
  expect(scripted.writes).toHaveLength(1);
  expect(admitted(actor)).toBe(false);
  // Unit B is refused: the machine has no transition for it, so its
  // submission neither reaches native nor changes anything at all.
  const before = actor.getSnapshot();
  expect(before.can({ type: 'UNIT.SUBMIT', identity: statusIdentity, selector: revisionSelector(statusMutation), revision: 'r1', mutation: statusMutation })).toBe(false);
  submitStatus(actor, 'r1');
  await flush();
  expect(scripted.writes).toHaveLength(1);
  expect(actor.getSnapshot().context.submission?.identity).toBe(toolsIdentity);
  expectStatusDraftIntact(actor);
  // B stays editable while A is pending.
  actor.send({ type: 'UNIT.EDIT', identity: statusIdentity, selector: revisionSelector(statusMutation), revision: 'r1', value: { enabled: true } });
  expectStatusDraftIntact(actor);
  // A settles normally. Its acknowledgement alone admits nothing: the
  // observation this target holds predates the commit.
  scripted.writes[0].resolve({ acknowledgement: projection('r2') });
  await flush();
  expect(mutationOutcome(actor.getSnapshot())).toEqual({ kind: 'committed' });
  expect(admitted(actor)).toBe(false);
  expect(scripted.reads).toHaveLength(2);
  scripted.reads[1].resolve(projection('r2'));
  await flush();
  expect(mutationOutcome(actor.getSnapshot())).toEqual({ kind: 'saved', observed: true });
  expect(admitted(actor)).toBe(true);
  expect(scripted.writes).toHaveLength(1);
  expect(unitOf(actor, statusIdentity).getSnapshot().context.draft).toEqual({ value: { enabled: true } });
});

it('R35 a definitive commit awaiting its post-commit observation refuses a second unit fenced on the pre-commit revision', async () => {
  const scripted = scriptedPort();
  const actor = settingsActor(scripted.port);
  await flush();
  scripted.reads[0].resolve(projection('r1'));
  await flush();
  edit(actor, ['read'], 'r1');
  editStatus(actor, 'r1');
  submit(actor, 'r1');
  await flush();
  scripted.writes[0].resolve({ acknowledgement: projection('r2') });
  await flush();
  // A is definitively committed; its post-commit read is outstanding.
  expect(unitOf(actor).getSnapshot().matches({ mutation: 'acknowledged' })).toBe(true);
  expect(actor.getSnapshot().context.observation?.user.revision).toBe('r1');
  expect(actor.getSnapshot().context.unobservedSettlement).toMatchObject({ identity: toolsIdentity, committed: true, revision: 'r2' });
  expect(admitted(actor)).toBe(false);
  submitStatus(actor, 'r1');
  await flush();
  expect(scripted.writes).toHaveLength(1);
  expectStatusDraftIntact(actor);
  // The authoritative post-commit observation at r2 releases the barrier.
  scripted.reads[1].resolve(projection('r2'));
  await flush();
  expect(admitted(actor)).toBe(true);
  // B's pinned base still names r1, which the source moved on from: its CAS
  // is never silently rebased. The explicit review fences it on r2.
  expect(requiresReview(unitOf(actor, statusIdentity).getSnapshot())).toBe(true);
  actor.send({ type: 'UNIT.REVIEW', identity: statusIdentity });
  submitStatus(actor, 'r2');
  await flush();
  expect(scripted.writes).toHaveLength(2);
  expect(scripted.writes[1].expected).toBe('r2');
  expect(scripted.writes[1].mutation).toEqual(statusMutation);
});

it('R35 a pre-commit read adopted after a Workspace commit is current observation, yet still admits no second mutation', async () => {
  const scripted = scriptedPort(true);
  const actor = settingsActor(scripted.port, userApplication('1'), true);
  await flush();
  scripted.reads[0].resolve(projection('r1', userApplication('1')));
  await flush();
  edit(actor, ['read'], 'r1');
  editStatus(actor, 'r1');
  submit(actor, 'r1');
  await flush();
  // A newer publication supersedes the write-owned reread: a read is issued
  // before the commit, and the reservation is revoked.
  reconnect(actor, 1, userApplication('2'));
  await flush();
  expect(scripted.reads).toHaveLength(2);
  scripted.writes[0].resolve({ acknowledgement: projection('r2'), reread: { status: 'observed', projection: projection('r2', userApplication('2')) } });
  await flush();
  // The read issued before the commit answers with the pre-commit source.
  scripted.reads[1].resolve(projection('r1', userApplication('2')));
  await flush();
  expect(actor.getSnapshot().context.observation?.user.revision).toBe('r1');
  expect(actor.getSnapshot().context.readError).toBe('');
  expect(admitted(actor)).toBe(false);
  submitStatus(actor, 'r1');
  await flush();
  expect(scripted.writes).toHaveLength(1);
  expectStatusDraftIntact(actor);
  // The standing commit obligation drives the post-commit read.
  expect(scripted.reads).toHaveLength(3);
  scripted.reads[2].resolve(projection('r2', userApplication('2')));
  await flush();
  expect(admitted(actor)).toBe(true);
});

it('R35 a commit whose post-commit read fails exposes no pre-commit authority and replays nothing', async () => {
  const scripted = scriptedPort();
  const actor = settingsActor(scripted.port);
  await flush();
  scripted.reads[0].resolve(projection('r1'));
  await flush();
  edit(actor, ['read'], 'r1');
  editStatus(actor, 'r1');
  submit(actor, 'r1');
  await flush();
  scripted.writes[0].resolve({ acknowledgement: projection('r2') });
  await flush();
  scripted.reads[1].reject(new Error('post-commit read unavailable'));
  await flush();
  expect(mutationOutcome(actor.getSnapshot())).toEqual({ kind: 'saved', observed: false });
  expect(admitted(actor)).toBe(false);
  submitStatus(actor, 'r1');
  await flush();
  // Neither B nor a replay of A reaches native, and nothing retries on its own.
  expect(scripted.writes).toHaveLength(1);
  expect(scripted.reads).toHaveLength(2);
  expectStatusDraftIntact(actor);
  // Recovery is explicit.
  actor.send({ type: 'REFRESH' });
  await flush();
  scripted.reads[2].resolve(projection('r2'));
  await flush();
  expect(mutationOutcome(actor.getSnapshot())).toEqual({ kind: 'saved', observed: true });
  expect(admitted(actor)).toBe(true);
  expect(scripted.writes).toHaveLength(1);
});

it.each([
  ['conflict', new RpcFailure({ code: -32000, message: 'conflict', data: { kind: 'source_conflict', scope: 'user', expected: 'r1', actual: 'r9' } })],
  ['rejection', new Error('native refused')],
  ['unknown outcome', new OutcomeUncertain()],
] as const)('R35 a %s holds the barrier until its post-settlement read is adopted', async (_kind, failure) => {
  const scripted = scriptedPort();
  const actor = settingsActor(scripted.port);
  await flush();
  scripted.reads[0].resolve(projection('r1'));
  await flush();
  edit(actor, ['read'], 'r1');
  editStatus(actor, 'r1');
  submit(actor, 'r1');
  await flush();
  scripted.writes[0].reject(failure);
  await flush();
  // The failed write forced a post-settlement read; until it is adopted the
  // observation is known to predate the settlement.
  expect(scripted.reads).toHaveLength(2);
  expect(admitted(actor)).toBe(false);
  submitStatus(actor, 'r1');
  await flush();
  expect(scripted.writes).toHaveLength(1);
  expectStatusDraftIntact(actor);
  scripted.reads[1].resolve(projection('r1'));
  await flush();
  expect(admitted(actor)).toBe(true);
  submitStatus(actor, 'r1');
  await flush();
  expect(scripted.writes).toHaveLength(2);
  expect(scripted.writes[1].expected).toBe('r1');
});

// ── 37. Same-unit editing across a definitive commit ────────────────────────
//
// A definitive acknowledgement advances the unit's CAS base to the committed
// revision at once, while the authoritative projection every editor renders
// still carries the source before the commit. A draft begun in that window
// would be fenced on the new revision yet derived from the old value, so no
// CAS could stop it from restoring what the commit replaced. A semantic unit
// whose definitive commit still awaits authoritative observation is therefore
// not a valid source for a new same-unit draft; every other unit stays
// editable behind the target-wide submission barrier.

it('R37 a unit whose definitive commit awaits its observation refuses a new edit, and resumes from the observed source', async () => {
  const scripted = scriptedPort();
  const actor = settingsActor(scripted.port);
  await flush();
  scripted.reads[0].resolve(projection('r0'));
  await flush();
  edit(actor, ['read'], 'r0');
  submit(actor, 'r0');
  await flush();
  const first = unitOf(actor);
  scripted.writes[0].resolve({ acknowledgement: projection('r1') });
  await flush();
  let unit = first.getSnapshot();
  expect(awaitingCommitObservation(unit)).toBe(true);
  expect(unit.context).toMatchObject({ base: 'r1', observed: 'r0' });
  // The confirmed draft of the unchanged intent is gone.
  expect(unit.context.draft).toBeUndefined();
  expect(unit.matches({ intent: 'clean' })).toBe(true);
  const generation = unit.context.generation;
  // An edit derived from the pre-commit projection is refused by the unit
  // itself: no draft, no new intent generation, nothing written.
  edit(actor, ['write'], 'r0');
  await flush();
  unit = first.getSnapshot();
  expect(unit.context.draft).toBeUndefined();
  expect(unit.context.generation).toBe(generation);
  expect(unit.matches({ intent: 'clean', mutation: { acknowledged: 'awaitingObservation' } })).toBe(true);
  expect(unit.context).toMatchObject({ base: 'r1', observed: 'r0' });
  expect(scripted.writes).toHaveLength(1);
  // Target mutation admission is still held by the unobserved commit, and a
  // different unit stays editable behind it.
  expect(admitted(actor)).toBe(false);
  editStatus(actor, 'r0');
  expect(unitOf(actor, statusIdentity).getSnapshot().context.draft).toEqual({ value: { enabled: true } });
  submitStatus(actor, 'r0');
  await flush();
  expect(scripted.writes).toHaveLength(1);
  // The authoritative post-commit observation settles the commit and retires
  // the transaction.
  scripted.reads[1].resolve(projection('r1'));
  await flush();
  expectRetired(actor);
  expect(admitted(actor)).toBe(true);
  expect(mutationOutcome(actor.getSnapshot())).toEqual({ kind: 'saved', observed: true });
  // Editing resumes as a fresh transaction on the newly observed source.
  edit(actor, ['read', 'write'], 'r1');
  const second = unitOf(actor);
  expect(second).not.toBe(first);
  expect(second.getSnapshot().context).toMatchObject({ base: 'r1', observed: 'r1', draft: { value: ['read', 'write'] } });
  expect(awaitingCommitObservation(second.getSnapshot())).toBe(false);
  submit(actor, 'r1');
  await flush();
  expect(scripted.writes).toHaveLength(2);
  expect(scripted.writes[1].expected).toBe('r1');
  expect(scripted.writes[1].mutation).toEqual(toolsMutation);
});

it('R37 a newer intent authored before the acknowledgement survives it, is frozen until the observation, then editable again', async () => {
  const scripted = scriptedPort();
  const actor = settingsActor(scripted.port);
  await flush();
  scripted.reads[0].resolve(projection('r0'));
  await flush();
  edit(actor, ['read'], 'r0');
  submit(actor, 'r0');
  await flush();
  // Intent B exists before the acknowledgement of A.
  edit(actor, ['read', 'write'], 'r0');
  scripted.writes[0].resolve({ acknowledgement: projection('r1') });
  await flush();
  let unit = unitOf(actor).getSnapshot();
  expect(awaitingCommitObservation(unit)).toBe(true);
  expect(unit.context.draft).toEqual({ value: ['read', 'write'] });
  const generation = unit.context.generation;
  // B survives, but no edit — of B or of anything else — begins while the
  // commit is unobserved.
  edit(actor, ['write'], 'r0');
  unit = unitOf(actor).getSnapshot();
  expect(unit.context.draft).toEqual({ value: ['read', 'write'] });
  expect(unit.context.generation).toBe(generation);
  scripted.reads[1].resolve(projection('r1'));
  await flush();
  unit = unitOf(actor).getSnapshot();
  expect(unit.matches({ intent: 'dirty', mutation: 'settled', lifetime: 'live' })).toBe(true);
  expect(unit.context).toMatchObject({ base: 'r1', observed: 'r1' });
  edit(actor, ['read', 'write', 'shell'], 'r1');
  expect(unitOf(actor).getSnapshot().context.draft).toEqual({ value: ['read', 'write', 'shell'] });
  submit(actor, 'r1');
  await flush();
  expect(scripted.writes).toHaveLength(2);
  expect(scripted.writes[1].expected).toBe('r1');
});

// ── 14./15. Settings navigation ─────────────────────────────────────────────

function navigationActor(lookup: (directory: string) => Promise<OwnerResolution>) {
  const actor = createActor(settingsNavigationMachine, { input: { lookup } });
  actor.start();
  return actor;
}

it.each(['resolved', 'failed'] as const)('R14 a stale owner lookup that %s after a newer navigation mutates nothing', async outcome => {
  const gate = deferred<OwnerResolution>();
  const lookup = vi.fn(() => gate.promise);
  const actor = navigationActor(lookup);
  actor.send({ type: 'OPEN.OWNER', directory: '/workspace/A' });
  expect(actor.getSnapshot().matches('resolvingOwner')).toBe(true);
  // A newer navigation decision. Leaving `resolvingOwner` stops the lookup
  // actor, so the stale answer has no completion path at all.
  actor.send({ type: 'OPEN', target: userSettingsTarget });
  gate.resolve(outcome === 'resolved' ? { kind: 'resolved', id: 'wA', displayName: 'Workspace A' } : { kind: 'failed', message: 'stale owner lookup failed' });
  await flush();
  expect(actor.getSnapshot().context.target).toEqual(userSettingsTarget);
  expect(actor.getSnapshot().context.page).toBe('general');
  expect(actor.getSnapshot().context.error).toBe('');
  expect(lookup).toHaveBeenCalledTimes(1);
});

it('R15 a lookup completing under a replaced authority mutates nothing', async () => {
  const gate = deferred<OwnerResolution>();
  const actor = navigationActor(() => gate.promise);
  actor.send({ type: 'CLOSE' });
  actor.send({ type: 'OPEN.OWNER', directory: '/workspace/A' });
  // The authority is replaced. The lookup answers `retired`, which is neither a
  // navigation decision nor an error.
  actor.send({ type: 'RETIRE' });
  gate.resolve({ kind: 'retired' });
  await flush();
  expect(actor.getSnapshot().context.page).toBeUndefined();
  expect(actor.getSnapshot().context.error).toBe('');
  expect(actor.getSnapshot().context.target).toEqual(userSettingsTarget);
});

it('R15 a current lookup still commits the exact registered owning Workspace', async () => {
  const actor = navigationActor(async () => ({ kind: 'resolved', id: 'wA', displayName: 'Workspace A' }));
  actor.send({ type: 'OPEN.OWNER', directory: '/workspace/A' });
  await flush();
  expect(actor.getSnapshot().context.target).toEqual(workspaceSettingsTarget('wA', 'Workspace A'));
  // A Workspace surface is constrained: it has no General page and lands on Models.
  expect(actor.getSnapshot().context.page).toBe('models');
  expect(actor.getSnapshot().context.focus).toEqual({});
});

it('R15 an unregistered owning Workspace reports explicitly and opens nothing', async () => {
  const actor = navigationActor(async directory => ({ kind: 'unregistered', directory }));
  actor.send({ type: 'OPEN.OWNER', directory: '/workspace/revoked' });
  await flush();
  expect(actor.getSnapshot().context.error).toContain('/workspace/revoked is not registered');
  expect(actor.getSnapshot().context.page).toBeUndefined();
  expect(actor.getSnapshot().context.target).toEqual(userSettingsTarget);
});

// ── #392 page and focus navigation ──────────────────────────────────────────

it('S2-01 User Settings lands on General and a Workspace lands on its first constrained page', () => {
  const actor = navigationActor(async () => ({ kind: 'retired' }));
  actor.send({ type: 'OPEN', target: userSettingsTarget });
  expect(actor.getSnapshot().context.page).toBe('general');
  actor.send({ type: 'OPEN', target: workspaceSettingsTarget('wA', 'Workspace A') });
  expect(actor.getSnapshot().context.page).toBe('models');
});

it('S2-09 each page keeps its own detail focus across page changes, and a new target starts with none', () => {
  const actor = navigationActor(async () => ({ kind: 'retired' }));
  actor.send({ type: 'OPEN', target: userSettingsTarget });
  actor.send({ type: 'SELECT', page: 'models' });
  actor.send({ type: 'FOCUS', focus: { kind: 'provider', id: 'deepseek' } });
  actor.send({ type: 'SELECT', page: 'extensions' });
  actor.send({ type: 'FOCUS', focus: { kind: 'extension', family: 'mcp', name: 'search' } });
  actor.send({ type: 'SELECT', page: 'models' });
  expect(actor.getSnapshot().context.focus).toEqual({
    models: { kind: 'provider', id: 'deepseek' },
    extensions: { kind: 'extension', family: 'mcp', name: 'search' },
  });
  // Leaving a detail for its list clears only that page's focus.
  actor.send({ type: 'FOCUS' });
  expect(actor.getSnapshot().context.focus).toEqual({ extensions: { kind: 'extension', family: 'mcp', name: 'search' } });
  actor.send({ type: 'OPEN', target: workspaceSettingsTarget('wA', 'Workspace A') });
  expect(actor.getSnapshot().context.focus).toEqual({});
});

it('S2-01 Connection is the Advanced sub-surface of the global client, never a seventh page', () => {
  const actor = navigationActor(async () => ({ kind: 'retired' }));
  actor.send({ type: 'OPEN', target: userSettingsTarget });
  actor.send({ type: 'SELECT', page: 'models' });
  actor.send({ type: 'FOCUS', focus: { kind: 'provider', id: 'deepseek' } });
  actor.send({ type: 'OPEN.CONNECTION' });
  expect(actor.getSnapshot().context.page).toBe('advanced');
  expect(actor.getSnapshot().context.focus).toEqual({ models: { kind: 'provider', id: 'deepseek' }, advanced: { kind: 'connection' } });
  // From a Workspace surface, Connection retargets to the global client and
  // carries none of the Workspace's focus with it.
  actor.send({ type: 'OPEN', target: workspaceSettingsTarget('wA', 'Workspace A') });
  actor.send({ type: 'FOCUS', focus: { kind: 'provider', id: 'workspace-only' } });
  actor.send({ type: 'OPEN.CONNECTION' });
  expect(actor.getSnapshot().context.target).toEqual(userSettingsTarget);
  expect(actor.getSnapshot().context.focus).toEqual({ advanced: { kind: 'connection' } });
});

it('S2-01 a page cannot be selected or focused while Settings is closed', () => {
  const actor = navigationActor(async () => ({ kind: 'retired' }));
  actor.send({ type: 'SELECT', page: 'models' });
  actor.send({ type: 'FOCUS', focus: { kind: 'provider', id: 'deepseek' } });
  expect(actor.getSnapshot().context.page).toBeUndefined();
  expect(actor.getSnapshot().context.focus).toEqual({});
});

// ── #392 target capability: navigation never enters an unauthorized state ──
//
// Every case sends the illegal event straight to the machine. No renderer is
// involved, so none of these can pass because a presentation repaired the
// state afterwards.

const workspaceA = workspaceSettingsTarget('wA', 'Workspace A');
const everyPage: readonly SettingsPage[] = ['general', 'models', 'agent', 'tools', 'extensions', 'advanced'];
const everyFocus: readonly SettingsFocus[] = [
  { kind: 'provider', id: 'deepseek' }, { kind: 'model', id: 'main', provider: 'deepseek' },
  { kind: 'extension', family: 'mcp', name: 'search' }, { kind: 'connection' },
];
/** The exact legal page → focus-kind matrix of each owner. */
const legal = {
  user: { general: [], models: ['provider', 'model'], agent: [], tools: [], extensions: ['extension'], advanced: ['connection'] },
  workspace: { models: ['provider', 'model'], agent: [], tools: [], extensions: ['extension'], advanced: [] },
} as const satisfies Record<'user' | 'workspace', Partial<Record<SettingsPage, readonly SettingsFocus['kind'][]>>>;
function opened(target = userSettingsTarget as typeof userSettingsTarget | typeof workspaceA) {
  const actor = navigationActor(async () => ({ kind: 'retired' }));
  actor.send({ type: 'OPEN', target });
  return actor;
}
/** The invariant itself, checked against a live snapshot. */
function expectLegal(actor: ReturnType<typeof navigationActor>) {
  const { target, page, focus } = actor.getSnapshot().context;
  if (page === undefined) return;
  expect(settingsPages(target)).toContain(page);
  for (const [owner, detail] of Object.entries(focus)) expect(admitsFocus(target, owner as SettingsPage, detail as SettingsFocus)).toBe(true);
}

it.each(['user', 'workspace'] as const)('N01 the %s target admits exactly its legal page and focus matrix', kind => {
  const target = kind === 'user' ? userSettingsTarget : workspaceA;
  const matrix: Partial<Record<SettingsPage, readonly string[]>> = legal[kind];
  expect(settingsPages(target)).toEqual(Object.keys(matrix));
  for (const page of everyPage) for (const focus of everyFocus) {
    const actor = opened(target);
    const landing = actor.getSnapshot().context.page;
    actor.send({ type: 'SELECT', page });
    const pageLegal = page in matrix;
    expect(actor.getSnapshot().context.page).toBe(pageLegal ? page : landing);
    if (!pageLegal) continue;
    actor.send({ type: 'FOCUS', focus });
    const focusLegal = matrix[page]!.includes(focus.kind);
    expect(admitsFocus(target, page, focus)).toBe(focusLegal);
    expect(actor.getSnapshot().context.focus).toEqual(focusLegal ? { [page]: focus } : {});
    expectLegal(actor);
  }
});

it('N02 a Workspace target refuses General and stays on the page it had', () => {
  const actor = opened(workspaceA);
  actor.send({ type: 'SELECT', page: 'tools' });
  actor.send({ type: 'SELECT', page: 'general' });
  expect(actor.getSnapshot().context.page).toBe('tools');
  expectLegal(actor);
});

it('N03 a Workspace target can never enter Connection focus', () => {
  const actor = opened(workspaceA);
  actor.send({ type: 'SELECT', page: 'advanced' });
  actor.send({ type: 'FOCUS', focus: { kind: 'connection' } });
  expect(actor.getSnapshot().context.page).toBe('advanced');
  expect(actor.getSnapshot().context.focus).toEqual({});
  expectLegal(actor);
});

it('N04 Models refuses an Extension focus and Extensions refuses a Provider or Model focus', () => {
  const actor = opened();
  actor.send({ type: 'SELECT', page: 'models' });
  actor.send({ type: 'FOCUS', focus: { kind: 'extension', family: 'mcp', name: 'search' } });
  expect(actor.getSnapshot().context.focus).toEqual({});
  actor.send({ type: 'SELECT', page: 'extensions' });
  actor.send({ type: 'FOCUS', focus: { kind: 'provider', id: 'deepseek' } });
  actor.send({ type: 'FOCUS', focus: { kind: 'model', id: 'main' } });
  actor.send({ type: 'FOCUS', focus: { kind: 'connection' } });
  expect(actor.getSnapshot().context.focus).toEqual({});
  expectLegal(actor);
});

it.each(['general', 'agent', 'tools'] as const)('N05 %s admits no secondary focus of any kind', page => {
  const actor = opened();
  actor.send({ type: 'SELECT', page });
  for (const focus of everyFocus) actor.send({ type: 'FOCUS', focus });
  expect(actor.getSnapshot().context.page).toBe(page);
  expect(actor.getSnapshot().context.focus).toEqual({});
});

it('N06 an illegal FOCUS refused on one page leaves every other page\'s focus intact', () => {
  const actor = opened();
  actor.send({ type: 'SELECT', page: 'models' });
  actor.send({ type: 'FOCUS', focus: { kind: 'provider', id: 'deepseek' } });
  actor.send({ type: 'FOCUS', focus: { kind: 'connection' } });
  expect(actor.getSnapshot().context.focus).toEqual({ models: { kind: 'provider', id: 'deepseek' } });
});

it('N07 switching User → Workspace drops User-only focus and lands on a Workspace page', () => {
  const actor = opened();
  actor.send({ type: 'SELECT', page: 'models' });
  actor.send({ type: 'FOCUS', focus: { kind: 'provider', id: 'deepseek' } });
  actor.send({ type: 'SELECT', page: 'advanced' });
  actor.send({ type: 'FOCUS', focus: { kind: 'connection' } });
  actor.send({ type: 'SELECT', page: 'general' });
  actor.send({ type: 'OPEN', target: workspaceA });
  expect(actor.getSnapshot().context.target).toEqual(workspaceA);
  expect(actor.getSnapshot().context.page).toBe('models');
  expect(actor.getSnapshot().context.focus).toEqual({});
  expectLegal(actor);
  // And the owning-Workspace path obeys the same rule.
  const owner = navigationActor(async () => ({ kind: 'resolved', id: 'wA', displayName: 'Workspace A' }));
  owner.send({ type: 'OPEN.CONNECTION' });
  owner.send({ type: 'OPEN.OWNER', directory: '/workspace/A' });
  return flush().then(() => {
    expect(owner.getSnapshot().context.target).toEqual(workspaceA);
    expect(owner.getSnapshot().context.page).toBe('models');
    expect(owner.getSnapshot().context.focus).toEqual({});
    expectLegal(owner);
  });
});

it('N08 OPEN.CONNECTION explicitly retargets to User, Advanced and Connection focus', () => {
  const actor = opened(workspaceA);
  actor.send({ type: 'SELECT', page: 'extensions' });
  actor.send({ type: 'FOCUS', focus: { kind: 'extension', family: 'agent', name: 'reviewer' } });
  actor.send({ type: 'OPEN.CONNECTION' });
  expect(actor.getSnapshot().context).toMatchObject({ target: userSettingsTarget, page: 'advanced', focus: { advanced: { kind: 'connection' } } });
  // The Workspace's extension focus did not migrate into the global surface.
  expect(actor.getSnapshot().context.focus.extensions).toBeUndefined();
  expectLegal(actor);
  // From a closed dialog too.
  const closed = navigationActor(async () => ({ kind: 'retired' }));
  closed.send({ type: 'OPEN.CONNECTION' });
  expect(closed.getSnapshot().context).toMatchObject({ target: userSettingsTarget, page: 'advanced', focus: { advanced: { kind: 'connection' } } });
});

it('N09 legal per-page focus is restored after leaving a page, and illegal attempts in between change nothing', () => {
  const actor = opened();
  actor.send({ type: 'SELECT', page: 'models' });
  actor.send({ type: 'FOCUS', focus: { kind: 'model', id: 'main', provider: 'deepseek' } });
  actor.send({ type: 'SELECT', page: 'extensions' });
  actor.send({ type: 'FOCUS', focus: { kind: 'extension', family: 'mcp', name: 'search' } });
  actor.send({ type: 'SELECT', page: 'agent' });
  actor.send({ type: 'FOCUS', focus: { kind: 'provider', id: 'elsewhere' } });
  actor.send({ type: 'SELECT', page: 'advanced' });
  actor.send({ type: 'FOCUS', focus: { kind: 'connection' } });
  actor.send({ type: 'SELECT', page: 'models' });
  expect(actor.getSnapshot().context.focus.models).toEqual({ kind: 'model', id: 'main', provider: 'deepseek' });
  actor.send({ type: 'SELECT', page: 'extensions' });
  expect(actor.getSnapshot().context.focus.extensions).toEqual({ kind: 'extension', family: 'mcp', name: 'search' });
  actor.send({ type: 'SELECT', page: 'advanced' });
  expect(actor.getSnapshot().context.focus.advanced).toEqual({ kind: 'connection' });
  expect(actor.getSnapshot().context.focus).not.toHaveProperty('agent');
  expectLegal(actor);
});
