import { act, cleanup, render } from '@testing-library/react';
import { afterEach, expect, it, vi } from 'vitest';
import { DocumentPreview } from '../src/app/components/documents/DocumentPreview';
import type { FilePreviewResources } from '../src/client/session-files';
import type { DerivedDocument } from '../shared/documents.ts';
function deferred<T>() { let resolve!: (value: T) => void, reject!: (cause: unknown) => void; const promise = new Promise<T>((a, b) => { resolve = a; reject = b; }); return { promise, resolve, reject }; }
afterEach(cleanup);
it.each(['result', 'error'] as const)('A followed by B discards late A %s and loading completion', async terminal => {
  const a = deferred<DerivedDocument>(), b = deferred<DerivedDocument>(), calls: AbortSignal[] = [];
  const resources = { derive: vi.fn((_source, _kind, _bytes, signal: AbortSignal) => { calls.push(signal); return calls.length === 1 ? a.promise : b.promise; }) } as unknown as FilePreviewResources;
  const bytes = new Uint8Array([1]);
  const view = render(<DocumentPreview kind="xlsx" bytes={bytes} source={{ kind: 'artifact', id: 'A' }} resources={resources} retry={() => {}} />);
  view.rerender(<DocumentPreview kind="xlsx" bytes={bytes} source={{ kind: 'artifact', id: 'B' }} resources={resources} retry={() => {}} />);
  expect(calls[0].aborted).toBe(true);
  await act(async () => b.resolve({ kind: 'xlsx', sheets: [{ name: 'B', cells: [{ address: 'A1', type: 'str', value: 'B value' }], truncated: false }] }));
  expect(view.getByText('B value')).toBeTruthy();
  await act(async () => { if (terminal === 'error') a.reject(new Error('parser_failure')); else a.resolve({ kind: 'xlsx', sheets: [{ name: 'A', cells: [], truncated: false }] }); });
  expect(view.getByText('B value')).toBeTruthy(); expect(view.queryByRole('alert')).toBeNull(); expect(view.queryByRole('status')).toBeNull();
  view.unmount(); expect(calls[1].aborted).toBe(true);
});
