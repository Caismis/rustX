import { applicationReceipt, contentAdvance } from './fixture';
// @vitest-environment jsdom
import { afterEach, expect, it } from 'vitest';
import { act, cleanup, fireEvent, render, screen, waitFor } from '@testing-library/react';
import { SessionConfiguration } from '../src/app/SessionConfiguration';
import type { ConfigurationApplication, RuntimeClientEvent } from '../../protocol/app-server/v73';
import { cfg3Application } from './cfg3-data';
import { cfg3Client, cfg3Session } from './cfg3-fixture';
import { Server, snapshot } from './fixture';
import { sameValue, selectShell, useClientSelector } from '../src/client/selectors';
afterEach(cleanup);
const servers: Server[] = [];
afterEach(() => { for (const s of servers.splice(0)) s.client.disconnect(); });
const cfg3Reads = (s: ReturnType<typeof cfg3Client>) => s.request.mock.calls.filter(([op]) => op.method === 'session/configuration');
const reconciles = (s: ReturnType<typeof cfg3Client>) => s.request.mock.calls.filter(([op]) => op.method === 'configuration/reconcile');
const rendered = (s: ReturnType<typeof cfg3Client>) => <SessionConfiguration client={s.client} view={s.state.views[cfg3Session]}/>;
const waiting = /apply automatically after the current work settles/;
const unavailableLine = () => screen.queryByText(/Configuration status unavailable/);

it('C13 a prepared change only reports that it applies after current work: nothing confirms or schedules it', async () => {
  const s = cfg3Client(); s.source.application = cfg3Application();
  render(rendered(s));
  await screen.findByText(waiting);
  expect(screen.queryAllByRole('button')).toHaveLength(0);
  expect(s.request.mock.calls.filter(([op]) => op.method !== 'session/configuration')).toHaveLength(0);
});

it('C13 success is silent: an applied application renders no banner', async () => {
  const s = cfg3Client(); s.source.application = { ...cfg3Application(), units: { execution_policy: { status: 'applied' }, instructions: { status: 'applied' } } };
  render(rendered(s));
  await waitFor(() => expect(cfg3Reads(s)).toHaveLength(1));
  await act(async () => {});
  expect(screen.queryByLabelText('Session configuration')).toBeNull();
});

it.each([
  ['preparing', { status: 'preparing' as const }, /Preparing configuration changes/],
  ['failed', { status: 'failed' as const, diagnostic: 'resource failed' }, /keep their previous configuration/],
  ['uncertain', { status: 'uncertain' as const, diagnostic: 'journal unavailable' }, /could not be recorded/],
  ['restart', { status: 'process_restart' as const }, /after the App Server restarts/],
])('C13 a %s unit shows its own compact line', async (_, unit, text) => {
  const s = cfg3Client(); s.source.application = { ...cfg3Application(), units: { capabilities: unit } };
  render(rendered(s));
  await screen.findByText(text);
  if ('diagnostic' in unit) expect(screen.getByText(new RegExp(unit.diagnostic))).toBeTruthy();
});

it('C14 a failure offers native Retry, which rescans each owner once and never replays a save', async () => {
  const s = cfg3Client(); s.source.application = { ...cfg3Application(), units: { capabilities: { status: 'failed', diagnostic: 'resource failed' } } };
  render(rendered(s));
  fireEvent.click(await screen.findByRole('button', { name: 'Retry' }));
  await waitFor(() => expect(reconciles(s)).toHaveLength(2));
  expect(reconciles(s).map(([op]) => op.params)).toEqual([{ target: { kind: 'user' } }, { target: { kind: 'workspace', directory: '/workspace/A' } }]);
  expect(s.request.mock.calls.filter(([op]) => op.method === 'configuration/sourceWrite')).toHaveLength(0);
});

it('C13 a failed reconnect read retains the pending observation and never asserts up to date', async () => {
  let unavailable = false;
  const s = cfg3Client(async op => { if (unavailable && op.method === 'session/configuration') throw new Error('unavailable'); });
  s.source.application = cfg3Application();
  render(rendered(s));
  await screen.findByText(waiting); unavailable = true;
  act(() => s.publish({ connection: 'stale', generation: 2 }));
  act(() => s.publish({ connection: 'connected' }));
  await screen.findByText(/Configuration status unavailable/);
  expect(screen.getByText(waiting)).toBeTruthy();
  expect(screen.queryByText(/up to date/i)).toBeNull();
});

it('C13 an authoritative answer of no application is known: the banner claims nothing', async () => {
  const s = cfg3Client(); s.source.application = undefined;
  render(rendered(s));
  await waitFor(() => expect(cfg3Reads(s)).toHaveLength(1));
  await act(async () => {});
  expect(screen.queryByLabelText('Session configuration')).toBeNull();
  expect(unavailableLine()).toBeNull();
});

/** App.tsx shape exactly: the shell projection's Session view. */
function Harness({ s }: { s: Server }) {
  const shell = useClientSelector(s.client, selectShell, sameValue);
  return <SessionConfiguration client={s.client} view={shell.views.A}/>;
}
const reads = (s: Server) => s.requests.filter(item => item.request.method === 'session/configuration');
async function session(application: ConfigurationApplication | null) {
  const s = new Server(); servers.push(s);
  s.snapshots.set('A', snapshot('A'));
  s.handlers.set('session/configuration', () => ({ type: 'session_configuration', projection: applicationReceipt(application) }));
  await s.attached('A');
  render(<Harness s={s}/>);
  await waitFor(() => expect(reads(s)).toHaveLength(1));
  return s;
}
function emit(s: Server, event: RuntimeClientEvent) {
  s.cursor++;
  s.socket.deliver({ jsonrpc: '2.0', method: 'session/event', params: { target: s.target('A'), cursor: String(s.cursor), event } });
}

it('C18 hundreds of streamed deltas never read Session configuration and never flicker the banner', async () => {
  const s = await session({ ...cfg3Application(), scope: 'A' });
  await screen.findByText(waiting);
  await act(async () => emit(s, { type: 'attempt_started', attempt_id: 'attempt' }));
  await act(async () => emit(s, { type: 'assistant_message_started', attempt_id: 'attempt', message_id: 'message' }));
  for (let delta = 0; delta < 300; delta++) {
    await act(async () => emit(s, contentAdvance({ attempt_id: 'attempt', message_id: 'message', block_index: 0, kind: 'text', delta: 'x' }, delta, delta + 1)));
    expect(unavailableLine()).toBeNull();
    expect(screen.getByText(waiting)).toBeTruthy();
  }
  expect(reads(s)).toHaveLength(1);
});

it('C19 a native publication that applies the change is folded as the observation and removes the banner, never reread', async () => {
  const s = await session({ ...cfg3Application(), scope: 'A' });
  await screen.findByText(waiting);
  await act(async () => {
    s.socket.deliver({ jsonrpc: '2.0', method: 'configuration/changed', params: { application: applicationReceipt({ ...cfg3Application(), scope: 'A', version: '3', units: { execution_policy: { status: 'applied' }, instructions: { status: 'applied' } } }) } });
  });
  await waitFor(() => expect(screen.queryByLabelText('Session configuration')).toBeNull());
  expect(reads(s)).toHaveLength(1);
});
