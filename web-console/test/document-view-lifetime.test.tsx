import { act, cleanup, fireEvent, render } from '@testing-library/react';
import { afterEach, expect, it, vi } from 'vitest';
import { ArtifactPreview } from '../src/app/components/ArtifactPreview';
import { PreviewContentOwner, type PreviewBytes } from '../src/client/preview-content';
import type { FilePreviewLease } from '../src/client/session-files';
vi.mock('../src/app/components/documents/PdfPreview', () => ({ default: ({ bytes }: { bytes: Uint8Array }) => <div>{new TextDecoder().decode(bytes)}</div> }));
function deferred<T>() { let resolve!: (value: T) => void, reject!: (cause: unknown) => void; const promise = new Promise<T>((a, b) => { resolve = a; reject = b; }); return { promise, resolve, reject }; }
afterEach(cleanup);
it.each(['result', 'error'] as const)('Office refresh retires A publication, waits for its actual %s, then publishes B', async terminal => {
  const a = deferred<PreviewBytes>(), b = deferred<PreviewBytes>(), calls: AbortSignal[] = [];
  const derive = vi.fn((_kind, signal: AbortSignal) => { calls.push(signal); return calls.length === 1 ? a.promise : b.promise; });
  const content = new PreviewContentOwner({ immutable: true, current: () => true, document: derive, page: async () => { throw Error('unexpected page'); }, bytes: async () => { throw Error('unexpected bytes'); }, probe: async () => 'version' });
  content.activate();
  const resources = { content, signal: new AbortController().signal, current: () => true } as unknown as FilePreviewLease;
  const view = render(<ArtifactPreview artifact={{source:{kind:'artifact',id:'file'},name:'report.docx',image:false}} resources={resources} viewState={{}} onViewStateChange={() => {}} onDownload={() => {}}/>);
  fireEvent.click(view.getByRole('button', {name:'Refresh'})); fireEvent.click(view.getByRole('button', {name:'Refresh'}));
  expect(calls[0].aborted).toBe(true); expect(derive).toHaveBeenCalledOnce();
  await act(async () => { if (terminal === 'error') a.reject(new Error('parser_failure')); else a.resolve({ data: new TextEncoder().encode('A value'), version:'old' }); });
  expect(derive).toHaveBeenCalledTimes(2); expect(view.queryByText('A value')).toBeNull(); expect(view.queryByRole('alert')).toBeNull(); expect(view.getByRole('status')).toBeTruthy();
  await act(async () => b.resolve({ data: new TextEncoder().encode('B value'), version:'current' }));
  expect(view.getByText('B value')).toBeTruthy(); expect(view.queryByRole('alert')).toBeNull(); expect(view.queryByRole('status')).toBeNull();
  view.unmount(); content.dispose();
});
