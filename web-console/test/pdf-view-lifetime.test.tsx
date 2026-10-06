import { act, cleanup, render } from '@testing-library/react';
import { afterEach, expect, it, vi } from 'vitest';
const boundary = vi.hoisted(() => ({ failures: [] as ((code: string) => void)[], dispose: vi.fn() }));
vi.mock('../src/client/pdf-document', () => ({ PdfDocumentOwner: class {
  document = new Promise<never>(() => {});
  dispose = boundary.dispose;
  constructor(_bytes: Uint8Array, fail: (code: string) => void) { boundary.failures.push(fail); }
} }));
import PdfPreview from '../src/app/components/documents/PdfPreview';
afterEach(() => { cleanup(); vi.useRealTimers(); boundary.failures.splice(0); boundary.dispose.mockReset(); });
it.each(['worker_failure', 'parser_timeout'] as const)('%s clears the load deadline even when worker destruction has no acknowledgement', async code => {
  vi.useFakeTimers();
  const view = render(<PdfPreview bytes={new Uint8Array([1])} signal={new AbortController().signal} viewState={{}} onViewStateChange={() => {}} retry={() => {}} />);
  expect(vi.getTimerCount()).toBe(1);
  await act(async () => {
    if (code === 'worker_failure') boundary.failures[0](code);
    else vi.advanceTimersByTime(15000);
  });
  expect(view.getByRole('alert')).toBeTruthy(); expect(view.queryByRole('status')).toBeNull();
  expect(boundary.dispose).toHaveBeenCalledOnce(); expect(vi.getTimerCount()).toBe(0);
});
