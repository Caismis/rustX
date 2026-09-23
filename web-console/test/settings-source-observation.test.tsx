// @vitest-environment jsdom
import { afterEach, expect, it } from 'vitest';
import { act, cleanup, fireEvent, render, screen, waitFor } from '@testing-library/react';
import type { ConfigurationApplication, Request, SourceSettings, SourceTarget } from '../../protocol/app-server/v18';
import { SettingsSurface } from './settings-harness';
import { RpcFailure } from '../src/client/app-server';
import { userSettingsTarget, workspaceSettingsTarget } from '../src/app/settings/projection';
import type { ProductHostWorkspaces, WorkspaceConfigurationReread } from '../src/workspaces/host';
import { cfg3Source, cfg3SourceApplication } from './cfg3-data';
import { Server } from './fixture';
afterEach(cleanup);

/** Native source authority for one User target. Authoring (revision) and native
 * application (the `source:user` ConfigurationApplication) advance separately,
 * exactly as the App Server owns them. */
class Native {
  connections = 32;
  revision = 'user-1';
  /** Authored marker independent of application version, for equal-version races. */
  description?: string;
  application: ConfigurationApplication | null = null;
  projection(): SourceSettings {
    const source = cfg3Source();
    source.user.revision = this.revision;
    if (this.description !== undefined) source.user.authored = { ...source.user.authored, agent: { description: this.description } };
    source.process_policy_impacts = { max_connections: 'hot' };
    source.process_bindings = { max_resident_runtimes: 8, max_connections: this.connections };
    source.application = this.application ? structuredClone(this.application) : null;
    return source;
  }
  applied(version: string, status: 'preparing' | 'applied'): ConfigurationApplication {
    return { scope: 'source:user', sources: [{ kind: 'user' }], version, desired: { input_revision: 'input-1', attempt: '1' },
      units: { process_bindings: { status } }, candidate: null, eligibility: { status: 'unavailable' } };
  }
}

async function open(native: Native) {
  const s = new Server();
  s.handlers.set('configuration/sourcesRead', () => ({ type: 'source_settings', projection: native.projection() }));
  s.handlers.set('configuration/sourceWrite', () => ({ type: 'source_settings', projection: native.projection() }));
  await s.connect();
  render(<SettingsSurface client={s.client} target={userSettingsTarget} />);
  await screen.findByText(new RegExp(`Revision: ${native.revision}`));
  fireEvent.click(screen.getByRole('button', { name: 'General' }));
  return s;
}
const sent = (s: Server, method: Request['method']) => s.requests.filter(item => item.request.method === method).map(item => item.request);
/** Native application publication, independent of any acknowledgement. */
async function publish(s: Server, application: ConfigurationApplication) {
  await act(async () => { s.socket.deliver({ jsonrpc: '2.0', method: 'configuration/changed', params: { application } }); });
}
async function deliver(s: Server, request: Request) { await act(async () => { s.reply(request); await Promise.resolve(); }); }
/** Settle every authoritative read this client has already issued. */
async function settleReads(s: Server, from = 1) {
  for (const read of sent(s, 'configuration/sourcesRead').slice(from)) await deliver(s, read);
}
/** The authoritative native process-binding projection, not a status label. */
const bindings = () => JSON.parse(screen.getByRole('heading', { name: 'Process bindings' }).nextElementSibling!.textContent!) as { max_connections: number };
const diagnostics = () => fireEvent.click(screen.getByRole('button', { name: 'Server & source diagnostics' }));
/** The whole accepted projection, exposed only by the Advanced diagnostics. */
const acceptedProjection = () => JSON.parse(screen.getByText('Source and application diagnostics').parentElement!.querySelector('pre')!.textContent!) as SourceSettings;
/** Save 19 with the acknowledgement held, freezing the projection the App Server
 * captured while application was still preparing, then complete application. */
async function saveHeldWrite(s: Server, native: Native) {
  s.held.add('configuration/sourceWrite'); s.held.add('configuration/sourcesRead');
  fireEvent.change(screen.getByLabelText('max_connections'), { target: { value: '19' } });
  fireEvent.click(screen.getByRole('button', { name: 'Save App Server policy' }));
  const [write] = sent(s, 'configuration/sourceWrite');
  native.revision = 'user-2'; native.application = native.applied('1', 'preparing');
  s.commit(write);
  native.connections = 19; native.application = native.applied('2', 'applied');
  return write;
}
async function expectApplied(after: () => void = () => {}) {
  diagnostics(); after();
  await waitFor(() => expect(bindings().max_connections).toBe(19));
  expect(screen.getByText('Saved process policy is active.')).toBeTruthy();
}

it('S1 a newer applied notification preceding an older preparing acknowledgement still converges', async () => {
  const native = new Native(); const s = await open(native);
  const write = await saveHeldWrite(s, native);
  await publish(s, native.applied('2', 'applied'));
  await deliver(s, write);
  // Save success settles only after the post-commit authoritative read lands.
  await waitFor(() => expect(sent(s, 'configuration/sourcesRead').length).toBeGreaterThan(2));
  await settleReads(s);
  await screen.findByText(/Source saved. Native coordination/);
  await expectApplied();
  expect(sent(s, 'configuration/sourceWrite')).toHaveLength(1);
});

it('S2 an outstanding notification-driven read survives the stale write acknowledgement', async () => {
  const native = new Native(); const s = await open(native);
  const write = await saveHeldWrite(s, native);
  await publish(s, native.applied('2', 'applied'));
  await waitFor(() => expect(sent(s, 'configuration/sourcesRead').length).toBeGreaterThan(1));
  const read = sent(s, 'configuration/sourcesRead')[1];
  s.commit(read); // Outstanding, and captured, before the acknowledgement is handled.
  await deliver(s, write);
  // The save owes one bounded post-commit read through the same read owner.
  await waitFor(() => expect(sent(s, 'configuration/sourcesRead').length).toBeGreaterThanOrEqual(3));
  await deliver(s, read);
  await settleReads(s, 2);
  await expectApplied();
  expect(sent(s, 'configuration/sourceWrite')).toHaveLength(1);
});

it('S3 an accepted newer application observation is not regressed by a late acknowledgement', async () => {
  const native = new Native(); const s = await open(native);
  const write = await saveHeldWrite(s, native);
  await publish(s, native.applied('2', 'applied'));
  await waitFor(() => expect(sent(s, 'configuration/sourcesRead').length).toBeGreaterThan(1));
  await settleReads(s);
  await expectApplied();
  await deliver(s, write);
  await waitFor(() => expect(sent(s, 'configuration/sourcesRead').length).toBeGreaterThan(2));
  await settleReads(s, 2);
  await screen.findByText(/Source saved. Native coordination/);
  expect(bindings().max_connections).toBe(19);
  expect(screen.getByText('Saved process policy is active.')).toBeTruthy();
});

it('S4 the acknowledgement-first order converges under the same observation model', async () => {
  const native = new Native(); const s = await open(native);
  const write = await saveHeldWrite(s, native);
  await deliver(s, write);
  await waitFor(() => expect(sent(s, 'configuration/sourcesRead').length).toBe(2));
  await settleReads(s);
  await screen.findByText(/Source saved. Native coordination/);
  await publish(s, native.applied('2', 'applied'));
  await expectApplied();
  expect(sent(s, 'configuration/sourceWrite')).toHaveLength(1);
});

it('S5 automatic convergence never replays the write or rewrites an unsaved draft', async () => {
  const native = new Native(); const s = await open(native);
  fireEvent.change(screen.getByRole('textbox', { name: 'Root description' }), { target: { value: 'unsaved draft' } });
  const write = await saveHeldWrite(s, native);
  await publish(s, native.applied('2', 'applied'));
  await deliver(s, write);
  await waitFor(() => expect(sent(s, 'configuration/sourcesRead').length).toBeGreaterThan(2));
  await settleReads(s);
  await expectApplied();
  fireEvent.click(screen.getByRole('button', { name: 'General' }));
  expect((screen.getByRole('textbox', { name: 'Root description' }) as HTMLInputElement).value).toBe('unsaved draft');
  expect(sent(s, 'configuration/sourceWrite')).toHaveLength(1);
  // Convergence is bounded by the versions actually published, never a poll.
  expect(sent(s, 'configuration/sourcesRead').length).toBeLessThanOrEqual(3);
  expect(s.requests.every(item => !['session/attach', 'session/create', 'turn/start'].includes(item.request.method))).toBe(true);
});

it('S6 a publication arriving during an outstanding convergence read still drives the follow-up read', async () => {
  const native = new Native(); native.application = native.applied('1', 'applied');
  const s = await open(native); // settles at application 1
  s.held.add('configuration/sourceWrite'); s.held.add('configuration/sourcesRead');
  fireEvent.change(screen.getByLabelText('max_connections'), { target: { value: '19' } });
  fireEvent.click(screen.getByRole('button', { name: 'Save App Server policy' }));
  const [write] = sent(s, 'configuration/sourceWrite');
  // Native commits the write and republishes application 2, still preparing.
  native.revision = 'user-2'; native.application = native.applied('2', 'preparing');
  s.commit(write);
  await publish(s, native.applied('2', 'preparing'));
  await waitFor(() => expect(sent(s, 'configuration/sourcesRead').length).toBe(2));
  const r2 = sent(s, 'configuration/sourcesRead')[1];
  s.commit(r2); // The outstanding read captured application 2.
  // While the read is outstanding, native settles and publishes application 3:
  // the newer publication obligation survives the in-flight read.
  native.connections = 19; native.application = native.applied('3', 'applied');
  await publish(s, native.applied('3', 'applied'));
  expect(sent(s, 'configuration/sourcesRead').length).toBe(2);
  // The acknowledgement captured at application 2 lands while R2 is
  // outstanding; it cannot discharge the newer publication obligation.
  await deliver(s, write);
  // R2 settles at 2, below the newest publication. Without another
  // notification, manual refresh, save, navigation or timer, the level still
  // owed drives exactly one more authoritative read.
  await deliver(s, r2);
  await waitFor(() => expect(sent(s, 'configuration/sourcesRead').length).toBe(3));
  await settleReads(s, 2);
  await screen.findByText(/Source saved. Native coordination/);
  await expectApplied();
  expect(sent(s, 'configuration/sourceWrite')).toHaveLength(1);
  expect(sent(s, 'configuration/sourcesRead').length).toBe(3);
});

it('S7 an equal-version acknowledgement cannot regress a newer authoritative authored source', async () => {
  const native = new Native(); native.application = native.applied('7', 'applied');
  const s = await open(native);
  s.held.add('configuration/sourceWrite'); s.held.add('configuration/sourcesRead');
  fireEvent.change(screen.getByLabelText('max_connections'), { target: { value: '19' } });
  // An explicit authoritative read is outstanding before the save starts.
  fireEvent.click(screen.getByRole('button', { name: 'Read current sources' }));
  await waitFor(() => expect(sent(s, 'configuration/sourcesRead').length).toBe(2));
  const read = sent(s, 'configuration/sourcesRead')[1];
  fireEvent.click(screen.getByRole('button', { name: 'Save App Server policy' }));
  const [write] = sent(s, 'configuration/sourceWrite');
  // The acknowledgement captured revision B at application version 7.
  native.revision = 'user-2'; native.description = 'revision B';
  s.commit(write);
  // A causally later external edit advances the authored source to revision C;
  // native coordination has not republished, so the application version is
  // still 7 — equal application versions cannot order the two projections.
  native.revision = 'user-3'; native.description = 'revision C';
  s.commit(read); // The authoritative read observes C at the same version 7.
  await deliver(s, read);
  await screen.findByText(/Revision: user-3/);
  // The delayed equal-version acknowledgement cannot restore B over the
  // causally later authoritative read.
  await deliver(s, write);
  expect(screen.getByText(/Revision: user-3/)).toBeTruthy();
  // The acknowledgement settles its save exactly once, after the bounded
  // post-commit authoritative read owed by that commit.
  await waitFor(() => expect(sent(s, 'configuration/sourcesRead').length).toBe(3));
  await settleReads(s, 2);
  await screen.findByText(/Source saved. Native coordination/);
  expect(screen.getByText(/Revision: user-3/)).toBeTruthy();
  expect((screen.getByRole('textbox', { name: 'Root description' }) as HTMLInputElement).value).toBe('revision C');
  expect(sent(s, 'configuration/sourceWrite')).toHaveLength(1);
  expect(sent(s, 'configuration/sourcesRead').length).toBe(3);
});

it('S8 a write acknowledgement cannot cancel an outstanding newer-authority read', async () => {
  const native = new Native(); native.application = native.applied('1', 'applied');
  const s = await open(native);
  s.held.add('configuration/sourceWrite'); s.held.add('configuration/sourcesRead');
  fireEvent.change(screen.getByLabelText('max_connections'), { target: { value: '19' } });
  // Native publishes application 2; the convergence read starts and is frozen.
  native.application = native.applied('2', 'preparing');
  await publish(s, native.applied('2', 'preparing'));
  await waitFor(() => expect(sent(s, 'configuration/sourcesRead').length).toBe(2));
  const read = sent(s, 'configuration/sourcesRead')[1];
  // The older acknowledgement lands while that read is outstanding.
  fireEvent.click(screen.getByRole('button', { name: 'Save App Server policy' }));
  const [write] = sent(s, 'configuration/sourceWrite');
  native.revision = 'user-2'; s.commit(write); // captured before application 3
  // Native settles at application 3 while the read is outstanding; the
  // authoritative observations carry the newer state.
  native.connections = 19; native.application = native.applied('3', 'applied');
  await deliver(s, write);
  // The acknowledgement neither cancels the outstanding read nor poses as the
  // newer observation: the save owes one bounded post-commit read through the
  // same read owner, and delivering the superseded read is harmless.
  await waitFor(() => expect(sent(s, 'configuration/sourcesRead').length).toBe(3));
  await deliver(s, read);
  await settleReads(s, 2);
  await screen.findByText(/Source saved. Native coordination/);
  await expectApplied();
  expect(sent(s, 'configuration/sourceWrite')).toHaveLength(1);
  expect(sent(s, 'configuration/sourcesRead').length).toBe(3);
});

it('S9 publications coalesce into one bounded follow-up read and none are lost', async () => {
  const native = new Native(); native.application = native.applied('1', 'applied');
  const s = await open(native);
  s.held.add('configuration/sourcesRead');
  native.application = native.applied('2', 'preparing');
  await publish(s, native.applied('2', 'preparing'));
  await waitFor(() => expect(sent(s, 'configuration/sourcesRead').length).toBe(2));
  const read = sent(s, 'configuration/sourcesRead')[1];
  s.commit(read); // The outstanding read captured application 2.
  // Several monotonically newer publications arrive while it is outstanding.
  native.application = native.applied('3', 'preparing');
  await publish(s, native.applied('3', 'preparing'));
  native.connections = 19; native.application = native.applied('4', 'applied');
  await publish(s, native.applied('4', 'applied'));
  // Coalesced: never one read per notification while one is outstanding.
  expect(sent(s, 'configuration/sourcesRead').length).toBe(2);
  await deliver(s, read); // settles at 2, below the newest publication
  await waitFor(() => expect(sent(s, 'configuration/sourcesRead').length).toBe(3));
  await deliver(s, sent(s, 'configuration/sourcesRead')[2]); // causally after the latest publication
  await expectApplied();
  expect(sent(s, 'configuration/sourceWrite')).toHaveLength(0);
  // Bounded by the obligations that actually existed; nothing polls afterward.
  expect(sent(s, 'configuration/sourcesRead').length).toBe(3);
});

it('S10 target replacement fences stale reads, acknowledgements and the stale worker', async () => {
  const native = new Native(); native.application = native.applied('1', 'applied');
  let workspaceApplication: ConfigurationApplication | null = null;
  const s = new Server();
  s.handlers.set('configuration/sourcesRead', request => {
    const target = (request as Extract<Request, { method: 'configuration/sourcesRead' }>).params.target;
    const projection = native.projection();
    projection.target = target;
    if (target.kind === 'workspace') { projection.workspace!.revision = 'ws-1'; projection.application = workspaceApplication ? structuredClone(workspaceApplication) : null; }
    return { type: 'source_settings', projection };
  });
  s.handlers.set('configuration/sourceWrite', request => {
    const target = (request as Extract<Request, { method: 'configuration/sourceWrite' }>).params.target;
    return { type: 'source_settings', projection: { ...native.projection(), target } };
  });
  const host: ProductHostWorkspaces = {
    ...s.workspaceHost,
    configureWorkspace: async (id, _endpoint, operation) => {
      const target: SourceTarget = { kind: 'workspace', directory: `/workspace/${id === 'workspace-a' ? 'A' : 'B'}` };
      if (operation.kind === 'write') {
        const acknowledgement = (await s.client.request({ method: 'configuration/sourceWrite', params: { target, expected_revision: operation.expected_revision, mutation: operation.mutation } }, 'source_settings')).projection;
        const reread: WorkspaceConfigurationReread = { status: 'observed', projection: (await s.client.request({ method: 'configuration/sourcesRead', params: { target } }, 'source_settings')).projection };
        return { kind: 'write', commit: { acknowledgement, reread } };
      }
      if (operation.kind === 'reconcile') await s.client.request({ method: 'configuration/reconcile', params: { target } }, 'configuration_application');
      return { kind: operation.kind, projection: (await s.client.request({ method: 'configuration/sourcesRead', params: { target } }, 'source_settings')).projection };
    },
  };
  await s.connect();
  const ui = render(<SettingsSurface client={s.client} target={userSettingsTarget} host={host} />);
  await screen.findByText(/Revision: user-1/);
  fireEvent.click(screen.getByRole('button', { name: 'General' }));
  s.held.add('configuration/sourceWrite'); s.held.add('configuration/sourcesRead');
  fireEvent.change(screen.getByLabelText('max_connections'), { target: { value: '19' } });
  fireEvent.click(screen.getByRole('button', { name: 'Save App Server policy' }));
  const [write] = sent(s, 'configuration/sourceWrite');
  native.revision = 'user-2'; native.application = native.applied('2', 'preparing');
  s.commit(write);
  await publish(s, native.applied('2', 'preparing'));
  await waitFor(() => expect(sent(s, 'configuration/sourcesRead').length).toBe(2));
  const staleRead = sent(s, 'configuration/sourcesRead')[1];
  // Replace the target before the held read or acknowledgement settles.
  ui.rerender(<SettingsSurface client={s.client} target={workspaceSettingsTarget('workspace-a', 'Workspace A')} host={host} />);
  await waitFor(() => expect(sent(s, 'configuration/sourcesRead').length).toBe(3));
  await deliver(s, sent(s, 'configuration/sourcesRead')[2]);
  await screen.findByText(/Revision: ws-1/);
  const reads = sent(s, 'configuration/sourcesRead').length;
  // Stale continuations from the replaced lifetime cannot land or re-arm.
  await deliver(s, staleRead);
  await deliver(s, write);
  expect(screen.queryByText(/Revision: user-2/)).toBeNull();
  expect(screen.getByText(/Revision: ws-1/)).toBeTruthy();
  expect(screen.queryByText(/Source saved/)).toBeNull();
  expect(sent(s, 'configuration/sourcesRead').length).toBe(reads);
  // The replacement lifetime still converges on its own publications.
  workspaceApplication = { ...native.applied('2', 'applied'), scope: 'source:workspace:/workspace/A' };
  await publish(s, workspaceApplication);
  await waitFor(() => expect(sent(s, 'configuration/sourcesRead').length).toBe(reads + 1));
  await settleReads(s, reads);
  await screen.findByText(/Revision: ws-1/);
  expect(sent(s, 'configuration/sourceWrite')).toHaveLength(1);
});

it('S11 a superseded convergence worker transfers a publication that arrived behind its newer read', async () => {
  const native = new Native(); native.application = native.applied('1', 'applied');
  const s = await open(native); // settles at application 1
  // Freeze every authoritative read so the exact capture/release order is forced.
  s.held.add('configuration/sourceWrite'); s.held.add('configuration/sourcesRead');
  // Step 1: native publishes application 2.
  native.application = native.applied('2', 'preparing');
  await publish(s, native.applied('2', 'preparing'));
  // Step 2: the convergence owner starts read R1; freeze it with a response
  // captured while application was still 2.
  await waitFor(() => expect(sent(s, 'configuration/sourcesRead').length).toBe(2));
  const r1 = sent(s, 'configuration/sourcesRead')[1];
  s.commit(r1);
  // Step 3: deliver the successful sourceWrite acknowledgement.
  fireEvent.change(screen.getByLabelText('max_connections'), { target: { value: '19' } });
  fireEvent.click(screen.getByRole('button', { name: 'Save App Server policy' }));
  const [write] = sent(s, 'configuration/sourceWrite');
  native.revision = 'user-2';
  s.commit(write);
  await deliver(s, write);
  // Step 4: the save's post-commit authoritative read R2 starts; freeze it at v2 too.
  await waitFor(() => expect(sent(s, 'configuration/sourcesRead').length).toBe(3));
  const r2 = sent(s, 'configuration/sourcesRead')[2];
  s.commit(r2);
  // Step 5: application 3 arrives while both R1 and R2 are still outstanding.
  native.connections = 19; native.application = native.applied('3', 'applied');
  await publish(s, native.applied('3', 'applied'));
  // The busy owner does not fan out one read per notification.
  expect(sent(s, 'configuration/sourcesRead').length).toBe(3);
  // Step 6: release R2 first. It advances accepted state to application 2 and
  // settles the commit's observation; publication 3 is still owed. R1 was not
  // merely superseded, it was *cancelled* when the save took the read order, so
  // the transferred obligation is discharged by exactly one new authoritative
  // read instead of waiting for a response no owner is left for.
  await deliver(s, r2);
  await screen.findByText(/Source saved\. Native coordination/);
  diagnostics();
  expect(acceptedProjection().application?.version).toBe('2');
  expect(screen.queryByText('Saved process policy is active.')).toBeNull();
  await waitFor(() => expect(sent(s, 'configuration/sourcesRead').length).toBe(4));
  // Step 7: release the cancelled R1. It can neither discharge v3, nor publish
  // a projection, nor arm another read.
  await deliver(s, r1);
  expect(sent(s, 'configuration/sourcesRead').length).toBe(4);
  // Step 8: the one authoritative read the level owed carries application 3.
  await deliver(s, sent(s, 'configuration/sourcesRead')[3]);
  await expectApplied();
  expect(acceptedProjection().application?.version).toBe('3');
  expect(bindings().max_connections).toBe(19);
  expect(screen.getByText('Saved process policy is active.')).toBeTruthy();
  // Exactly one write, no replay, one read per real obligation, nothing after.
  expect(sent(s, 'configuration/sourceWrite')).toHaveLength(1);
  expect(sent(s, 'configuration/sourcesRead').length).toBe(4);
});

it('S12 a held Workspace post-write reread cannot regress a newer authoritative read', async () => {
  // One Workspace source. The committed revision B and the later external
  // revision C deliberately share application version 2, so application
  // version cannot be used as a false ordering shortcut.
  const target: SourceTarget = { kind: 'workspace', directory: '/workspace/A' };
  let revision = 'ws-A';
  let application: ConfigurationApplication | null = { ...cfg3SourceApplication(target), version: '1' };
  const projection = () => {
    const source = cfg3Source();
    source.target = target;
    source.workspace = { path: '/workspace/rustx.toml', revision, authored: { agent: { tools: { builtin: [] } } } };
    source.application = application ? structuredClone(application) : null;
    return source;
  };
  // The Host write commits B and captures reread B, then holds the whole write
  // response until the test releases it. Capture and delivery are separate.
  let captured!: () => void;
  const capturedB = new Promise<void>(resolve => { captured = resolve; });
  let releaseWrite!: () => void;
  const heldWrite = new Promise<void>(resolve => { releaseWrite = resolve; });
  const s = new Server();
  s.handlers.set('configuration/sourcesRead', () => ({ type: 'source_settings', projection: projection() }));
  s.handlers.set('configuration/sourceWrite', () => { revision = 'ws-B'; return { type: 'source_settings', projection: projection() }; });
  const host: ProductHostWorkspaces = {
    ...s.workspaceHost,
    configureWorkspace: async (_id, _endpoint, operation) => {
      if (operation.kind === 'write') {
        const acknowledgement = (await s.client.request({ method: 'configuration/sourceWrite', params: { target, expected_revision: operation.expected_revision, mutation: operation.mutation } }, 'source_settings')).projection;
        const reread: WorkspaceConfigurationReread = { status: 'observed', projection: (await s.client.request({ method: 'configuration/sourcesRead', params: { target } }, 'source_settings')).projection };
        captured();
        await heldWrite;
        return { kind: 'write', commit: { acknowledgement, reread } };
      }
      if (operation.kind === 'reconcile') await s.client.request({ method: 'configuration/reconcile', params: { target } }, 'configuration_application');
      return { kind: operation.kind, projection: (await s.client.request({ method: 'configuration/sourcesRead', params: { target } }, 'source_settings')).projection };
    },
  };
  await s.connect();
  render(<SettingsSurface client={s.client} target={workspaceSettingsTarget('A', 'A')} host={host} />);
  await screen.findByText(/Revision: ws-A/);
  // B and C will both settle at application version 2; only the authored
  // revision distinguishes them.
  application = { ...cfg3SourceApplication(target), version: '2' };
  fireEvent.click(screen.getByRole('button', { name: 'Tools' }));
  fireEvent.click(screen.getByLabelText('read'));
  fireEvent.click(screen.getByRole('button', { name: 'Save Native Tools' }));
  await capturedB;
  // Step: external authority advances to C while the write response is held.
  revision = 'ws-C';
  await publish(s, { ...cfg3SourceApplication(target), version: '2' });
  await screen.findByText(/Revision: ws-C/);
  // Step: release the held response carrying the equal-version reread B.
  releaseWrite();
  await screen.findByText(/Source saved\. Native coordination/);
  // The newer accepted projection C is never regressed by the late, stale B.
  expect(screen.getByText(/Revision: ws-C/)).toBeTruthy();
  expect(screen.queryByText(/Revision: ws-B/)).toBeNull();
  expect(sent(s, 'configuration/sourceWrite')).toHaveLength(1);
  // No replay, and convergence stays bounded to the real commit obligation.
  await waitFor(() => expect(screen.getByText(/Revision: ws-C/)).toBeTruthy());
  expect(sent(s, 'configuration/sourceWrite')).toHaveLength(1);
  expect(sent(s, 'configuration/sourcesRead').length).toBeLessThanOrEqual(5);
});

/** One Workspace source whose authored revision and application version advance
 * separately, for the S13/S14 ordering contracts below. */
function workspaceSource() {
  const target: SourceTarget = { kind: 'workspace', directory: '/workspace/A' };
  const native = {
    target, revision: 'ws-A', application: { ...cfg3SourceApplication(target), version: '1' } as ConfigurationApplication,
    projection(): SourceSettings {
      const source = cfg3Source();
      source.target = target;
      source.workspace = { path: '/workspace/rustx.toml', revision: native.revision, authored: { agent: { tools: { builtin: [] } } } };
      source.application = structuredClone(native.application);
      return source;
    },
  };
  return native;
}
function barrier() {
  let release!: () => void;
  const reached = new Promise<void>(resolve => { release = resolve; });
  return { reached, release };
}
/** Drain the microtask queue deterministically. No timer, no sleep: every
 * continuation these tests release is a promise continuation. */
const flush = () => act(async () => { for (let turn = 0; turn < 32; turn++) await Promise.resolve(); });

it('S13 a write-owned reread cannot commit over a newer read that was only initiated', async () => {
  // The defect this pins: ordering by the newest *accepted* projection lets a
  // held reread land while a newer authoritative read is merely pending, so the
  // older projection becomes visible and the newer read then fails on top of it.
  const native = workspaceSource();
  let readFails = false;
  const captured = barrier(), held = barrier();
  const s = new Server();
  s.handlers.set('configuration/sourcesRead', () => {
    if (readFails) throw new RpcFailure({ code: -32000, message: 'authoritative read unavailable', data: { kind: 'operation_failed' } });
    return { type: 'source_settings', projection: native.projection() };
  });
  s.handlers.set('configuration/sourceWrite', () => { native.revision = 'ws-B'; return { type: 'source_settings', projection: native.projection() }; });
  const host: ProductHostWorkspaces = {
    ...s.workspaceHost,
    configureWorkspace: async (_id, _endpoint, operation) => {
      if (operation.kind === 'write') {
        const acknowledgement = (await s.client.request({ method: 'configuration/sourceWrite', params: { target: native.target, expected_revision: operation.expected_revision, mutation: operation.mutation } }, 'source_settings')).projection;
        // Native has committed B and this operation owns its authoritative
        // reread of B; the whole Host response is then held.
        const reread: WorkspaceConfigurationReread = { status: 'observed', projection: (await s.client.request({ method: 'configuration/sourcesRead', params: { target: native.target } }, 'source_settings')).projection };
        captured.release(); await held.reached;
        return { kind: 'write', commit: { acknowledgement, reread } };
      }
      return { kind: operation.kind, projection: (await s.client.request({ method: 'configuration/sourcesRead', params: { target: native.target } }, 'source_settings')).projection };
    },
  };
  await s.connect();
  render(<SettingsSurface client={s.client} target={workspaceSettingsTarget('A', 'A')} host={host} />);
  // Step 1: Workspace Settings is open at revision A.
  await screen.findByText(/Revision: ws-A/);
  // Step 2: a Workspace save whose write-owned reread captured revision B.
  fireEvent.click(screen.getByRole('button', { name: 'Tools' }));
  fireEvent.click(screen.getByLabelText('read'));
  fireEvent.click(screen.getByRole('button', { name: 'Save Native Tools' }));
  // Step 3: hold the write response carrying that reread.
  await captured.reached;
  // Step 4/5: a newer authoritative read N+1 is initiated and stays pending. It
  // is never accepted, so only read *reservation* can order it ahead of the
  // held reread.
  s.held.add('configuration/sourcesRead');
  native.application = { ...cfg3SourceApplication(native.target), version: '2' };
  await publish(s, native.application);
  await waitFor(() => expect(sent(s, 'configuration/sourcesRead')).toHaveLength(3));
  const newer = sent(s, 'configuration/sourcesRead')[2];
  // Step 6: release the older write-owned reread N.
  held.release();
  await flush();
  // Step 7: B is not adopted merely because N+1 has not completed yet, and the
  // saved notice waits for the read that actually owns the read sequence rather
  // than racing it with a redundant read of its own.
  expect(screen.queryByText(/Revision: ws-B/)).toBeNull();
  expect(screen.getByText(/Revision: ws-A/)).toBeTruthy();
  // The commit is definitive, but the saved notice is truthful only against the
  // projection this presentation is actually showing, so it waits for the read
  // that owns the read sequence instead of racing it with a redundant one.
  expect(screen.queryByText(/Source saved/)).toBeNull();
  expect(screen.queryByRole('alert')).toBeNull();
  expect(sent(s, 'configuration/sourcesRead')).toHaveLength(3);
  // Step 8: reject N+1.
  readFails = true;
  await deliver(s, newer);
  // Step 9: the current read failure is reported truthfully, by the read that
  // owns the read sequence — and the definitive commit is still reported saved.
  await screen.findByText(/Source read failed/);
  expect(screen.getByText(/authoritative read unavailable/)).toBeTruthy();
  await screen.findByText(/Source saved\. Native coordination/);
  // Step 10: the older B projection never became authoritative presentation.
  expect(screen.queryByText(/Revision: ws-B/)).toBeNull();
  expect(screen.getByText(/Revision: ws-A/)).toBeTruthy();
  // Steps 11/12: exactly one source write, no replay, and the still-unobserved
  // commit drives at most one more bounded convergence read.
  await flush();
  expect(sent(s, 'configuration/sourceWrite')).toHaveLength(1);
  expect(sent(s, 'configuration/sourcesRead').length).toBeLessThanOrEqual(4);
});

it('S14 a superseded write-owned reread failure publishes no read error and leaves its commit to convergence', async () => {
  const native = workspaceSource();
  const captured = barrier(), held = barrier();
  const s = new Server();
  s.handlers.set('configuration/sourcesRead', () => ({ type: 'source_settings', projection: native.projection() }));
  s.handlers.set('configuration/sourceWrite', () => { native.revision = 'ws-B'; return { type: 'source_settings', projection: native.projection() }; });
  const host: ProductHostWorkspaces = {
    ...s.workspaceHost,
    configureWorkspace: async (_id, _endpoint, operation) => {
      if (operation.kind === 'write') {
        const acknowledgement = (await s.client.request({ method: 'configuration/sourceWrite', params: { target: native.target, expected_revision: operation.expected_revision, mutation: operation.mutation } }, 'source_settings')).projection;
        // The commit is definitive; only this operation's own reread failed.
        const reread: WorkspaceConfigurationReread = { status: 'failed', error: new Error('authoritative reread unavailable') };
        captured.release(); await held.reached;
        return { kind: 'write', commit: { acknowledgement, reread } };
      }
      return { kind: operation.kind, projection: (await s.client.request({ method: 'configuration/sourcesRead', params: { target: native.target } }, 'source_settings')).projection };
    },
  };
  await s.connect();
  render(<SettingsSurface client={s.client} target={workspaceSettingsTarget('A', 'A')} host={host} />);
  await screen.findByText(/Revision: ws-A/);
  fireEvent.click(screen.getByRole('button', { name: 'Tools' }));
  fireEvent.click(screen.getByLabelText('read'));
  fireEvent.click(screen.getByRole('button', { name: 'Save Native Tools' }));
  await captured.reached;
  // A newer authoritative read is initiated and held pending.
  s.held.add('configuration/sourcesRead');
  native.application = { ...cfg3SourceApplication(native.target), version: '2' };
  await publish(s, native.application);
  await waitFor(() => expect(sent(s, 'configuration/sourcesRead')).toHaveLength(2));
  held.release();
  await flush();
  // The superseded reread failure is not this presentation's read state: the
  // newer initiated read owns it, the target stays valid, and nothing is
  // reported until that read settles.
  expect(screen.queryByText(/Saved, but the authoritative reread failed/)).toBeNull();
  expect(screen.queryByText(/Source read failed/)).toBeNull();
  expect(screen.queryByRole('alert')).toBeNull();
  // The superseded reread is silent in both directions: it publishes no read
  // failure, and it settles no saved notice. The newer initiated read owns both.
  expect(screen.queryByText(/Source saved/)).toBeNull();
  // The newer authoritative read settles and owns the presentation; the
  // definitive commit is then reported saved against the revision that read
  // actually carries.
  s.held.delete('configuration/sourcesRead');
  await settleReads(s, 1);
  await screen.findByText(/Source saved\. Native coordination/);
  expect(screen.queryByText(/Saved, but the authoritative reread failed/)).toBeNull();
  await waitFor(() => expect(screen.getByText(/Revision: ws-B/)).toBeTruthy());
  await waitFor(() => expect(sent(s, 'configuration/sourcesRead').length).toBeLessThanOrEqual(4));
  expect(sent(s, 'configuration/sourceWrite')).toHaveLength(1);
});
