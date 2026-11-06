import { act, cleanup, render, waitFor } from '@testing-library/react';
import { afterEach, expect, it } from 'vitest';
import { ArtifactPreview } from '../src/app/components/ArtifactPreview';
import { FilePreviewCoordinator, FilePreviewLease, type PreviewSource } from '../src/client/session-files';
import { PreviewContentOwner } from '../src/client/preview-content';
import { WorkspaceAuthority } from '../src/workspaces/authority';
import { Server } from './fixture';
afterEach(cleanup);
it('keeps highlighted code DOM and keyboard focus during a no-op file-watch validation', async () => {
  const server = new Server(); await server.attached('A');
  const authority = new WorkspaceAuthority(server.workspaceHost); await authority.observe();
  const coordinator = new FilePreviewCoordinator(server.client, 'A', server.workspaceHost, authority);
  const source: PreviewSource = { kind: 'session_file', conversationId: server.target('A').conversation_id, locator: { type: 'by_message_id', message_id: 'file-result' }, index: 0, file: { scope: { conversation_id: server.target('A').conversation_id, device: '1', inode: '2' }, path: 'example.ts', name: 'example.ts', mime_type: 'text/plain' } };
  let hint: (() => void) | undefined, finishProbe: ((version: string) => void) | undefined, finishWatch!: () => void, pages = 0;
  const done = new Promise<void>(resolve => { finishWatch = resolve; });
  const owner = new PreviewContentOwner({ current: () => coordinator.sourceCurrent(source), immutable: false,
    page: async offset => { pages++; return { text: 'const answer = 1;', version: 'v1', offset, lines: 1, eof: true }; },
    bytes: async () => { throw Error('Only text pages are expected'); },
    document: async () => { throw Error('Only text pages are expected'); },
    probe: () => new Promise<string>(resolve => { finishProbe = resolve; }),
    observe: async (_signal, changed) => { hint = changed; return { version: 'v1', done, close: async () => finishWatch() }; },
  });
  owner.activate();
  const lease = new FilePreviewLease(coordinator, 1, source, owner);
  try {
    const ui = render(<ArtifactPreview artifact={{ source, name: 'example.ts', image: false }} resources={lease} viewState={{ viewer: 'code' }} onViewStateChange={() => {}} onDownload={() => {}}/>);
    await waitFor(() => expect(ui.container.querySelector('pre.shiki')).not.toBeNull());
    await waitFor(() => expect(hint).toBeDefined());
    const code = ui.container.querySelector<HTMLElement>('pre.shiki')!; code.tabIndex = 0; code.focus();
    expect(document.activeElement).toBe(code);
    await act(async () => hint!()); await waitFor(() => expect(finishProbe).toBeDefined());
    expect(owner.getSnapshot().loading).toBe(true); expect(pages).toBe(1);
    expect(ui.container.querySelector('pre.shiki')).toBe(code); expect(document.activeElement).toBe(code);
    await act(async () => finishProbe!('v1'));
    await waitFor(() => expect(owner.getSnapshot().loading).toBe(false));
    expect(ui.container.querySelector('pre.shiki')).toBe(code); expect(document.activeElement).toBe(code); expect(pages).toBe(1);
  } finally { owner.dispose(); coordinator.dispose(); server.client.dispose(); finishWatch(); }
});
