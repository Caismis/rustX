import { act, cleanup, fireEvent, render, screen, waitFor } from '@testing-library/react';
import { afterEach, expect, it, vi } from 'vitest';
import { NewConversation } from '../src/app/new-conversation/NewConversation';
import { RpcFailure } from '../src/client/app-server';
import { WorkspaceHostError } from '../src/workspaces/host';
import { cfg3Source } from './cfg3-data';
import { Server } from './fixture';
let server: Server;
afterEach(() => { cleanup(); server?.client.disconnect(); });
async function mount(current = () => true) {
  server = new Server(); await server.connect();
  const source = { ...cfg3Source(), target: { kind: 'workspace' as const, directory: '/workspace' }, prospective_approval_mode: 'policy' as const };
  const host = { ...server.workspaceHost,
    resolveWorkspace: vi.fn(async () => ({ cwd: '/workspace' })),
    configureWorkspace: vi.fn(async () => ({ kind: 'read' as const, projection: source })),
  };
  const opened = vi.fn();
  await act(async () => { render(<NewConversation client={server.client} host={host} initialWorkspace="workspace-a" current={current} opened={opened}/>); });
  fireEvent.change(screen.getByRole('textbox', { name: 'Message' }), { target: { value: 'Preserved draft' } });
  await waitFor(() => expect((screen.getByRole('button', { name: 'Send' }) as HTMLButtonElement).disabled).toBe(false));
  return { host, opened };
}
it.each(['resolve', 'create'] as const)('known %s rejection keeps the actual composer editable and retries only on a new gesture', async phase => {
  const { host, opened } = await mount();
  if (phase === 'resolve') host.resolveWorkspace.mockRejectedValueOnce(new WorkspaceHostError('Workspace revoked'));
  else server.handlers.set('session/create', () => { throw new RpcFailure({ code: -32000, message: 'Create rejected' }); });
  await act(async () => fireEvent.click(screen.getByRole('button', { name: 'Send' })));
  expect(screen.getByRole('alert').textContent).toContain('No Session was created');
  const input = screen.getByRole('textbox', { name: 'Message' }) as HTMLTextAreaElement;
  expect(input.disabled).toBe(false); expect(input.value).toBe('Preserved draft'); expect(opened).not.toHaveBeenCalled();
  const calls = () => server.requests.filter(r => r.request.method === 'session/create');
  expect(calls()).toHaveLength(phase === 'resolve' ? 0 : 1);
  server.held.add('session/create');
  fireEvent.change(input, { target: { value: 'Corrected draft' } });
  await act(async () => fireEvent.click(screen.getByRole('button', { name: 'Send' })));
  expect(calls()).toHaveLength(phase === 'resolve' ? 1 : 2);
});
it('uncertain creation preserves the draft, refuses another Send and requires native inspection', async () => {
  const { host, opened } = await mount();
  const previousResolutions = host.resolveWorkspace.mock.calls.length;
  host.resolveWorkspace.mockRejectedValueOnce(new WorkspaceHostError('Unknown outcome', undefined, true));
  await act(async () => fireEvent.click(screen.getByRole('button', { name: 'Send' })));
  expect(screen.getByRole('alert').textContent).toContain('Inspect native Sessions');
  expect((screen.getByRole('button', { name: 'Send' }) as HTMLButtonElement).disabled).toBe(true);
  expect((screen.getByRole('textbox', { name: 'Message' }) as HTMLTextAreaElement).value).toBe('Preserved draft');
  fireEvent.click(screen.getByRole('button', { name: 'Send' }));
  expect(host.resolveWorkspace).toHaveBeenCalledTimes(previousResolutions + 1);
  expect(opened).not.toHaveBeenCalled();
});

it('confirmed create survives immediate authority replacement in the recovery presentation without stale attachment', async () => {
  let current = true; const { opened } = await mount(() => current);
  server.held.add('session/create');
  server.handlers.set('session/create', () => ({ type: 'session_transition', session: { id: 'native-committed', active_node: 'node-native', active_conversation_id: 'conversation-native', node_count: 1, created_at: '0', updated_at: '0' } }));
  await act(async () => fireEvent.click(screen.getByRole('button', { name: 'Send' })));
  const request = await server.waitFor('session/create', 1);
  await act(async () => { server.reply(request); current = false; });
  expect(screen.getByRole('alert').textContent).toContain('Session native-committed was created');
  expect(server.requests.filter(r => ['session/attach', 'session/setModel', 'turn/start'].includes(r.request.method))).toHaveLength(0);
  expect(opened).not.toHaveBeenCalled(); // obsolete navigation cannot take over a newer route
  expect((screen.getByRole('button', { name: 'Send' }) as HTMLButtonElement).disabled).toBe(true);
});
