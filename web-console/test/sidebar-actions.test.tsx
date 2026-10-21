import { afterEach, expect, it, vi } from 'vitest';
import { act, cleanup, fireEvent, render, screen, within } from '@testing-library/react';
import { App } from '../src/app/App';
import { SessionPins } from '../src/workspaces/session-pins';
import { RpcFailure } from '../src/client/app-server';
import { Server, endpoint } from './fixture';

let server: Server | undefined;
afterEach(() => { cleanup(); server?.client.disconnect(); vi.unstubAllGlobals(); localStorage.clear(); });
async function mount() {
  localStorage.clear(); server = new Server(); await server.attached('A', 'B');
  localStorage.setItem('rustx-console-view-v2', JSON.stringify({ endpoint, openViews: ['A', 'B'] }));
  const ui = render(<App client={server.client} workspaceHost={server.workspaceHost}/>);
  await act(async () => {}); return ui;
}

it('pins are persistent, ordered, removable and scoped to their service identity', () => {
  const store = new SessionPins('service-A', localStorage);
  store.toggle('first'); store.toggle('second');
  expect(store.getSnapshot()).toEqual(['second', 'first']);
  expect(new SessionPins('service-A', localStorage).getSnapshot()).toEqual(['second', 'first']);
  expect(new SessionPins('service-B', localStorage).getSnapshot()).toEqual([]);
  store.toggle('second'); store.remove('first');
  expect(new SessionPins('service-A', localStorage).getSnapshot()).toEqual([]);
});

it('DSH row actions pin and copy an inactive Session without opening it or touching native work', async () => {
  const writeText = vi.fn(async () => {});
  vi.stubGlobal('navigator', { clipboard: { writeText }, platform: 'Linux', languages: ['en'], language: 'en' });
  await mount(); const before = server!.requests.length;
  const trigger = () => screen.getByRole('button', { name: 'Session actions for Session B' });
  fireEvent.click(trigger());
  expect(screen.getAllByRole('menuitem').map(row => row.textContent)).toEqual(['Pin session', 'Rename', 'Fork session', 'Delete Session', 'Copy Session ID']);
  expect(screen.queryByRole('menuitem', { name: /Archive/ })).toBeNull();
  fireEvent.click(screen.getByRole('menuitem', { name: 'Pin session' }));
  expect(screen.getByRole('img', { name: 'Pinned' })).toBeTruthy();
  expect(screen.getByLabelText('Session title').textContent).toBe('Session A');
  fireEvent.click(trigger()); await act(async () => fireEvent.click(screen.getByRole('menuitem', { name: 'Copy Session ID' })));
  expect(writeText).toHaveBeenCalledWith('B'); expect(screen.getByRole('alert').textContent).toBe('Session ID copied');
  expect(server!.requests.slice(before).filter(row => ['session/attach', 'session/detach', 'turn/cancel', 'session/name'].includes(row.request.method))).toEqual([]);
  fireEvent.click(screen.getByRole('button', { name: 'View options' })); fireEvent.click(screen.getByRole('menuitem', { name: 'Flat view' }));
  expect([...document.querySelectorAll<HTMLButtonElement>('button[data-session-id]')].map(row => row.dataset.sessionId)).toEqual(['B', 'A']);
  fireEvent.click(trigger()); fireEvent.click(screen.getByRole('menuitem', { name: 'Unpin session' }));
  expect(screen.queryByRole('img', { name: 'Pinned' })).toBeNull();
});

it('rename selects the full title, blocks IME/empty/duplicate submits, retains a refused draft and restores row focus', async () => {
  await mount();
  const trigger = screen.getByRole('button', { name: 'Session actions for Session B' });
  fireEvent.click(trigger); fireEvent.click(screen.getByRole('menuitem', { name: 'Rename' }));
  const input = screen.getByRole('textbox', { name: 'Session name' }) as HTMLInputElement;
  expect(document.activeElement).toBe(input); expect([input.selectionStart, input.selectionEnd]).toEqual([0, input.value.length]);
  fireEvent.compositionStart(input); fireEvent.keyDown(input, { key: 'Enter' }); fireEvent.compositionEnd(input);
  expect(server!.requests.some(row => row.request.method === 'session/name')).toBe(false);
  fireEvent.change(input, { target: { value: ' ' } }); expect(screen.getByRole('button', { name: 'Rename' })).toHaveProperty('disabled', true);
  fireEvent.change(input, { target: { value: '  Native title  ' } });
  server!.handlers.set('session/name', () => { throw new RpcFailure({ code: -32602, message: 'Fixture name refused' }); });
  await act(async () => fireEvent.keyDown(input, { key: 'Enter' }));
  expect(screen.getByRole('alert').textContent).toContain('Fixture name refused'); expect(input.value).toBe('  Native title  ');
  const dialog = screen.getByRole('dialog', { name: 'Rename session' });
  fireEvent.click(within(dialog).getByRole('button', { name: 'Cancel' })); expect(document.activeElement).toBe(screen.getByRole('button', { name: 'Open Session B' }));
  expect(server!.requests.filter(row => row.request.method === 'session/name')).toHaveLength(1);
});
