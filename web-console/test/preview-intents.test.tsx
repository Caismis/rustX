import { act, cleanup, fireEvent, render } from '@testing-library/react';
import { afterEach, expect, it, vi } from 'vitest';
import { Artifact, ArtifactContext } from '../src/app/components/Artifact';
import { PreviewContext } from '../src/app/components/ArtifactPreview';
import { ArtifactResources } from '../src/client/artifacts';
import { AttachmentCard } from '../src/presentation/attachments/AttachmentCard';
import { Server } from './fixture';

afterEach(() => { cleanup(); vi.unstubAllGlobals(); });
it.each([false, true])('managed Artifact Download has an explicit original-source intent before and after inline loading (image=%s)', async image => {
  const server = new Server(); await server.attached('A');
  server.held.add('artifact/read');
  const create = vi.fn(() => 'blob:inline'), revoke = vi.fn();
  vi.stubGlobal('URL', Object.assign(URL, { createObjectURL: create, revokeObjectURL: revoke }));
  const inline = new ArtifactResources(server.client, 'A'), openPreview = vi.fn(), download = vi.fn();
  const artifact = { source: { kind: 'artifact' as const, id: 'native-artifact' }, name: '报告 original', image, mimeType: 'application/octet-stream' };
  const ui = render(<PreviewContext.Provider value={{ openPreview, download }}><ArtifactContext.Provider value={inline}>
    <Artifact id="native-artifact" name={artifact.name} image={image} mimeType={artifact.mimeType}/>
  </ArtifactContext.Provider></PreviewContext.Provider>);
  const before = server.requests.length;
  fireEvent.click(ui.getByRole('button', { name: 'Download 报告 original' }));
  expect(download).toHaveBeenLastCalledWith(artifact); expect(openPreview).not.toHaveBeenCalled();
  expect(server.requests).toHaveLength(before); expect(create).not.toHaveBeenCalled();
  fireEvent.click(ui.getByRole('button', { name: 'Load attachment' }));
  const request = await server.waitFor('artifact/read', 1);
  await act(async () => server.socket.success(request, { type: 'artifact_bytes', data: btoa('original bytes') }));
  fireEvent.click(ui.getByRole('button', { name: 'Download 报告 original' }));
  expect(download).toHaveBeenCalledTimes(2); expect(openPreview).not.toHaveBeenCalled(); expect(ui.queryByRole('link')).toBeNull();
  if (image) {
    fireEvent.click(ui.getByRole('button', { name: 'Open image 报告 original' }));
    expect(ui.getByRole('dialog').querySelector('a[download]')).toBeNull();
  }
  ui.unmount(); inline.dispose(); server.client.disconnect(); expect(revoke).toHaveBeenCalledExactlyOnceWith('blob:inline');
});

it('local user-attachment bytes keep the draft owner download URL without a native preview intent', () => {
  const ui = render(<AttachmentCard name="draft.txt" image={false} url="blob:draft"/>);
  expect(ui.getByRole('link', { name: 'Download' }).getAttribute('href')).toBe('blob:draft');
  expect(ui.getByRole('link', { name: 'Download' }).getAttribute('download')).toBe('draft.txt');
});
