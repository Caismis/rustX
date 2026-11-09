import { createRef } from 'react';
import { act, cleanup, fireEvent, render, waitFor, within } from '@testing-library/react';
import { afterEach, expect, it, vi } from 'vitest';
import { Workbench, type WorkbenchHandle } from '../src/app/components/Workbench';
import { WorkbenchPreviewRetention } from '../src/app/workbench-previews';
import { PreviewContentCache } from '../src/client/preview-content';
import { DockController } from '../src/presentation/dockkit/engine/controller';
import { getPane } from '../src/presentation/dockkit/engine/tree';
import { WorkspaceAuthority } from '../src/workspaces/authority';
import { PREVIEW_POLICY } from '../src/client/preview-policy';
import { Server } from './fixture';

let server: Server, stop: (() => void) | undefined;
afterEach(() => { cleanup(); stop?.(); server?.client.disconnect(); localStorage.clear(); });
async function fixture() {
  server = new Server(); await server.attached('A');
  const authority = new WorkspaceAuthority(server.workspaceHost); await authority.observe();
  const previews = new WorkbenchPreviewRetention(server.client, authority, new PreviewContentCache()); stop = previews.start();
  const scope = authority.getCatalog()!, target = { session_id: 'A', active_node: 'node-A' };
  const retained = previews.acquire(scope, target);
  server.workspaceHost.workbench = async (_scope, { request }) => {
    if (request.kind === 'resolve') return { path: request.path };
    if (request.kind === 'files') return { entries: ['a.txt', 'b.txt', 'c.txt'].map(name => ({ name, directory: false, link: false })) };
    if (request.kind === 'preview') {
      const data = new TextEncoder().encode(`Contents of ${request.path}`);
      return { preview: { data, bytes: data.byteLength, version: request.path, offset: request.read.kind === 'text' ? request.read.offset : 0, lines: 1, eof: true } };
    }
    return {};
  };
  const ref = createRef<WorkbenchHandle>();
  const ui = render(<Workbench fileRef={ref} host={server.workspaceHost} previews={previews} scope={scope} target={target} visible floatingVisible fullscreen={false} toggleFullscreen={() => {}} closePanel={() => {}}/>);
  let dock!: DockController;
  const original = DockController.prototype.openContent;
  vi.spyOn(DockController.prototype, 'openContent').mockImplementation(function (this: DockController, input) { dock = this; return original.call(this, input); });
  return { ui, ref, retained, get dock() { return dock; } };
}
it('the floating file navigator replaces its own file, disposes its old reader and leaves an existing destination copy intact', async () => {
  const f = await fixture();
  const { ui, ref, retained } = f;
  await act(async () => { await ref.current!.openFile('b.txt'); await ref.current!.openFile('a.txt'); });
  const { dock } = f;
  await ui.findByText('Contents of a.txt');
  const first = Object.values(dock.getSnapshot().state.tabs).find(tab => tab.contentId === 'file:a.txt')!;
  const existing = Object.values(dock.getSnapshot().state.tabs).find(tab => tab.contentId === 'file:b.txt')!;
  const oldReader = retained.views.get(first.id)!.preview!, disposed = vi.spyOn(oldReader, 'dispose');
  let floating!: ReturnType<DockController['floatTab']>;
  act(() => { floating = dock.floatTab(first.id, { x: 100, y: 150, width: 640, height: 500 }); });
  const pane = () => within(ui.container.querySelector<HTMLElement>(`[data-dockkit-float="${floating}"]`)!);
  fireEvent.click(pane().getByRole('button', { name: 'Show workspace files' }));
  fireEvent.click(await pane().findByRole('button', { name: 'c.txt' }));
  await pane().findByText('Contents of c.txt');
  expect(getPane(dock.getSnapshot().state, floating).rect).toEqual({ x: 100, y: 150, width: 640, height: 500 });
  await waitFor(() => expect(disposed).toHaveBeenCalledOnce());
  expect(retained.views.has(first.id)).toBe(false);
  expect(oldReader.owner.getSnapshot().text).toBeUndefined();
  fireEvent.click(pane().getByRole('button', { name: 'Show workspace files' }));
  fireEvent.click(await pane().findByRole('button', { name: 'b.txt' }));
  await pane().findByText('Contents of b.txt');
  const state = dock.getSnapshot().state;
  expect(state.tabs[existing.id]).toBe(existing);
  expect(getPane(state, floating).tabs).not.toContain(existing.id);
  expect(getPane(state, floating).rect).toEqual({ x: 100, y: 150, width: 640, height: 500 });
  expect(state.activePaneId).toBe(floating);
});

it('enforces the file-tab budget for a floated Files page while allowing file replacement at capacity', async () => {
  const f = await fixture();
  const { ui, ref } = f;
  await act(async () => {
    await ref.current!.openFile('a.txt');
    for (let index = 1; index < PREVIEW_POLICY.tabsPerSession; index++) await ref.current!.openFile(`other-${index}.txt`);
  });
  const { dock } = f;
  const first = Object.values(dock.getSnapshot().state.tabs).find(tab => tab.contentId === 'file:a.txt')!;
  act(() => { dock.addTab(dock.activeDockPaneId()); });
  fireEvent.click(ui.getByRole('button', { name: /^Workspace files/ }));
  const files = Object.values(dock.getSnapshot().state.tabs).find(tab => tab.kind === 'files')!;
  let filesPane!: ReturnType<DockController['floatTab']>;
  act(() => { filesPane = dock.floatTab(files.id); });
  const filesBody = within(ui.container.querySelector<HTMLElement>(`[data-dockkit-float="${filesPane}"]`)!);
  fireEvent.click(await filesBody.findByRole('button', { name: 'a.txt' }));
  expect(dock.getSnapshot().state.tabs[files.id]).toBe(files);
  expect(Object.values(dock.getSnapshot().state.tabs).filter(tab => tab.kind === 'file')).toHaveLength(PREVIEW_POLICY.tabsPerSession);
  expect(ui.getByRole('alert')).toBeTruthy();
  let filePane!: ReturnType<DockController['floatTab']>;
  act(() => { filePane = dock.floatTab(first.id); });
  const fileBody = () => within(ui.container.querySelector<HTMLElement>(`[data-dockkit-float="${filePane}"]`)!);
  fireEvent.click(fileBody().getByRole('button', { name: 'Show workspace files' }));
  fireEvent.click(await fileBody().findByRole('button', { name: 'c.txt' }));
  await fileBody().findByText('Contents of c.txt');
  const state = dock.getSnapshot().state;
  expect(state.tabs[first.id]).toBeUndefined();
  expect(state.tabs[files.id]).toBe(files);
  expect(Object.values(state.tabs).filter(tab => tab.kind === 'file')).toHaveLength(PREVIEW_POLICY.tabsPerSession);
});
