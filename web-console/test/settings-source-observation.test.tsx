// @vitest-environment jsdom
import { afterEach, expect, it } from 'vitest';
import { act, cleanup, fireEvent, render, screen, waitFor } from '@testing-library/react';
import type { ConfigurationApplication, Request, SourceSettings } from '../../protocol/app-server/v17';
import { Settings } from '../src/app/settings/Settings';
import { cfg3Source } from './cfg3-data';
import { Server } from './fixture';
afterEach(cleanup);

/** Native source authority for one User target. Authoring (revision) and native
 * application (the `source:user` ConfigurationApplication) advance separately,
 * exactly as the App Server owns them. */
class Native {
  connections = 32;
  revision = 'user-1';
  application: ConfigurationApplication | null = null;
  projection(): SourceSettings {
    const source = cfg3Source();
    source.user.revision = this.revision;
    source.process_policy_impacts = { max_connections: 'hot' };
    source.process_bindings = { max_resident_runtimes: 8, max_connections: this.connections };
    source.application = this.application ? structuredClone(this.application) : null;
    return source;
  }
  applied(version: string, status: 'preparing' | 'applied'): ConfigurationApplication {
    return { scope: 'source:user', version, desired: { input_revision: 'input-1', attempt: '1' },
      units: { process_bindings: { status } }, candidate: null, eligibility: { status: 'unavailable' } };
  }
}

async function open(native: Native) {
  const s = new Server();
  s.handlers.set('configuration/sourcesRead', () => ({ type: 'source_settings', projection: native.projection() }));
  s.handlers.set('configuration/sourceWrite', () => ({ type: 'source_settings', projection: native.projection() }));
  await s.connect();
  render(<Settings client={s.client} />);
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
  await screen.findByText(/Source saved. Native coordination/);
  await waitFor(() => expect(sent(s, 'configuration/sourcesRead').length).toBeGreaterThan(1));
  await settleReads(s);
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
  await deliver(s, read);
  await waitFor(() => expect(sent(s, 'configuration/sourcesRead').length).toBeGreaterThanOrEqual(2));
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
  await screen.findByText(/Source saved. Native coordination/);
  expect(bindings().max_connections).toBe(19);
  expect(screen.getByText('Saved process policy is active.')).toBeTruthy();
});

it('S4 the acknowledgement-first order converges under the same observation model', async () => {
  const native = new Native(); const s = await open(native);
  const write = await saveHeldWrite(s, native);
  await deliver(s, write);
  await screen.findByText(/Source saved. Native coordination/);
  await publish(s, native.applied('2', 'applied'));
  await waitFor(() => expect(sent(s, 'configuration/sourcesRead').length).toBeGreaterThan(1));
  await settleReads(s);
  await expectApplied();
  expect(sent(s, 'configuration/sourceWrite')).toHaveLength(1);
});

it('S5 automatic convergence never replays the write or rewrites an unsaved draft', async () => {
  const native = new Native(); const s = await open(native);
  fireEvent.change(screen.getByRole('textbox', { name: 'Root description' }), { target: { value: 'unsaved draft' } });
  const write = await saveHeldWrite(s, native);
  await publish(s, native.applied('2', 'applied'));
  await deliver(s, write);
  await waitFor(() => expect(sent(s, 'configuration/sourcesRead').length).toBeGreaterThan(1));
  await settleReads(s);
  await expectApplied();
  fireEvent.click(screen.getByRole('button', { name: 'General' }));
  expect((screen.getByRole('textbox', { name: 'Root description' }) as HTMLInputElement).value).toBe('unsaved draft');
  expect(sent(s, 'configuration/sourceWrite')).toHaveLength(1);
  // Convergence is bounded by the versions actually published, never a poll.
  expect(sent(s, 'configuration/sourcesRead').length).toBeLessThanOrEqual(3);
  expect(s.requests.every(item => !['session/attach', 'session/create', 'turn/start'].includes(item.request.method))).toBe(true);
});
