import { expect, it, vi } from 'vitest';
import { createActor, type InspectionEvent } from 'xstate';
import type { ConfigurationApplication, SourceMutation, SourceSettings } from '../../protocol/app-server/v18';
import { mutationOutcome, settingsTargetMachine } from '../src/app/settings/machines/settings-target';
import { requiresReview } from '../src/app/settings/machines/unit-transaction';
import { sessionConfigurationMachine, type SessionConfigurationPort } from '../src/app/settings/machines/session-configuration';
import { settingsNavigationMachine, type OwnerResolution } from '../src/app/settings/machines/navigation';
import type { ConfigurationPort, WriteOutcome } from '../src/app/settings/machines/port';
import { ConfigurationSystem } from '../src/app/settings/machines/system';
import { revisionSelector, userSettingsTarget, workspaceSettingsTarget } from '../src/app/settings/projection';
import { OutcomeUncertain, RpcFailure, type AppServerClient, type ClientView } from '../src/client/app-server';
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
      read: () => { const gate = deferred<SourceSettings>(); reads.push(gate); return gate.promise; },
      write: (expected, mutation) => { const gate = deferred<WriteOutcome>(); writes.push({ expected, mutation, ...gate }); return gate.promise; },
      reconcile: async () => {},
    },
  };
}
function settingsActor(
  port: ConfigurationPort, publications?: Record<string, ConfigurationApplication>, workspace = false,
  inspect?: (inspection: InspectionEvent) => void,
) {
  const actor = createActor(settingsTargetMachine, {
    input: {
      target: workspace ? workspaceSettingsTarget('A', 'A') : userSettingsTarget,
      port, connection: 'connected', generation: 1, publications,
    },
    inspect,
  });
  actor.start();
  actor.send({ type: 'ATTACH' });
  return actor;
}
const unitOf = (actor: ReturnType<typeof settingsActor>, identity = toolsIdentity) => actor.getSnapshot().context.units[identity];
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

const reconnect = (actor: ReturnType<typeof settingsActor>, generation: number, publications?: Record<string, ConfigurationApplication>) =>
  actor.send({ type: 'TRANSPORT', connection: 'connected', generation, publications });

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
  expect(unitOf(actor).getSnapshot().matches({ mutation: 'settled' })).toBe(true);
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
  const actor = settingsActor(scripted.port, { 'source:user': userApplication('1') }, true);
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
  reconnect(actor, 1, { 'source:user': userApplication('2') });
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
  actor.send({ type: 'TRANSPORT', connection: 'stale', generation: 2, publications: undefined });
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
  expect(unitOf(actor).getSnapshot().matches({ mutation: 'settled' })).toBe(true);
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
  expect(unitOf(actor).getSnapshot().matches({ mutation: 'settled' })).toBe(true);
  expect(unitOf(actor).getSnapshot().context.submitted).toBeUndefined();
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
  expect(actor.getSnapshot().context.unobservedCommit).toEqual({ identity: toolsIdentity, selector: revisionSelector(toolsMutation), revision: 'r2' });
  actor.send({ type: 'ATTACH' });
  await flush();
  // The new attachment's validation and the commit's post-commit read are the
  // same single authoritative read.
  expect(scripted.reads).toHaveLength(2);
  scripted.reads[1].resolve(projection('r2'));
  await flush();
  expect(actor.getSnapshot().context.observation?.user.revision).toBe('r2');
  expect(unitOf(actor).getSnapshot().matches({ mutation: 'settled' })).toBe(true);
  expect(actor.getSnapshot().matches({ mutation: 'saved' })).toBe(true);
  // Nothing duplicates, replays or polls: one write ever, two reads ever.
  expect(scripted.reads).toHaveLength(2);
  expect(scripted.writes).toHaveLength(1);
});

it('R22 reopening with a newer publication coalesces validation and publication into one read', async () => {
  const scripted = scriptedPort();
  const actor = settingsActor(scripted.port, { 'source:user': userApplication('1') });
  await flush();
  scripted.reads[0].resolve(projection('r1', userApplication('1')));
  await flush();
  actor.send({ type: 'DETACH' });
  // A newer native publication arrives while nothing is presented.
  reconnect(actor, 1, { 'source:user': userApplication('2') });
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
  expect(unitOf(actor).getSnapshot().matches({ mutation: 'settled' })).toBe(true);
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
  expect(unitOf(actor).getSnapshot().matches({ mutation: 'settled' })).toBe(true);
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
  expect(unitOf(actor).getSnapshot().matches({ mutation: 'settled' })).toBe(true);
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
  expect(unitOf(actor).getSnapshot().matches({ mutation: 'settled' })).toBe(true);
  expect(snapshot.matches({ mutation: 'saved' })).toBe(true);
  expect(canSubmit(actor, 'r2')).toBe(true);
  expect(settlement.log).toEqual({ commits: 1, mutation: ['idle', 'submitting', 'observing', 'saved'] });
  expect(scripted.writes).toHaveLength(1);
  expect(scripted.reads).toHaveLength(2);
});

it('R24 a reservation superseded by a newer publication read is not resurrected by reattaching', async () => {
  const scripted = scriptedPort(true);
  const actor = settingsActor(scripted.port, { 'source:user': userApplication('1') }, true);
  await flush();
  scripted.reads[0].resolve(projection('r1', userApplication('1')));
  await flush();
  edit(actor, ['read'], 'r1');
  submit(actor, 'r1');
  await flush();
  // Same generation: a newer publication owes a read that supersedes the
  // write-owned reread.
  reconnect(actor, 1, { 'source:user': userApplication('2') });
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
  expect(unitOf(actor).getSnapshot().matches({ mutation: 'settled' })).toBe(true);
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
  const actor = settingsActor(scripted.port, { 'source:user': userApplication('1') }, true, settlement.inspect);
  await flush();
  scripted.reads[0].resolve(projection('r1', userApplication('1')));
  await flush();
  edit(actor, ['read'], 'r1');
  submit(actor, 'r1');
  await flush();
  // The Workspace write reserves the read order at publication 1.
  expect(actor.getSnapshot().matches({ authority: { attached: 'awaitingWrite' } })).toBe(true);
  expect(actor.getSnapshot().context.rereadReservation).toEqual({ token: 1, scope: 'source:user', publication: 1n });
  actor.send({ type: 'DETACH' });
  if (arrival === 'while detached') reconnect(actor, 1, { 'source:user': userApplication('2') });
  else {
    // A presentation bounce alone neither revokes the reservation nor lets the
    // reattached presentation start a competing read: the publication the
    // reservation was established against is exactly what its reread answers.
    actor.send({ type: 'ATTACH' });
    await flush();
    const reattached = actor.getSnapshot();
    expect(reattached.matches({ authority: { attached: 'awaitingWrite' } })).toBe(true);
    expect(reattached.context.observation).toBeUndefined();
    expect(reattached.context.rereadReservation).toEqual({ token: 1, scope: 'source:user', publication: 1n });
    expect(scripted.reads).toHaveLength(1);
    // With no current observation at all, publication 2 is still newer than
    // the reservation's watermark.
    reconnect(actor, 1, { 'source:user': userApplication('2') });
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
  expect(unitOf(actor).getSnapshot().matches({ mutation: 'settled' })).toBe(true);
  expect(snapshot.matches({ mutation: 'saved' })).toBe(true);
  expect(mutationOutcome(snapshot)).toEqual({ kind: 'saved', observed: true });
  // Settled exactly once, never replayed, and nothing polls afterwards.
  expect(settlement.log).toEqual({ commits: 1, mutation: ['idle', 'submitting', 'observing', 'saved'] });
  expect(scripted.writes).toHaveLength(1);
  expect(scripted.reads).toHaveLength(3);
});

it('R25 a publication no newer than the reservation watermark never supersedes the reserved reread', async () => {
  const scripted = scriptedPort(true);
  const actor = settingsActor(scripted.port, { 'source:user': userApplication('1') }, true);
  await flush();
  scripted.reads[0].resolve(projection('r1', userApplication('1')));
  await flush();
  edit(actor, ['read'], 'r1');
  submit(actor, 'r1');
  await flush();
  actor.send({ type: 'DETACH' });
  actor.send({ type: 'ATTACH' });
  // The same publication delivered again is no publication progress.
  reconnect(actor, 1, { 'source:user': userApplication('1') });
  await flush();
  expect(actor.getSnapshot().matches({ authority: { attached: 'awaitingWrite' } })).toBe(true);
  expect(scripted.reads).toHaveLength(1);
  scripted.writes[0].resolve({ acknowledgement: projection('r2'), reread: { status: 'observed', projection: projection('r2', userApplication('1')) } });
  await flush();
  // The reserved reread is the reattached presentation's fresh observation.
  const snapshot = actor.getSnapshot();
  expect(snapshot.context.observation?.user.revision).toBe('r2');
  expect(unitOf(actor).getSnapshot().matches({ mutation: 'settled' })).toBe(true);
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
  const actor = settingsActor(scripted.port, { 'source:user': userApplication('1') });
  await flush();
  scripted.reads[0].resolve(projection('r1', userApplication('1')));
  await flush();
  expect(scripted.reads).toHaveLength(1);
  // Publication 2 starts exactly one read. Publication 3 arrives while it is
  // outstanding and starts none — the obligation is a level, not an edge.
  actor.send({ type: 'TRANSPORT', connection: 'connected', generation: 1, publications: { 'source:user': userApplication('2') } });
  await flush();
  expect(scripted.reads).toHaveLength(2);
  actor.send({ type: 'TRANSPORT', connection: 'connected', generation: 1, publications: { 'source:user': userApplication('3') } });
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
  const actor = settingsActor(scripted.port, { 'source:user': userApplication('5') });
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
  expect(unitOf(actor).getSnapshot().matches({ mutation: 'settled' })).toBe(true);
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
  const actor = settingsActor(scripted.port, { 'source:user': userApplication('1') }, true);
  await flush();
  scripted.reads[0].resolve(projection('r1', userApplication('1')));
  await flush();
  edit(actor, ['read'], 'r1');
  submit(actor, 'r1');
  await flush();
  // A newer publication supersedes the Workspace write's reserved reread
  // with a read issued while the write is still in flight.
  reconnect(actor, 1, { 'source:user': userApplication('2') });
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
  unit = unitOf(actor).getSnapshot();
  expect(actor.getSnapshot().matches({ mutation: 'saved' })).toBe(true);
  if (verdict === 'settles') {
    expect(unit.matches({ mutation: 'settled' })).toBe(true);
    expect(requiresReview(unit)).toBe(false);
  } else {
    expect(unit.matches({ mutation: { acknowledged: 'diverged' } })).toBe(true);
    expect(unit.context.submitted?.committed).toBe('r2');
    expect(requiresReview(unit)).toBe(true);
  }
  expect(scripted.writes).toHaveLength(1);
  expect(scripted.reads).toHaveLength(3);
});

// ── 12./13. Session configuration ───────────────────────────────────────────

function sessionActor(port: SessionConfigurationPort) {
  const actor = createActor(sessionConfigurationMachine, { input: { port, connection: 'connected', generation: 1 } });
  actor.start();
  return actor;
}
const candidate = { identity: { input_revision: 'input-2', attempt: '2' }, expected_binding: '1', impact: 'prefix_changed' as const };

it('R12 a failed reread after a failed adoption strands no in-flight guard', async () => {
  const reads: ReturnType<typeof deferred<ConfigurationApplication | null>>[] = [];
  const adoptions: ReturnType<typeof deferred<void>>[] = [];
  const actor = sessionActor({
    read: () => { const gate = deferred<ConfigurationApplication | null>(); reads.push(gate); return gate.promise; },
    adopt: () => { const gate = deferred<void>(); adoptions.push(gate); return gate.promise; },
  });
  actor.send({ type: 'REFRESH' });
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
  expect(actor.getSnapshot().matches({ observation: 'unavailable' })).toBe(true);
  expect(adoptions).toHaveLength(1);
  // A later observation makes the same candidate actionable again.
  actor.send({ type: 'ADOPT', candidate });
  await flush();
  expect(actor.getSnapshot().matches({ adoption: 'submitting' })).toBe(true);
  expect(adoptions).toHaveLength(2);
});

it('R13 a successful observation clears the read failure and preserves the independent adoption failure', async () => {
  const reads: ReturnType<typeof deferred<ConfigurationApplication | null>>[] = [];
  const adoptions: ReturnType<typeof deferred<void>>[] = [];
  const actor = sessionActor({
    read: () => { const gate = deferred<ConfigurationApplication | null>(); reads.push(gate); return gate.promise; },
    adopt: () => { const gate = deferred<void>(); adoptions.push(gate); return gate.promise; },
  });
  actor.send({ type: 'REFRESH' });
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
  expect(actor.getSnapshot().matches({ observation: 'ready' })).toBe(true);
  expect(actor.getSnapshot().context.adoptionError).toContain('Conflict');
});

// ── 13b. Application versions are scoped to one connection generation ───────
//
// (These two continue the Session R12/R13 series; the R19-R23 presentation
// attachment regressions live in section 4b below.)

/** A Session actor whose reads are released explicitly by the test. */
function scriptedSession() {
  const reads: ReturnType<typeof deferred<ConfigurationApplication | null>>[] = [];
  const actor = sessionActor({
    read: () => { const gate = deferred<ConfigurationApplication | null>(); reads.push(gate); return gate.promise; },
    adopt: async () => {},
  });
  return { actor, reads };
}
const otherCandidate = { identity: { input_revision: 'input-9', attempt: '9' }, expected_binding: '2', impact: 'prefix_changed' as const };

it('R13b a new connection generation establishes its own application-version baseline', async () => {
  const { actor, reads } = scriptedSession();
  actor.send({ type: 'REFRESH' });
  await flush();
  reads[0].resolve({ ...cfg3SourceApplication(), version: '100', candidate });
  await flush();
  expect(actor.getSnapshot().context.application?.version).toBe('100');
  // The App Server restarts. Application versions are a runtime counter of one
  // process, so generation 2's version 3 is not "older" than generation 1's 100
  // — there is no comparison to make across the boundary at all.
  actor.send({ type: 'TRANSPORT', connection: 'connected', generation: 2 });
  await flush();
  expect(reads).toHaveLength(2);
  expect(actor.getSnapshot().context.application).toBeUndefined();
  expect(actor.getSnapshot().context.staleApplication?.version).toBe('100');
  reads[1].resolve({ ...cfg3SourceApplication(), version: '3', candidate: otherCandidate });
  await flush();
  const snapshot = actor.getSnapshot();
  expect(snapshot.context.application?.version).toBe('3');
  expect(snapshot.context.application?.candidate).toEqual(otherCandidate);
  expect(snapshot.context.staleApplication).toBeUndefined();
  expect(snapshot.matches({ observation: 'ready' })).toBe(true);
});

it('R13c an obsolete result of the same connection generation still cannot regress the observation', async () => {
  const { actor, reads } = scriptedSession();
  actor.send({ type: 'TRANSPORT', connection: 'connected', generation: 2 });
  await flush();
  reads[0].resolve({ ...cfg3SourceApplication(), version: '3', candidate: otherCandidate });
  await flush();
  expect(actor.getSnapshot().context.application?.version).toBe('3');
  // Same generation, same application-version domain: version 2 is genuinely
  // older and never replaces version 3.
  actor.send({ type: 'REFRESH' });
  await flush();
  reads[1].resolve({ ...cfg3SourceApplication(), version: '2', candidate });
  await flush();
  const snapshot = actor.getSnapshot();
  expect(snapshot.context.application?.version).toBe('3');
  expect(snapshot.context.application?.candidate).toEqual(otherCandidate);
  expect(snapshot.matches({ observation: 'ready' })).toBe(true);
});

// ── 13c. The configuration system owns actor lifetime ───────────────────────
//
// An actor retained only for a transaction in flight is released exactly at
// that transaction's terminal point, observed by the system — never by
// polling, and never by waiting for some later, unrelated lifetime change.

/** An App Server client whose every request is released explicitly by the
 * test, and whose App Server authority the test replaces. */
function scriptedClient() {
  const view = {
    endpoint: 'ws://native.invalid', authorityRevision: 1, connection: 'connected', generation: 1,
    sessions: [], views: {}, uncertain: [], interactionOperations: {},
  } satisfies ClientView;
  const requests: { method: string; resolve: (value: unknown) => void; reject: (reason?: unknown) => void }[] = [];
  const client = {
    getSnapshot: () => view,
    subscribe: () => () => {},
    request: ({ method }: { method: string }) => { const gate = deferred<unknown>(); requests.push({ method, ...gate }); return gate.promise; },
  } as unknown as AppServerClient;
  return {
    system: new ConfigurationSystem(client),
    pending: (method: string) => requests.filter(request => request.method === method),
    replaceAuthority: () => { view.authorityRevision += 1; },
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

it('R26 an authority replacement stops and drops a target with no native mutation in flight, its dirty secret-bearing draft with it', async () => {
  const native = scriptedClient();
  const old = await observedTarget(native);
  const oldUnit = old.getSnapshot().context.units[providerIdentity];
  // The unsaved literal credential is the live editing draft of the old authority.
  const [oldOwner] = native.system.transactionOwners();
  expect(retainedBy(native.system)).toContain(PROVIDER_SECRET);
  native.replaceAuthority();
  const replacement = native.system.settingsTarget(userSettingsTarget, () => undefined);
  expect(replacement).not.toBe(old);
  // Nothing of the old authority crossed the native submission boundary, so
  // nothing of it may outlive the replacement: it is stopped at the
  // replacement itself, its transactions with it.
  expect(old.getSnapshot().status).toBe('stopped');
  expect(oldUnit.getSnapshot().status).toBe('stopped');
  // The old transaction owner is unreachable, and the secret-bearing draft is
  // neither retained nor migrated into the replacement.
  const owners = native.system.transactionOwners();
  expect(owners).toHaveLength(1);
  expect(owners).not.toContain(oldOwner);
  expect(retainedBy(native.system)).toBe('[[]]');
  expect(replacement.getSnapshot().context.units).toEqual({});
  expect(native.pending('configuration/sourceWrite')).toHaveLength(0);
});

it.each(['acknowledged', 'conflicted'] as const)('R26 an authority replacement retains a target with a native mutation in flight only until that mutation settles, when it is %s', async outcome => {
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
  const replacement = native.system.settingsTarget(userSettingsTarget, () => undefined);
  // The mutation already crossed the native submission boundary: the old
  // lifetime is retained, detached, for exactly that settlement.
  expect(old.getSnapshot().status).toBe('active');
  expect(old.getSnapshot().matches({ authority: 'suspended' })).toBe(true);
  expect(old.getSnapshot().matches({ mutation: 'submitting' })).toBe(true);
  expect(native.system.transactionOwners()).toHaveLength(2);
  expect(native.system.transactionOwners()[0]).toBe(oldOwner);
  replacement.send({ type: 'ATTACH' });
  await flush();
  native.pending('configuration/sourcesRead')[1].resolve({ projection: projection('fresh') });
  await flush();
  const write = native.pending('configuration/sourceWrite')[0];
  if (outcome === 'acknowledged') write.resolve({ projection: projection('r2') });
  else write.reject(new RpcFailure({ code: -32000, message: 'Conflict', data: { kind: 'source_conflict', scope: 'user', expected: 'r1', actual: 'r-external' } }));
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
  expect(old.getSnapshot().matches({ mutation: outcome === 'acknowledged' ? 'observing' : 'conflicted' })).toBe(true);
  // …and that settlement is the retained lifetime's terminal point: the system
  // stops and drops it at once, with no further authority replacement.
  expect(old.getSnapshot().status).toBe('stopped');
  expect(oldUnit.getSnapshot().status).toBe('stopped');
  expect(native.system.transactionOwners()).toHaveLength(1);
  expect(native.system.transactionOwners()).not.toContain(oldOwner);
  expect(retainedBy(native.system)).not.toContain(PROVIDER_SECRET);
  // The detached old lifetime issued no read of its own while settling, and
  // the replacement stayed isolated from it throughout.
  expect(native.pending('configuration/sourcesRead')).toHaveLength(2);
  const isolated = replacement.getSnapshot();
  expect(isolated.context.units).toEqual({});
  expect(isolated.context.observation?.user.revision).toBe('fresh');
  expect(mutationOutcome(isolated)).toEqual({ kind: 'none' });
  expect(isolated.context.rejection).toBeUndefined();
  // The mutation left the browser exactly once.
  expect(native.pending('configuration/sourceWrite')).toHaveLength(1);
});

/** A Session actor of the current authority, held by one presentation, observed
 * with a candidate and with an adoption of it in flight. */
async function adoptingSession(native: ReturnType<typeof scriptedClient>) {
  const actor = native.system.sessionConfiguration('session-1');
  native.system.retainSession(actor);
  actor.send({ type: 'REFRESH' });
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
  expect(answered.matches({ observation: { loading: 'adoptionReread' } })).toBe(true);
  expect(native.pending('session/configuration')).toHaveLength(2);
  const rereadGate = native.pending('session/configuration')[1];
  if (reread === 'observed') rereadGate.resolve({ application: cfg3SourceApplication() });
  else rereadGate.reject(new Error('configuration read unavailable'));
  await flush();
  // The reread settled: that is the terminal point, and the system stops and
  // removes the actor at once, with no second release.
  const terminal = actor.getSnapshot();
  expect(terminal.status).toBe('stopped');
  expect(terminal.matches({ observation: reread === 'observed' ? 'ready' : 'unavailable' })).toBe(true);
  expect(native.system.sessionConfiguration('session-1')).not.toBe(actor);
  // Adoption was never replayed, whatever its outcome, and nothing reread twice.
  expect(native.pending('session/adoptConfiguration')).toHaveLength(1);
  expect(native.pending('session/configuration')).toHaveLength(2);
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
  expect(actor.getSnapshot().context.section).toBe('overview');
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
  expect(actor.getSnapshot().context.section).toBeUndefined();
  expect(actor.getSnapshot().context.error).toBe('');
  expect(actor.getSnapshot().context.target).toEqual(userSettingsTarget);
});

it('R15 a current lookup still commits the exact registered owning Workspace', async () => {
  const actor = navigationActor(async () => ({ kind: 'resolved', id: 'wA', displayName: 'Workspace A' }));
  actor.send({ type: 'OPEN.OWNER', directory: '/workspace/A' });
  await flush();
  expect(actor.getSnapshot().context.target).toEqual(workspaceSettingsTarget('wA', 'Workspace A'));
  expect(actor.getSnapshot().context.section).toBe('overview');
});

it('R15 an unregistered owning Workspace reports explicitly and opens nothing', async () => {
  const actor = navigationActor(async directory => ({ kind: 'unregistered', directory }));
  actor.send({ type: 'OPEN.OWNER', directory: '/workspace/revoked' });
  await flush();
  expect(actor.getSnapshot().context.error).toContain('/workspace/revoked is not registered');
  expect(actor.getSnapshot().context.section).toBeUndefined();
  expect(actor.getSnapshot().context.target).toEqual(userSettingsTarget);
});
