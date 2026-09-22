// @vitest-environment jsdom
import { afterEach, expect, it } from 'vitest';
import { act, cleanup, fireEvent, render, screen, waitFor } from '@testing-library/react';
import { SessionConfiguration } from '../src/app/SessionConfiguration';
import { OutcomeUncertain } from '../src/client/app-server';
import type { MethodResult } from '../../protocol/app-server/v18';
import { cfg3Application } from './cfg3-data';
import { cfg3Client, cfg3Session } from './cfg3-fixture';
import { snapshot } from './fixture';
afterEach(cleanup);
const writes = (s: ReturnType<typeof cfg3Client>) => s.request.mock.calls.filter(([op]) => op.method === 'session/adoptConfiguration');

it('C13 ready native eligibility submits exactly the inspected candidate and binding', async () => {
 const s = cfg3Client(); s.source.application = cfg3Application();
 const candidate = structuredClone(s.source.application.candidate);
 render(<SessionConfiguration client={s.client} view={s.state.views[cfg3Session]}/>);
 fireEvent.click(await screen.findByRole('button', { name: 'Adopt configuration' }));
 await waitFor(() => expect(screen.queryByRole('button', { name: 'Adopt configuration' })).toBeNull());
 expect(writes(s)[0][0]).toEqual({ method: 'session/adoptConfiguration', params: { session_id: cfg3Session, candidate: candidate!.identity, expected_binding: candidate!.expected_binding } });
 expect(writes(s)).toHaveLength(1);
});

it('C15 native Busy remains visible and becomes eligible on settlement observation without polling', async () => {
 const s = cfg3Client(); s.source.application = { ...cfg3Application(), eligibility: { status: 'busy' } };
 const ui = render(<SessionConfiguration client={s.client} view={s.state.views[cfg3Session]}/>);
 expect((await screen.findByRole('button', { name: 'Adopt configuration' }) as HTMLButtonElement).disabled).toBe(true);
 expect(screen.getByText(/Session work must settle/)).toBeTruthy();
 s.source.application.eligibility = { status: 'eligible' };
 ui.rerender(<SessionConfiguration client={s.client} view={{ ...s.state.views[cfg3Session], snapshot: snapshot() }}/>);
 await waitFor(() => expect((screen.getByRole('button', { name: 'Adopt configuration' }) as HTMLButtonElement).disabled).toBe(false));
 expect(writes(s)).toHaveLength(0);
});

it.each(['Busy', 'NotReady', 'Conflict'])('C14 %s rereads and retains the candidate without automatic replay', async rejection => {
 const s = cfg3Client(async op => { if (op.method === 'session/adoptConfiguration') throw new Error(rejection); });
 s.source.application = cfg3Application();
 render(<SessionConfiguration client={s.client} view={s.state.views[cfg3Session]}/>);
 fireEvent.click(await screen.findByRole('button', { name: 'Adopt configuration' }));
 await screen.findByRole('alert');
 await waitFor(() => expect((screen.getByRole('button', { name: 'Adopt configuration' }) as HTMLButtonElement).disabled).toBe(false));
 expect(writes(s)).toHaveLength(1);
 expect(s.request.mock.calls.filter(([op]) => op.method === 'session/configuration')).toHaveLength(2);
});

it('C16 lost adoption response rereads committed native state exactly once, never replays', async () => {
 const s = cfg3Client(async (op, source) => {
   if (op.method === 'session/adoptConfiguration') {
     source.application = { ...cfg3Application(), version: '3', candidate: null, units: {} };
     throw new OutcomeUncertain();
   }
 });
 s.source.application = cfg3Application();
 render(<SessionConfiguration client={s.client} view={s.state.views[cfg3Session]}/>);
 fireEvent.click(await screen.findByRole('button', { name: 'Adopt configuration' }));
 await waitFor(() => expect(screen.queryByRole('button', { name: 'Adopt configuration' })).toBeNull());
 expect(writes(s)).toHaveLength(1);
 expect(s.request.mock.calls.filter(([op]) => op.method === 'session/configuration')).toHaveLength(2);
});

it('C16 late success A cannot hide newly observed candidate B; acknowledgement alone never hides', async () => {
 let release!: (value: MethodResult) => void;
 const pending = new Promise<MethodResult>(resolve => { release = resolve; });
 const s = cfg3Client(async op => { if (op.method === 'session/adoptConfiguration') return pending; });
 s.source.application = cfg3Application();
 const ui = render(<SessionConfiguration client={s.client} view={s.state.views[cfg3Session]}/>);
 fireEvent.click(await screen.findByRole('button', { name: 'Adopt configuration' }));
 s.source.application = { ...cfg3Application(), version: '4', candidate: { identity: { input_revision: 'B', attempt: '4' }, expected_binding: '2', impact: 'prefix_changed' } };
 ui.rerender(<SessionConfiguration client={s.client} view={{ ...s.state.views[cfg3Session], snapshot: snapshot() }}/>);
 await waitFor(() => expect(s.request.mock.calls.filter(([op]) => op.method === 'session/configuration')).toHaveLength(2));
 await act(async () => { release({ type: 'configuration_application', application: { ...cfg3Application(), version: '3', candidate: null } }); await pending; });
 await waitFor(() => expect((screen.getByRole('button', { name: 'Adopt configuration' }) as HTMLButtonElement).disabled).toBe(false));
 expect(writes(s)).toHaveLength(1);
 fireEvent.click(screen.getByRole('button', { name: 'Adopt configuration' }));
 await waitFor(() => expect(writes(s)).toHaveLength(2));
 expect(writes(s)[1][0]).toMatchObject({ params: { candidate: { input_revision: 'B', attempt: '4' }, expected_binding: '2' } });
});

it('C13 failed refresh retains pending observation and never asserts up to date', async () => {
 let unavailable = false;
 const s = cfg3Client(async op => { if (unavailable && op.method === 'session/configuration') throw new Error('unavailable'); });
 s.source.application = cfg3Application();
 const ui = render(<SessionConfiguration client={s.client} view={s.state.views[cfg3Session]}/>);
 await screen.findByRole('button', { name: 'Adopt configuration' }); unavailable = true;
 ui.rerender(<SessionConfiguration client={s.client} view={{ ...s.state.views[cfg3Session], snapshot: snapshot() }}/>);
 await screen.findByText(/Configuration status unavailable/);
 expect(screen.getByText(/Prepared configuration is waiting/)).toBeTruthy();
 expect((screen.getByRole('button', { name: 'Adopt configuration' }) as HTMLButtonElement).disabled).toBe(true);
 expect(screen.queryByText(/up to date/i)).toBeNull();
});

it.each(['preparing', 'failed'] as const)('C13 %s never fabricates an Adopt candidate', async status => {
 const s = cfg3Client(); s.source.application = { ...cfg3Application(), candidate: null, units: { capabilities: status === 'failed' ? { status, diagnostic: 'resource failed' } : { status } } };
 render(<SessionConfiguration client={s.client} view={s.state.views[cfg3Session]}/>);
 await screen.findByText(status === 'preparing' ? /Preparing configuration/ : /Some configuration preparation failed/);
 expect(screen.queryByRole('button', { name: 'Adopt configuration' })).toBeNull();
});

// Blocking finding — one adoption attempt's terminal client-side cleanup owns
// only that attempt's own state. It must release that state independently of
// the authoritative reread it then issues.
it('C17 a failed authoritative reread after adoption strands neither busy nor the in-flight guard', async () => {
 let unavailable = false;
 const s = cfg3Client(async (op, source) => {
   if (op.method === 'session/configuration' && unavailable) throw new Error('configuration read unavailable');
   // Native keeps the candidate: this adoption attempt did not commit.
   if (op.method === 'session/adoptConfiguration') { source.application = cfg3Application(); throw new Error('NotReady'); }
 });
 s.source.application = cfg3Application();
 const ui = render(<SessionConfiguration client={s.client} view={s.state.views[cfg3Session]}/>);
 fireEvent.click(await screen.findByRole('button', { name: 'Adopt configuration' }));
 unavailable = true;
 // The rejection and the failed reread are two separate visible facts; neither
 // replays the adoption.
 await screen.findByText(/Configuration status unavailable/);
 expect(screen.getByRole('alert')).toBeTruthy();
 expect(writes(s)).toHaveLength(1);
 // `busy` cleared and the candidate is retained, merely not actionable while
 // native status is unknown.
 await waitFor(() => expect(screen.getByRole('button', { name: 'Adopt configuration' })).toBeTruthy());
 expect((screen.getByRole('button', { name: 'Adopt configuration' }) as HTMLButtonElement).disabled).toBe(true);
 // A later native observation makes the same candidate actionable again, which
 // is only possible if the in-flight guard was released.
 unavailable = false;
 ui.rerender(<SessionConfiguration client={s.client} view={{ ...s.state.views[cfg3Session], snapshot: snapshot() }}/>);
 await waitFor(() => expect((screen.getByRole('button', { name: 'Adopt configuration' }) as HTMLButtonElement).disabled).toBe(false));
 fireEvent.click(screen.getByRole('button', { name: 'Adopt configuration' }));
 await waitFor(() => expect(writes(s)).toHaveLength(2));
});

it('C17 a superseded lifetime\'s reread cannot clear the busy state of the adoption attempt that replaced it', async () => {
 let held!: (value: MethodResult) => void;
 const pending = new Promise<MethodResult>(resolve => { held = resolve; });
 let reads = 0, holdRead = false, holdAdopt = false;
 const s = cfg3Client(async (op, source) => {
   if (op.method === 'session/configuration') { ++reads; if (holdRead) { holdRead = false; return pending; } }
   if (op.method === 'session/adoptConfiguration') {
     if (holdAdopt) return new Promise<MethodResult>(() => {});
     source.application = cfg3Application();
     throw new Error('NotReady');
   }
 });
 s.source.application = cfg3Application();
 const ui = render(<SessionConfiguration client={s.client} view={s.state.views[cfg3Session]}/>);
 fireEvent.click(await screen.findByRole('button', { name: 'Adopt configuration' }));
 // The first attempt's own reread is held open across a connection lifetime
 // change, so it settles long after the lifetime that issued it ended.
 holdRead = true;
 await waitFor(() => expect(reads).toBe(2));
 const generation = { ...s.state, generation: 2 };
 s.client.getSnapshot = () => generation;
 ui.rerender(<SessionConfiguration client={s.client} view={s.state.views[cfg3Session]}/>);
 await waitFor(() => expect((screen.getByRole('button', { name: 'Adopt configuration' }) as HTMLButtonElement).disabled).toBe(false));
 // A second attempt owns the current lifetime and is in flight.
 holdAdopt = true;
 fireEvent.click(screen.getByRole('button', { name: 'Adopt configuration' }));
 await waitFor(() => expect((screen.getByRole('button', { name: 'Adopt configuration' }) as HTMLButtonElement).disabled).toBe(true));
 await act(async () => { held({ type: 'session_configuration', application: cfg3Application() } as never); await pending; });
 // The superseded lifetime's settlement owns none of the current attempt's
 // state: the in-flight adoption stays in flight.
 expect((screen.getByRole('button', { name: 'Adopt configuration' }) as HTMLButtonElement).disabled).toBe(true);
 expect(writes(s)).toHaveLength(2);
});
