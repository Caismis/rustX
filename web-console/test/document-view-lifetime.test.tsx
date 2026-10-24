import { act, cleanup, render } from '@testing-library/react';
import { afterEach, expect, it, vi } from 'vitest';
import { DocumentPreview } from '../src/app/components/documents/DocumentPreview';
import type { FilePreviewLease } from '../src/client/session-files';
import type { DerivedDocument } from '../shared/documents.ts';
vi.mock('../src/app/components/documents/PdfPreview', () => ({ default: ({ bytes }: { bytes: Uint8Array }) => <div>{new TextDecoder().decode(bytes)}</div> }));
function deferred<T>() { let resolve!: (value: T) => void, reject!: (cause: unknown) => void; const promise = new Promise<T>((a, b) => { resolve = a; reject = b; }); return { promise, resolve, reject }; }
afterEach(cleanup);
it.each(['result', 'error'] as const)('A followed by B discards late A %s and loading completion', async terminal => {
  const a = deferred<DerivedDocument>(), b = deferred<DerivedDocument>(), calls: AbortSignal[] = [];
  const derive = vi.fn((_kind, signal: AbortSignal) => { calls.push(signal); return calls.length === 1 ? a.promise : b.promise; });
  const resources = { derive, current: () => true } as unknown as FilePreviewLease;
  const bytes = new Uint8Array([1]);
  const state = { viewState: {}, onViewStateChange: () => {} };
  const view = render(<DocumentPreview kind="docx" bytes={bytes} resources={resources} retry={() => {}} {...state} />);
  view.rerender(<DocumentPreview kind="docx" bytes={bytes} resources={{ ...resources } as FilePreviewLease} retry={() => {}} {...state} />);
  expect(calls[0].aborted).toBe(true);
  await act(async () => b.resolve({ kind: 'pdf', data: new TextEncoder().encode('B value') }));
  expect(view.getByText('B value')).toBeTruthy();
  await act(async () => { if (terminal === 'error') a.reject(new Error('parser_failure')); else a.resolve({ kind: 'pdf', data: new TextEncoder().encode('A value') }); });
  expect(view.getByText('B value')).toBeTruthy(); expect(view.queryByRole('alert')).toBeNull(); expect(view.queryByRole('status')).toBeNull();
  view.unmount(); expect(calls[1].aborted).toBe(true);
});
