import { act, cleanup, fireEvent, render, screen } from '@testing-library/react';
import { afterEach, beforeEach, expect, it, vi } from 'vitest';
import { App } from '../src/app/App';
import { computeColumns } from '../src/presentation/layout/columns';
import { Server, interaction } from './fixture';
let server: Server;
beforeEach(() => { localStorage.clear(); server = new Server(); });
afterEach(() => { cleanup(); server.client.disconnect(); vi.useRealTimers(); });
it('Harness column solve preserves the center, concedes the right panel and keeps the 56px rail', () => {
  expect(computeColumns(1440, 280, 648)).toEqual({ sidebar: 280, center: 512, rightbar: 648 });
  expect(computeColumns(1000, 280, 648)).toEqual({ sidebar: 280, center: 400, rightbar: 320 });
  expect(computeColumns(900, 280, 648)).toEqual({ sidebar: 280, center: 620, rightbar: 0 });
  expect(computeColumns(390, 0, 648)).toEqual({ sidebar: 56, center: 334, rightbar: 0 });
});
it('collapse, rail expansion, Inspector and Settings appearance gestures emit no native operations', async () => {
  await server.connect();
  await act(async () => { render(<App client={server.client} workspaceHost={server.workspaceHost} />); });
  const baseline = server.requests.length;
  vi.useFakeTimers();
  fireEvent.click(screen.getByRole('button', { name: 'Collapse Sidebar' }));
  act(() => vi.advanceTimersByTime(150)); // upstream presentation transition only, never race synchronization
  expect(document.querySelector('[data-sidebar-wide]')?.getAttribute('data-sidebar-wide')).toBe('false');
  expect(screen.getByRole('button', { name: 'New Session' })).toBeTruthy();
  fireEvent.click(screen.getByRole('button', { name: 'Expand Sidebar' }));
  expect(document.querySelector('[data-sidebar-wide]')?.getAttribute('data-sidebar-wide')).toBe('true');
  fireEvent.click(screen.getByRole('button', { name: 'Toggle Inspector' }));
  expect(screen.getByRole('complementary', { name: 'Developer inspector' }).hasAttribute('data-sidebar-right-open')).toBe(true);
  fireEvent.click(screen.getByRole('button', { name: 'Settings' }));
  fireEvent.click(screen.getByRole('button', { name: 'Appearance' }));
  fireEvent.change(screen.getByLabelText('Theme'), { target: { value: 'dark' } });
  expect(document.body.hasAttribute('data-ds-dark-theme')).toBe(true);
  fireEvent.change(screen.getByLabelText('Theme'), { target: { value: 'light' } });
  expect(document.body.hasAttribute('data-ds-dark-theme')).toBe(false);
  expect(server.requests.slice(baseline).every(row => row.request.method === 'configuration/sourcesRead')).toBe(true);
  expect(JSON.stringify(localStorage)).not.toMatch(/workspaceId|cwd|snapshot|interaction/);
});
it('waiting interactions outrank running; queued input alone is not a waiting interaction', async () => {
  server.snapshots.get('A')!.attempt = { attempt_id: 'running', phase: { type: 'running' }, turn: 1 };
  server.snapshots.get('A')!.pending_interactions = [interaction('approval')];
  await server.attached('A');
  await act(async () => { render(<App client={server.client} workspaceHost={server.workspaceHost} />); });
  const row = screen.getByRole('button', { name: 'Open Session A' }).closest('[role=treeitem]')!;
  expect(row.querySelector('[data-state]')?.getAttribute('data-state')).toBe('warning');
  expect(row.textContent).toContain('Waiting for approval');
  await act(async () => { server.snapshots.get('A')!.pending_interactions = []; await server.client.refresh('A'); });
  expect(row.querySelector('[data-state]')?.getAttribute('data-state')).toBe('ongoing');
});
it('native paging sends offsets and does not create browser-owned Session membership', async () => {
  server.handlers.set('session/list', request => {
    if (request.method !== 'session/list') throw Error('wrong request');
    return { type: 'sessions', sessions: [{ id: `page-${request.params.offset}`, name: `Page ${request.params.offset}`, cwd: '/workspace/A', active_node: 'n', updated_at: '2026-09-18T00:00:00Z' }], next_offset: request.params.offset === 0 ? 32 : null };
  });
  await server.connect();
  await act(async () => { render(<App client={server.client} workspaceHost={server.workspaceHost} />); });
  await act(async () => fireEvent.click(screen.getByRole('button', { name: 'Next' })));
  expect(screen.queryByRole('button', { name: 'Open Page 0' })).toBeNull();
  expect(screen.getByRole('button', { name: 'Open Page 32' })).toBeTruthy();
  await act(async () => fireEvent.click(screen.getByRole('button', { name: 'Previous' })));
  expect(server.requests.filter(row => row.request.method === 'session/list').map(row => row.request.params)).toEqual([
    { offset: 0, limit: 32, query: '' }, { offset: 32, limit: 32, query: '' }, { offset: 0, limit: 32, query: '' },
  ]);
  expect(JSON.parse(localStorage.getItem('rustx-console-view-v2')!)).toEqual({ endpoint: 'ws://127.0.0.1:8080/', openViews: [] });
});
