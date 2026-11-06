import { createRef } from 'react';
import { act, cleanup, render } from '@testing-library/react';
import { afterEach, expect, it, vi } from 'vitest';
import { Workbench, type WorkbenchHandle } from '../src/app/components/Workbench';
import type { ChangesReviewRequest } from '../src/app/changes-review';
import { WorkbenchPreviewRetention } from '../src/app/workbench-previews';
import { PreviewContentCache } from '../src/client/preview-content';
import { DockController } from '../src/presentation/dockkit/engine/controller';
import { WorkspaceAuthority } from '../src/workspaces/authority';
import { Server } from './fixture';

let server: Server | undefined, stop: (() => void) | undefined;
afterEach(() => { cleanup(); stop?.(); server?.client.disconnect(); localStorage.clear(); });

it('retains review requests through duplicate and floating tabs, then releases each attempt after its last tab closes', async () => {
  server = new Server(); await server.attached('A');
  const authority = new WorkspaceAuthority(server.workspaceHost); await authority.observe();
  const previews = new WorkbenchPreviewRetention(server.client, authority, new PreviewContentCache()); stop = previews.start();
  const ref = createRef<WorkbenchHandle>();
  const request = (attemptId: string): ChangesReviewRequest => ({
    client: server!.client, target: server!.client.target('A')!, attemptId, index: 0, owned: () => true,
    summary: { files: [{ path: '/workspace/A/file.txt', display: 'file.txt', added: 1, deleted: 1, binary: false, oversized: false }], total: 1, added: 1, deleted: 1 },
  });
  server.handlers.set('conversation/changeDiff', message => {
    if (message.method !== 'conversation/changeDiff') throw Error('Unexpected method');
    return { type: 'workspace_change_diff', comparison: { hunks: [{ old_start: 1, new_start: 1, old_text: 'before\n', new_text: `${message.params.attempt_id}\n` }], binary: false, oversized: false, truncated: false } };
  });
  const ui = render(<Workbench fileRef={ref} host={server.workspaceHost} previews={previews} scope={authority.getCatalog()!}
    target={{ session_id: 'A', active_node: 'node-A' }} visible floatingVisible fullscreen={false} toggleFullscreen={() => {}} closePanel={() => {}}/>);

  // Capture the actual request cache rather than relying on nondeterministic
  // garbage collection: an invisible closed tab alone does not prove release.
  const first = request('attempt-1');
  let reviews!: Map<string, ChangesReviewRequest>, dock!: DockController;
  const set = Map.prototype.set, openContent = DockController.prototype.openContent;
  const cacheSpy = vi.spyOn(Map.prototype, 'set').mockImplementation(function (this: Map<unknown, unknown>, key, value) {
    if (value === first) reviews = this as Map<string, ChangesReviewRequest>;
    return set.call(this, key, value);
  });
  const dockSpy = vi.spyOn(DockController.prototype, 'openContent').mockImplementation(function (this: DockController, input) {
    dock = this; return openContent.call(this, input);
  });
  act(() => ref.current!.openChanges(first));
  cacheSpy.mockRestore(); dockSpy.mockRestore();
  const retainedAttempts = () => [...reviews.values()].map(review => review.attemptId);
  await ui.findByText('attempt-1');
  expect(retainedAttempts()).toEqual(['attempt-1']);
  const firstTab = Object.values(dock.getSnapshot().state.tabs).find(tab => tab.kind === 'changes')!;
  act(() => { const duplicate = dock.duplicateTab(firstTab.id); dock.floatTab(duplicate); });
  const floatingTab = Object.values(dock.getSnapshot().state.tabs).find(tab => tab.kind === 'changes' && tab.id !== firstTab.id)!;
  act(() => dock.closeTab(firstTab.id));
  expect(retainedAttempts()).toEqual(['attempt-1']);
  await ui.findByText('attempt-1');

  const second = request('attempt-2');
  act(() => ref.current!.openChanges(second));
  await ui.findByText('attempt-2');
  expect(retainedAttempts()).toEqual(['attempt-1', 'attempt-2']);
  act(() => dock.closeTab(floatingTab.id));
  expect(retainedAttempts()).toEqual(['attempt-2']);
  expect(ui.queryByText('attempt-1')).toBeNull();
  const secondTab = Object.values(dock.getSnapshot().state.tabs).find(tab => tab.kind === 'changes')!;
  act(() => dock.closeTab(secondTab.id));
  expect(reviews.size).toBe(0);

  const reopened = request('attempt-1');
  act(() => ref.current!.openChanges(reopened));
  await ui.findByText('attempt-1');
  expect(reviews.size).toBe(1);
  expect(reviews.values().next().value).toBe(reopened);
  expect([...reviews.values()]).not.toContain(first);
});
