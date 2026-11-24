import { expect, it, vi } from 'vitest';
import { PreviewContentOwner } from '../src/client/preview-content';
import { readGeometrySource } from '../src/client/geometry-source';
import { WorkspaceHostError } from '../src/workspaces/host';

it('retires an incomplete geometry copy and fences its late physical read settlement', async () => {
  let finish!: (value: {data: Uint8Array<ArrayBuffer>; version: string}) => void;
  const owner = new PreviewContentOwner({current: () => true, immutable: true,
    page: async () => { throw Error('page'); }, document: async () => { throw Error('document'); }, probe: async () => 'one',
    bytes: () => new Promise(resolve => { finish = resolve; })});
  try {
    owner.activate(); owner.ensure('geometry');
    expect(owner.getSnapshot().data).toBeUndefined(); expect(owner.retainedBytes).toBe(0);
    owner.deactivate(); finish({data: new Uint8Array([1]), version: 'one'});
    await vi.waitFor(() => expect(owner.getSnapshot().loading).toBe(false));
    expect(owner.getSnapshot().data).toBeUndefined(); expect(owner.retainedBytes).toBe(0);
  } finally { owner.dispose(); }
});

it('refreshes a replaced streaming source and publishes the next validated copy', async () => {
  const replaced = new WorkspaceHostError('File replaced at validation', 'source_changed');
  let opened = 0;
  const owner = new PreviewContentOwner({ current: () => true, immutable: true,
    page: async () => { throw Error('page'); }, document: async () => { throw Error('document'); }, probe: async () => 'two',
    bytes: async () => { throw Error('bytes'); },
    geometry: (signal, sink) => readGeometrySource(async () => {
      const first = ++opened === 1, version = first ? 'one' : 'two';
      return { bytes: 1, version, read: async () => ({ data: new Uint8Array([opened]), offset: 0, bytes: 1, eof: true, version }),
        close: async () => { if (first) throw replaced; } };
    }, signal, sink),
  });
  try {
    const sink = { start: vi.fn(), chunk: vi.fn() }, signal = new AbortController().signal;
    owner.activate(); owner.ensure('geometry');
    const first = owner.getSnapshot().source!;
    await expect(first.consume(sink, signal)).rejects.toBe(replaced);
    await vi.waitFor(() => expect(owner.getSnapshot().source).toBeDefined());
    const second = owner.getSnapshot().source!;
    expect(second).not.toBe(first);
    await expect(second.consume(sink, signal)).resolves.toEqual({ version: 'two' });
    expect(owner.getSnapshot()).toMatchObject({ data: new Uint8Array([2]), version: 'two', loading: false });
    expect(owner.getSnapshot().error).toBeUndefined();
  } finally { owner.dispose(); }
});

it.each([false, true])('leaves decoder cancellation to the viewer and preserves unknown settlement (%s)', async uncertain => {
  const settlement = new WorkspaceHostError('Settlement unconfirmed', 'file_settlement_unknown');
  let started!: () => void;
  const reading = new Promise<void>(resolve => { started = resolve; });
  const owner = new PreviewContentOwner({ current: () => true, immutable: true,
    page: async () => { throw Error('page'); }, document: async () => { throw Error('document'); }, probe: async () => 'one',
    bytes: async () => { throw Error('bytes'); },
    geometry: signal => new Promise((_, reject) => {
      signal.addEventListener('abort', () => reject(uncertain ? settlement : signal.reason), { once: true });
      started();
    }),
  });
  try {
    owner.activate(); owner.ensure('geometry');
    const decoder = new AbortController();
    const pending = owner.getSnapshot().source!.consume({ start() {}, chunk() {} }, decoder.signal);
    await reading;
    decoder.abort();
    await expect(pending).rejects.toBe(uncertain ? settlement : decoder.signal.reason);
    expect(owner.getSnapshot().loading).toBe(false);
    expect(owner.getSnapshot().error).toBe(uncertain ? settlement : undefined);
  } finally { owner.dispose(); }
});
