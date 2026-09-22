import { expect, it, vi } from 'vitest';
import { createActor } from 'xstate';
import type { ConfigurationApplication, SourceMutation, SourceSettings } from '../../protocol/app-server/v18';
import { settingsTargetMachine } from '../src/app/settings/machines/settings-target';
import { sessionConfigurationMachine, type SessionConfigurationPort } from '../src/app/settings/machines/session-configuration';
import { settingsNavigationMachine, type OwnerResolution } from '../src/app/settings/machines/navigation';
import type { ConfigurationPort, WriteOutcome } from '../src/app/settings/machines/port';
import { revisionSelector, userSettingsTarget, workspaceSettingsTarget } from '../src/app/settings/projection';
import { OutcomeUncertain, RpcFailure } from '../src/client/app-server';
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
function settingsActor(port: ConfigurationPort, publications?: Record<string, ConfigurationApplication>, workspace = false) {
  const actor = createActor(settingsTargetMachine, {
    input: {
      target: workspace ? workspaceSettingsTarget('A', 'A') : userSettingsTarget,
      port, connection: 'connected', generation: 1, publications,
    },
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
  expect(snapshot.context.message).toBe('');
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
  expect(snapshot.matches({ mutation: 'idle' })).toBe(true);
  expect(snapshot.context.message).toContain('Source saved');
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
  expect(actor.getSnapshot().context.writeError).toContain('Source changed');
  scripted.reads.at(-1)!.reject(new Error('read unavailable'));
  await flush();
  expect(actor.getSnapshot().context.readError).toContain('read unavailable');
  actor.send({ type: 'REFRESH' });
  await flush();
  scripted.reads.at(-1)!.resolve(projection('r2'));
  await flush();
  expect(actor.getSnapshot().context.readError).toBe('');
  // The write failure is a different fact and survives untouched.
  expect(actor.getSnapshot().context.writeError).toContain('Source changed');
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
  expect(snapshot.context.message).toContain('Source saved');
  // The observation, and only the observation, is uncertain.
  expect(snapshot.context.readError).toContain('Saved, but the authoritative reread failed');
  expect(snapshot.matches({ authority: { attached: 'blocked' } })).toBe(true);
  expect(snapshot.context.writeError).toBe('');
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
  expect(snapshot.context.message).toContain('Source saved');
  expect(unitOf(actor).getSnapshot().matches({ mutation: 'acknowledged' })).toBe(true);
  expect(unitOf(actor).getSnapshot().context.submitted?.committed).toBe('r2');
  expect(snapshot.context.writeError).toBe('');
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
  expect(disconnected.context.message).toContain('Source saved');
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
  expect(reconnected.matches({ mutation: 'idle' })).toBe(true);
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
  expect(newActor.getSnapshot().context.message).toBe('');
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
  expect(actor.getSnapshot().context.writeError).toContain('Source changed');
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
  expect(snapshot.context.writeError).toContain('Source changed');
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
  expect(actor.getSnapshot().context.unobservedCommit).toEqual({ identity: toolsIdentity });
  actor.send({ type: 'ATTACH' });
  await flush();
  // The new attachment's validation and the commit's post-commit read are the
  // same single authoritative read.
  expect(scripted.reads).toHaveLength(2);
  scripted.reads[1].resolve(projection('r2'));
  await flush();
  expect(actor.getSnapshot().context.observation?.user.revision).toBe('r2');
  expect(unitOf(actor).getSnapshot().matches({ mutation: 'settled' })).toBe(true);
  expect(actor.getSnapshot().matches({ mutation: 'idle' })).toBe(true);
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
  expect(snapshot.matches({ mutation: 'idle' })).toBe(true);
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
  expect(snapshot.matches({ mutation: 'idle' })).toBe(true);
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
    expect(snapshot.context.message).toBe('');
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
  expect(actor.getSnapshot().context.writeError).toContain('published application version 5');
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
  expect(actor.getSnapshot().context.writeError).toContain('Save outcome uncertain');
  expect(scripted.reads.length).toBe(readsBefore + 1);
  scripted.reads.at(-1)!.resolve(projection('r1'));
  await flush();
  // Exactly one write ever left the browser, and the draft is preserved.
  expect(scripted.writes).toHaveLength(1);
  expect(unitOf(actor).getSnapshot().context.draft).toEqual({ value: ['read'] });
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
