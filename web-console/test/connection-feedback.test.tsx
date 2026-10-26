import { act, cleanup, fireEvent, render, screen } from '@testing-library/react';
import { afterEach, expect, it, vi } from 'vitest';
import { ConnectionFeedback } from '../src/app/ConnectionFeedback';
import { App } from '../src/app/App';
import { ConnectionController } from '../src/connection/controller';
import { Server, endpoint, TOKEN } from './fixture';

let server: Server | undefined;
afterEach(() => { cleanup(); server?.client.disconnect(); localStorage.clear(); vi.useRealTimers(); });

it('one compact reconnect control holds progress then confirms recovery without delaying native readiness', async () => {
  server = new Server(); const connection = new ConnectionController(server.client), settings = vi.fn();
  await connection.connectRemote(endpoint, TOKEN);
  vi.useFakeTimers();
  render(<ConnectionFeedback client={server.client} connection={connection} wide settings={settings}/>);
  expect(screen.queryByText('Connected')).toBeNull();
  act(() => server!.socket.close());
  expect(screen.getByRole('button', { name: 'Reconnect now' }).textContent).toContain('Reconnect');
  await act(async () => fireEvent.click(screen.getByRole('button', { name: 'Reconnect now' })));
  expect(server.client.getSnapshot().connection).toBe('connected');
  expect(screen.getByRole('button', { name: 'Restart connection attempt' }).textContent).toContain('Reconnecting');
  await act(async () => vi.advanceTimersByTime(800));
  expect(screen.getByRole('status', { name: 'Connected' })).toBeTruthy();
  await act(async () => vi.advanceTimersByTime(2000));
  await act(async () => vi.advanceTimersByTime(150));
  expect(screen.queryByRole('status', { name: 'Connected' })).toBeNull();
  expect(server.sockets).toHaveLength(2); expect(settings).not.toHaveBeenCalled();
  expect(server.requests.filter(row => row.request.method === 'turn/start')).toEqual([]);
});

it('a new outage retires recovery confirmation and the rail retains its accessible action', async () => {
  server = new Server(); const connection = new ConnectionController(server.client);
  await connection.connectRemote(endpoint, TOKEN); vi.useFakeTimers();
  render(<ConnectionFeedback client={server.client} connection={connection} wide={false} settings={() => {}}/>);
  act(() => server!.socket.close());
  await act(async () => connection.reconnect()); await act(async () => vi.advanceTimersByTime(800));
  expect(screen.getByRole('status', { name: 'Connected' })).toBeTruthy();
  act(() => server!.socket.close());
  expect(screen.getByRole('button', { name: 'Reconnect now' })).toBeTruthy();
  expect(screen.queryByRole('status', { name: 'Connected' })).toBeNull();
  await act(async () => vi.advanceTimersByTime(3000));
  expect(screen.getByRole('button', { name: 'Reconnect now' })).toBeTruthy();
});

it('App exposes one physical outage control without a conversation-blocking banner or request replay', async () => {
  server = new Server(); const connection = new ConnectionController(server.client);
  await connection.connectRemote(endpoint, TOKEN); await server.client.attach('A');
  localStorage.setItem('rustx-console-view-v2', JSON.stringify({ endpoint, openViews: ['A'] }));
  render(<App client={server.client} workspaceHost={server.workspaceHost} connection={connection}/>);
  const editor = screen.getByRole('textbox', { name: 'Message' });
  act(() => server!.socket.close());
  expect(screen.getAllByRole('button', { name: 'Reconnect now' })).toHaveLength(1);
  expect(screen.queryByLabelText('Session status')).toBeNull();
  expect(screen.queryByText('Configuration status unavailable. Retaining the last observation.')).toBeNull();
  expect(screen.queryByRole('region', { name: 'Connection recovery' })).toBeNull();
  expect(screen.getByRole('textbox', { name: 'Message' })).toBe(editor);
  expect(server.requests.filter(row => row.request.method === 'turn/start')).toEqual([]);
});

it('incompatible versions open settings instead of retrying the same incompatible connection', async () => {
  server = new Server(); server.version = 999;
  const connection = new ConnectionController(server.client), settings = vi.fn();
  await connection.connectRemote(endpoint, TOKEN);
  render(<ConnectionFeedback client={server.client} connection={connection} wide settings={settings}/>);
  fireEvent.click(screen.getByRole('button', { name: 'Connection settings' }));
  expect(settings).toHaveBeenCalledOnce(); expect(server.sockets).toHaveLength(1);
});
