import { useSyncExternalStore } from 'react';
import { act, cleanup, fireEvent, render } from '@testing-library/react';
import { afterEach, beforeEach, expect, it, vi } from 'vitest';
import { DeliveryCards } from '../src/app/components/Artifact';
import { ArtifactPreview, PreviewContext, type PreviewArtifact } from '../src/app/components/ArtifactPreview';
import { PreviewWorkspaceOwner } from '../src/app/preview-workspace';
import { WorkspaceAuthority } from '../src/workspaces/authority';
import type { DeliveryBytes, DeliveryRead, ProductHostWorkspaces } from '../src/workspaces/host';
import type { PresentedDelivery } from '../src/bindings/present';
import { Server } from './fixture';

const delivery: PresentedDelivery = { key: 'report', locator: { type: 'by_message_id', message_id: 'canonical-tool' }, index: 0,
  file: { scope: { conversation_id: 'original', device: '1', inode: '2' }, path: 'report.txt', name: 'report.txt', mime_type: 'text/plain' } };
function deferred<T>() { let resolve!: (value: T) => void; const promise = new Promise<T>(done => { resolve = done; }); return { promise, resolve }; }
let server: Server, owner: PreviewWorkspaceOwner, stop: () => void;
beforeEach(() => {
  server = new Server();
  vi.stubGlobal('URL', class extends URL { static createObjectURL = vi.fn(() => 'blob:download'); static revokeObjectURL = vi.fn(); });
});
afterEach(() => { cleanup(); stop?.(); server.client.disconnect(); vi.restoreAllMocks(); vi.unstubAllGlobals(); });
function WorkspaceBody() {
  const snapshot = useSyncExternalStore(owner.subscribe, owner.getSnapshot), workspace = snapshot.workspace;
  const tab = workspace?.tabs.find(tab => tab.id === workspace.panes.find(pane => pane.id === workspace.activePane)?.selected);
  const lease = tab && snapshot.leases.get(tab.id);
  return tab && lease ? <ArtifactPreview artifact={tab.artifact} resources={lease} viewState={tab.view}
    onViewStateChange={patch => owner.updateView(tab.id, patch)} onDownload={() => {}}/> : null;
}
async function fixture() {
  await server.attached('A', 'B');
  let hostId = 'host-1';
  const calls: { read: DeliveryRead; signal?: AbortSignal; response: ReturnType<typeof deferred<DeliveryBytes>> }[] = [];
  const host: ProductHostWorkspaces = { ...server.workspaceHost,
    listWorkspaces: async () => ({ ...await server.workspaceHost.listWorkspaces(), authorityId: hostId }),
    readDelivery: async (_scope, read, signal) => { const response = deferred<DeliveryBytes>(); calls.push({ read, signal, response }); return response.promise; },
  };
  const authority = new WorkspaceAuthority(host); await authority.observe();
  owner = new PreviewWorkspaceOwner(server.client, host, authority); stop = owner.start(); owner.selectSession('A');
  const openPreview = vi.fn((artifact: PreviewArtifact) => { owner.openPreview(artifact); });
  const download = vi.fn((artifact: PreviewArtifact) => owner.download(artifact));
  const view = (owned: () => boolean) => <PreviewContext.Provider value={{ openPreview, download }}>
    <DeliveryCards conversationId="conversation-A" deliveries={[delivery]} owned={owned}/><WorkspaceBody/>
  </PreviewContext.Provider>;
  const reply = (at: number, text: string) => {
    const data = new TextEncoder().encode(text), read = calls[at].read.read;
    calls[at].response.resolve({ file: delivery.file, data, version: 'version-1', bytes: data.length,
      offset: read?.kind === 'text' ? read.offset : 0, eof: true, ...(read?.kind === 'text' ? { lines: 1 } : {}) });
  };
  return { calls, openPreview, download, view, reply, replaceHost: async () => { hostId = 'host-2'; await authority.observe(); } };
}

it.each(['hidden', 'returned'] as const)('root delivery preview survives A/B/A while its obsolete read settles %s', async timing => {
  const f = await fixture(); let viewOwned = true;
  const ui = render(f.view(() => viewOwned));
  fireEvent.click(ui.getByRole('button', { name: 'Preview report.txt in sidebar' }));
  expect(f.calls).toHaveLength(1);
  const tab = owner.getSnapshot().workspace!.tabs[0], oldLease = owner.getSnapshot().leases.get(tab.id)!;
  act(() => owner.updateView(tab.id, { wrap: false, pdfScrollTop: 7 }));
  await act(async () => { viewOwned = false; owner.selectSession('B'); });
  expect(oldLease.signal.aborted).toBe(true); expect(f.calls[0].signal?.aborted).toBe(true);
  if (timing === 'hidden') await act(async () => f.reply(0, 'obsolete preview body'));
  await act(async () => owner.selectSession('A'));
  expect(owner.getSnapshot().workspace?.tabs[0]).toMatchObject({ id: tab.id, view: { wrap: false, pdfScrollTop: 7 } });
  expect(owner.getSnapshot().leases.get(tab.id)).not.toBe(oldLease);
  // A retired message view cannot admit another preview or download, even if
  // its callback is retained. Already admitted tabs have a separate lifetime.
  fireEvent.click(ui.getByRole('button', { name: 'Preview report.txt in sidebar' }));
  fireEvent.click(ui.getByRole('button', { name: 'Download report.txt' }));
  expect(f.openPreview).toHaveBeenCalledTimes(1); expect(f.download).not.toHaveBeenCalled();
  if (timing === 'returned') {
    // This tab's content owner keeps the old physical read as its barrier.
    // Reactivating the view cannot admit a replacement before it settles.
    expect(f.calls).toHaveLength(1);
    expect(ui.queryByText('obsolete preview body')).toBeNull();
    await act(async () => f.reply(0, 'obsolete preview body'));
  }
  expect(f.calls).toHaveLength(2);
  expect(ui.queryByText('obsolete preview body')).toBeNull();
  await act(async () => f.reply(1, 'fresh authorized preview body'));
  expect(ui.getByText('fresh authorized preview body')).toBeTruthy();
  expect(ui.queryByText('obsolete preview body')).toBeNull();
});

it.each(['attachment', 'Host'] as const)('root delivery reads and downloads still retire on %s replacement', async replacement => {
  const f = await fixture(), observation = server.client.getSnapshot().views.A!.attachmentObservation;
  const ui = render(f.view(() => server.client.isAttachmentObservationCurrent('A', observation)));
  const click = vi.spyOn(HTMLAnchorElement.prototype, 'click').mockImplementation(() => {});
  fireEvent.click(ui.getByRole('button', { name: 'Preview report.txt in sidebar' }));
  fireEvent.click(ui.getByRole('button', { name: 'Download report.txt' }));
  expect(f.calls).toHaveLength(2);
  const oldLease = [...owner.getSnapshot().leases.values()][0];
  await act(async () => {
    if (replacement === 'attachment') { await server.client.release('A'); await server.client.attach('A'); }
    else await f.replaceHost();
  });
  expect(oldLease.signal.aborted).toBe(true);
  expect(f.calls.every(call => call.signal?.aborted)).toBe(true);
  expect(owner.getSnapshot().workspace).toBeUndefined();
  await act(async () => { f.reply(0, 'retired preview body'); f.reply(1, 'retired download'); await f.download.mock.results[0].value; });
  expect(ui.queryByText('retired preview body')).toBeNull(); expect(click).not.toHaveBeenCalled();
  expect(URL.createObjectURL).not.toHaveBeenCalled();
  expect(owner.getSnapshot().downloadError).toBeUndefined();
  if (replacement === 'attachment') {
    fireEvent.click(ui.getByRole('button', { name: 'Preview report.txt in sidebar' }));
    fireEvent.click(ui.getByRole('button', { name: 'Download report.txt' }));
    expect(f.openPreview).toHaveBeenCalledTimes(1); expect(f.download).toHaveBeenCalledTimes(1);
  }
  const freshObservation = server.client.getSnapshot().views.A!.attachmentObservation;
  ui.rerender(f.view(() => server.client.isAttachmentObservationCurrent('A', freshObservation)));
  fireEvent.click(ui.getByRole('button', { name: 'Preview report.txt in sidebar' }));
  expect(f.calls).toHaveLength(3);
  await act(async () => f.reply(2, 'replacement preview body'));
  expect(ui.getByText('replacement preview body')).toBeTruthy();
});
