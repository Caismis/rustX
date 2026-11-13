import { act, cleanup, fireEvent, render } from '@testing-library/react';
import { afterEach, expect, it, vi } from 'vitest';
import { WorkspaceFile, fileTabView, type WorkbenchCall } from '../src/app/components/WorkbenchFiles';

afterEach(cleanup);

it('reveals the file by default and opens it in an application only after an explicit selection', async () => {
  const call = vi.fn<WorkbenchCall>(async request => request.kind === 'applications'
    ? { applications: { available: true, applications: [{ id: 'code', label: 'Visual Studio Code' }, { id: 'files', label: 'File manager' }] } }
    : {});
  let ui!: ReturnType<typeof render>;
  await act(async () => { ui = render(<WorkspaceFile path="models/bracket.step" call={call} view={fileTabView()} onOpen={() => {}}/>); });
  await act(async () => fireEvent.click(ui.getByRole('button', { name: 'Open file location' })));
  expect(call).toHaveBeenCalledWith({ kind: 'open', path: 'models/bracket.step', directory: false, application: 'files' });
  await act(async () => fireEvent.click(ui.getByRole('button', { name: 'Choose application' })));
  await act(async () => fireEvent.click(ui.getByRole('menuitem', { name: 'Visual Studio Code' })));
  expect(call).toHaveBeenLastCalledWith({ kind: 'open', path: 'models/bracket.step', directory: false, application: 'code' });
  expect(ui.getByRole('button', { name: 'Open with Visual Studio Code' })).toBeTruthy();
});

it('does not silently substitute a file-opening application when a file manager is unavailable', async () => {
  const call = vi.fn<WorkbenchCall>(async request => request.kind === 'applications'
    ? { applications: { available: true, applications: [{ id: 'code', label: 'Visual Studio Code' }] } }
    : {});
  let ui!: ReturnType<typeof render>;
  await act(async () => { ui = render(<WorkspaceFile path="models/bracket.step" call={call} view={fileTabView()} onOpen={() => {}}/>); });
  expect((ui.getByRole('button', { name: 'Open file location' }) as HTMLButtonElement).disabled).toBe(true);
  expect((ui.getByRole('button', { name: 'Choose application' }) as HTMLButtonElement).disabled).toBe(false);
  expect(call.mock.calls.some(([request]) => request.kind === 'open')).toBe(false);
});
