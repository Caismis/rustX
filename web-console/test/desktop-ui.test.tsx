import { act, cleanup, fireEvent, render, screen } from '@testing-library/react';
import { afterEach, expect, it, vi } from 'vitest';
import { OpenWorkspace } from '../src/app/agent/OpenWorkspace';
import { WorkspaceAuthority } from '../src/workspaces/authority';
import { WorkspaceHostError, type ProductHostWorkspaces } from '../src/workspaces/host';
import type { DesktopCatalog, DesktopLaunch } from '../src/workspaces/desktop';
import type { AppServerClient } from '../src/client/app-server';
import { localeController } from '../src/locale/controller';
afterEach(cleanup);
const apps: DesktopCatalog = { available: true, applications: [{ id: 'files', label: 'File manager' }, { id: 'code', label: 'Visual Studio Code' }] };
function gate<T>() { let resolve!: (value: T) => void; const promise = new Promise<T>(r => { resolve = r; }); return { promise, resolve }; }
async function fixture() {
  let authorityId = 'host-one';
  const host = { listWorkspaces: async () => ({ authorityId, endpoint: 'ws://localhost:8080/', workspaces: [], picker: { kind: 'unavailable' as const, reason: 'none' } }),
    desktopCatalog: vi.fn(async (): Promise<DesktopCatalog> => apps), openWorkspace: vi.fn(async (): Promise<DesktopLaunch> => ({ status: 'spawned' })) };
  const authority = new WorkspaceAuthority(host as unknown as ProductHostWorkspaces); await authority.observe();
  const state = { generation: 1, authorityRevision: 1, endpoint: 'ws://localhost:8080/' };
  const client = { getSnapshot: () => state } as unknown as AppServerClient;
  const props = { client, host: host as unknown as ProductHostWorkspaces, authority, target: { session_id: 'A', active_node: 'node-A' }, disabled: false };
  return { props, host, replace: async () => { authorityId = 'host-two'; await authority.observe(); } };
}
it('primary and application menu send exact targets without paths or native browser requests', async () => {
  const f = await fixture(); render(<OpenWorkspace {...f.props}/>);
  await act(async () => fireEvent.click(screen.getByRole('button', { name: 'Open workspace' })));
  expect(f.host.openWorkspace).toHaveBeenCalledExactlyOnceWith({ authorityId: 'host-one', endpoint: 'ws://localhost:8080/' }, f.props.target, 'files');
  expect(screen.getByRole('status').textContent).toContain('Launcher started');
  await act(async () => fireEvent.click(screen.getByRole('button', { name: 'Choose workspace application' })));
  await act(async () => fireEvent.click(screen.getByRole('menuitem', { name: 'Visual Studio Code' })));
  expect(f.host.openWorkspace).toHaveBeenLastCalledWith(expect.anything(), f.props.target, 'code');
});
it('late discovery from a previous Session cannot launch or contaminate its replacement', async () => {
  const f = await fixture(), held = gate<DesktopCatalog>(); f.host.desktopCatalog.mockReturnValueOnce(held.promise);
  const view = render(<OpenWorkspace {...f.props}/>);
  await act(async () => fireEvent.click(screen.getByRole('button', { name: 'Open workspace' })));
  expect(screen.getByRole('button', { name: 'Finding applications…' })).toBeTruthy();
  view.rerender(<OpenWorkspace {...f.props} target={{ session_id: 'B', active_node: 'node-B' }}/>);
  await act(async () => held.resolve(apps));
  expect(f.host.openWorkspace).not.toHaveBeenCalled(); expect(screen.queryByRole('status')).toBeNull();
});
it('late launch acknowledgement stays scoped, never replays, and observer disposal does not cancel the launch', async () => {
  const f = await fixture(), held = gate<DesktopLaunch>(); f.host.openWorkspace.mockReturnValueOnce(held.promise);
  const view = render(<OpenWorkspace {...f.props}/>);
  await act(async () => fireEvent.click(screen.getByRole('button', { name: 'Open workspace' })));
  expect(f.host.openWorkspace).toHaveBeenCalledTimes(1);
  view.rerender(<OpenWorkspace {...f.props} target={{ session_id: 'B', active_node: 'node-B' }}/>);
  await act(async () => held.resolve({ status: 'spawned' }));
  expect(screen.queryByRole('status')).toBeNull(); expect(f.host.openWorkspace).toHaveBeenCalledTimes(1);
});
it('authority replacement fences a held catalog before launch', async () => {
  const f = await fixture(), held = gate<DesktopCatalog>(); f.host.desktopCatalog.mockReturnValueOnce(held.promise);
  render(<OpenWorkspace {...f.props}/>);
  await act(async () => fireEvent.click(screen.getByRole('button', { name: 'Open workspace' })));
  await act(async () => f.replace()); await act(async () => held.resolve(apps));
  expect(f.host.openWorkspace).not.toHaveBeenCalled(); expect(screen.queryByRole('status')).toBeNull();
});
it('unavailable, explicit retry, localized failure and lost acknowledgement are truthful', async () => {
  const f = await fixture(); f.host.desktopCatalog.mockResolvedValueOnce({ available: false, reason: 'headless' });
  render(<OpenWorkspace {...f.props}/>);
  await act(async () => fireEvent.click(screen.getByRole('button', { name: 'Open workspace' })));
  expect(screen.getByRole('status').textContent).toContain('no local desktop'); expect(f.host.openWorkspace).not.toHaveBeenCalled();
  await act(async () => fireEvent.click(screen.getByRole('button', { name: 'Check applications again' })));
  f.host.openWorkspace.mockRejectedValueOnce(new Error('adapter failure'));
  await act(async () => fireEvent.click(screen.getByRole('menuitem', { name: 'File manager' })));
  expect(screen.getByRole('alert').textContent).toContain('adapter failure');
  await act(async () => localeController.setLocale('zh'));
  expect(screen.getByRole('alert').textContent).toContain('无法打开工作区');
  f.host.openWorkspace.mockRejectedValueOnce(new WorkspaceHostError('lost', undefined, true));
  await act(async () => fireEvent.click(screen.getByRole('button', { name: '打开工作区' })));
  expect(screen.getByRole('alert').textContent).toContain('未收到启动确认'); expect(f.host.openWorkspace).toHaveBeenCalledTimes(2);
});
