import { expect, it, vi } from 'vitest';
import { FirstSubmissions, type FirstSubmitPort, type FirstDraft, type CreatedSession } from '../src/app/new-conversation/first-submit';
import { OutcomeUncertain, RpcFailure } from '../src/client/app-server';
import type { UploadReceipt } from '../../protocol/app-server/v27';
function gate<T>() { let resolve!: (value: T) => void, reject!: (reason: unknown) => void; const promise = new Promise<T>((a, b) => { resolve = a; reject = b; }); return { promise, resolve, reject }; }
const session: CreatedSession = { id: 'native-session', node: 'native-node', conversation: 'native-conversation' };
const draft: FirstDraft = { workspaceId: 'registered', text: 'Task', files: [], model: { model: 'explicit' } };
const receipt = { batch_id: 'batch', token: 'token', session_id: session.id } as UploadReceipt;
function fixture(overrides: Partial<FirstSubmitPort> = {}) {
  const owner = new FirstSubmissions();
  const port: FirstSubmitPort = { current: () => true, create: vi.fn(async () => session), handoff: vi.fn(), attach: vi.fn(async () => {}), upload: vi.fn(async () => receipt), send: vi.fn(async () => {}), ...overrides };
  return { owner, port, start: (input = draft) => owner.submit('draft', input, port) };
}
it('installs the Session owner before navigation, attach, upload and admission; observers never dispatch', async () => {
  const attach = gate<void>(), entered = gate<void>();
  const { owner, port, start } = fixture({ attach: vi.fn(() => { entered.resolve(); return attach.promise; }) });
  port.handoff = vi.fn(() => { expect(owner.session(session.id)?.draft.text).toBe('Task'); expect(port.attach).not.toHaveBeenCalled(); });
  const work = start(); await entered.promise;
  expect(owner.session(session.id)?.phase).toBe('attaching');
  expect(port.send).not.toHaveBeenCalled(); expect(port.upload).not.toHaveBeenCalled();
  for (let i = 0; i < 5; i++) { const remove = owner.subscribe(() => {}); owner.session(session.id); remove(); await start(); }
  expect(port.create).toHaveBeenCalledTimes(1); expect(port.handoff).toHaveBeenCalledTimes(1);
  attach.resolve(); expect(await work).toBe(true); expect(port.send).toHaveBeenCalledTimes(1);
});
it.each(['attach', 'upload', 'send'] as const)('retains the Session and original intent after %s failure without replay', async phase => {
  const { owner, port, start } = fixture({ [phase]: vi.fn(async () => { throw new Error('rejected'); }) });
  const input = { ...draft, files: [new File(['a'], 'a')] };
  expect(await start(input)).toBe(false); const state = owner.session(session.id)!;
  expect(state.phase).toBe('failed'); expect(state.draft).toEqual(input);
  expect(await start(input)).toBe(false); expect(port.create).toHaveBeenCalledTimes(1); expect(port[phase]).toHaveBeenCalledTimes(1);
  owner.discard(state); expect(owner.session(session.id)?.draft.files).toEqual([]);
});
it.each(['create', 'upload', 'send'] as const)('lost %s response stays uncertain through observation; no retry edge', async phase => {
  const { owner, port, start } = fixture({ [phase]: vi.fn(async () => { throw new OutcomeUncertain(); }) });
  await start({ ...draft, files: [new File(['a'], 'a')] });
  expect(owner.draft('draft')?.phase).toBe('uncertain');
  const unsubscribe = owner.subscribe(() => {}); unsubscribe(); await start();
  expect(port[phase]).toHaveBeenCalledTimes(1);
});
it('known creation rejection is editable and only a fresh explicit gesture retries', async () => {
  const { owner, port, start } = fixture({ create: vi.fn().mockRejectedValueOnce(new RpcFailure({ code: -1, message: 'invalid' })).mockResolvedValueOnce(session) });
  await start(); expect(owner.draft('draft')?.phase).toBe('rejected'); expect(port.attach).not.toHaveBeenCalled();
  await start(); expect(port.create).toHaveBeenCalledTimes(2); expect(port.send).toHaveBeenCalledTimes(1);
});
it('captures create ACK before a subsequent authority error; never hijacks a newer route', async () => {
  const { owner, port, start } = fixture({ create: async (_, acknowledged) => { acknowledged(session); throw Error('transport retired after ACK'); } });
  await start(); expect(owner.session(session.id)?.session).toEqual(session);
  expect(owner.session(session.id)?.phase).toBe('failed'); expect(port.handoff).toHaveBeenCalledTimes(1);
});
it('acknowledged creation carrying a durability diagnostic navigates but stops attachment', async () => {
  const { owner, port, start } = fixture({ create: async () => ({ ...session, diagnostic: 'durability' }) });
  await start(); expect(port.handoff).toHaveBeenCalledTimes(1); expect(port.attach).not.toHaveBeenCalled();
  expect(owner.session(session.id)?.session?.diagnostic).toBe('durability');
});
it('ordered files and partial receipts survive fencing after a confirmed upload', async () => {
  const files = [new File(['a'], 'a'), new File(['b'], 'b')]; let live = true;
  const { owner, port, start } = fixture({ current: () => live, upload: vi.fn(async (_, file, acknowledged) => { expect(file).toBe(files[0]); acknowledged(receipt); live = false; throw Error('retired after ACK'); }) });
  await start({ ...draft, files });
  expect(owner.session(session.id)?.receipts).toEqual([receipt]); expect(owner.session(session.id)?.draft.files).toEqual(files);
  expect(port.upload).toHaveBeenCalledTimes(1); expect(port.send).not.toHaveBeenCalled();
});
it('admission ACK consumes intent even when the transport retires before promise settlement', async () => {
  const { owner, start } = fixture({ send: async (_, __, ___, acknowledged) => { acknowledged(); throw Error('retired after ACK'); } });
  expect(await start({ ...draft, files: [new File(['a'], 'a')] })).toBe(true);
  expect(owner.session(session.id)?.phase).toBe('admitted'); expect(owner.session(session.id)?.draft.files).toEqual([]); expect(owner.session(session.id)?.draft.text).toBe('');
});
it('final disposal fences outstanding work, releases retained state and subscriptions', async () => {
  const creation = gate<CreatedSession>(); const { owner, port, start } = fixture({ create: () => creation.promise });
  const observe = vi.fn(); owner.subscribe(observe); const work = start(); owner.dispose(); creation.resolve(session); await work;
  expect(port.handoff).not.toHaveBeenCalled(); expect(owner.draft('draft')).toBeUndefined(); expect(observe).toHaveBeenCalledTimes(1);
});
it('a second upload rejection preserves the first receipt, all Files, and their order', async () => {
  const files = [new File(['a'], 'a'), new File(['b'], 'b')];
  const { owner, port, start } = fixture({ upload: vi.fn().mockResolvedValueOnce(receipt).mockRejectedValueOnce(new Error('second rejected')) });
  await start({ ...draft, files });
  expect(owner.session(session.id)?.receipts).toEqual([receipt]);
  expect(owner.session(session.id)?.draft.files).toEqual(files);
  expect(port.upload).toHaveBeenCalledTimes(2); expect(port.send).not.toHaveBeenCalled();
  expect(owner.session(session.id)?.uploadIndex).toBe(1);
});
it('a replaced authority cannot expose old Session or draft state under reused identities', async () => {
  const attach = gate<void>(), entered = gate<void>(); let current = true;
  const { owner, port, start } = fixture({ current: () => current, attach: async () => { entered.resolve(); await attach.promise; } });
  const work = start(); await entered.promise; const known = owner.session(session.id)!;
  owner.retireAuthority(); current = false; attach.resolve(); await work;
  expect(owner.session(session.id)).toBeUndefined(); expect(owner.draft('draft')).toBeUndefined();
  expect(known.session).toEqual(session); expect(port.send).not.toHaveBeenCalled();
});
it('retired input remains inspectable until explicit discard and stale observers cannot discard a newer operation', async () => {
  const { owner, start } = fixture({ create: async () => { throw Error('rejected'); } });
  await start(); const old = owner.draft('draft')!;
  await start({ ...draft, text: 'new input' });
  owner.discard(old); expect(owner.draft('draft')?.draft.text).toBe('new input');
  owner.retireAuthority(); const detached = owner.detachedSnapshot()[0];
  expect(detached.draft.text).toBe('new input'); owner.discard(detached);
  expect(owner.detachedSnapshot()).toEqual([]);
});
