import { expect, it, vi } from 'vitest';
import { PreviewContentOwner } from '../src/client/preview-content';

it('releases a large admitted model raw buffer without changing its active copy token and rereads after hiding', async () => {
  const bytes = vi.fn(async () => ({ data: new Uint8Array(64 * 1024 * 1024 + 1), version: 'one' }));
  const owner = new PreviewContentOwner({ current: () => true, immutable: false, bytes,
    page: async () => { throw new Error('unexpected'); }, document: async () => { throw new Error('unexpected'); }, probe: async () => 'one' });
  try {
    owner.activate(); owner.ensure('geometry'); await vi.waitFor(() => expect(owner.getSnapshot().data).toBeDefined());
    const before = owner.getSnapshot(), data = before.data!;
    data[0] = 42; const transferred = owner.takeGeometryBytes(data)!;
    expect(transferred.byteLength).toBe(64 * 1024 * 1024 + 1); expect(transferred[0]).toBe(42);
    expect(data.byteLength).toBe(0); expect(owner.retainedBytes).toBe(0); expect(owner.takeGeometryBytes(data)).toBeUndefined();
    expect(owner.getSnapshot()).toBe(before); expect(owner.getSnapshot().version).toBe('one');
    owner.deactivate(); expect(owner.getSnapshot().data).toBeUndefined(); owner.activate(); owner.ensure('geometry');
    await vi.waitFor(() => expect(bytes).toHaveBeenCalledTimes(2)); await vi.waitFor(() => expect(owner.getSnapshot().data?.byteLength).toBe(64 * 1024 * 1024 + 1));
    expect(owner.getSnapshot().revision).toBeGreaterThan(before.revision); owner.releaseGeometryBytes(owner.getSnapshot().data!);
  } finally { owner.dispose(); }
});
it('does not release a small, foreign or non-geometry byte buffer', async () => {
  const data = new Uint8Array([1,2,3]), owner = new PreviewContentOwner({ current: () => true, immutable: true,
    bytes: async () => ({ data, version:'one' }), page: async () => { throw new Error('unexpected'); }, document: async () => { throw new Error('unexpected'); }, probe: async () => 'one' });
  try {
    owner.activate(); owner.ensure('bytes'); await vi.waitFor(() => expect(owner.getSnapshot().data).toBe(data));
    expect(owner.releaseGeometryBytes(data)).toBe(false); expect(owner.releaseGeometryBytes(new Uint8Array(64 * 1024 * 1024 + 1))).toBe(false); expect([...data]).toEqual([1,2,3]);
    owner.ensure('geometry'); await vi.waitFor(() => expect(owner.getSnapshot().data).toBe(data)); expect(owner.releaseGeometryBytes(data)).toBe(false);
  } finally { owner.dispose(); }
});
