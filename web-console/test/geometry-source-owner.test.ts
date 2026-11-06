import { expect, it, vi } from 'vitest';
import { PreviewContentOwner } from '../src/client/preview-content';

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
