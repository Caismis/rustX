import { expect, it, vi } from 'vitest';
import { FirstSubmissions, type FirstSubmitPort, type FirstDraft, type CreatedSession } from '../src/app/new-conversation/first-submit';
import { UploadFailure } from '../src/client/uploads';
import { OutcomeUncertain, RpcFailure } from '../src/client/app-server';
import type { UploadReceipt } from '../../protocol/app-server/v44';
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
  const { owner, port, start } = fixture({ [phase]: vi.fn(async () => { throw phase === 'upload' ? new UploadFailure('failed') : new Error('rejected'); }) });
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

it('repairs the retained exact upload without create/upload/send replay, then continues only on a gesture', async () => {
  const { owner, port, start } = fixture({ upload: vi.fn(async () => { throw new OutcomeUncertain(); }) });
  await start({ ...draft, files: [new File(['first'], 'first.md'), new File(['second'], 'second.md')] });
  const lost = owner.session(session.id)!;
  const reply = gate<import('../../protocol/app-server/v44').UploadOutcome>();
  port.status = vi.fn(() => reply.promise);
  const repair = owner.recoverUpload(lost, port, false);
  await owner.recoverUpload(lost, port, false);
  expect(port.status).toHaveBeenCalledExactlyOnceWith(session, lost.operations[0]);
  reply.resolve({ state: 'ready', files: [{ receipt, file: { name: 'first.md', batch_id: receipt.batch_id }, path: '/native/first.md' }] });
  await repair;
  const repaired = owner.session(session.id)!;
  expect(repaired.receipts).toEqual([receipt]); expect(repaired.phase).toBe('paused');
  expect(port.upload).toHaveBeenCalledOnce(); expect(port.create).toHaveBeenCalledOnce(); expect(port.send).not.toHaveBeenCalled();
  const second = { ...receipt, token: 'second' }; port.upload = vi.fn(async () => second);
  await owner.continueUploads(repaired, port);
  expect(port.upload).toHaveBeenCalledOnce(); expect(port.send).toHaveBeenCalledOnce(); expect(port.create).toHaveBeenCalledOnce();
  await owner.continueUploads(repaired, port); expect(port.send).toHaveBeenCalledOnce();
});
it('retained known failure requires a read before one explicit Retry; removal preserves text and order', async () => {
  const { owner, port, start } = fixture({ upload: vi.fn(async () => { throw Error('carrier lost'); }), status: vi.fn(async () => ({ state: 'failed' as const })) });
  await start({ ...draft, files: [new File(['a'], 'a'), new File(['b'], 'b')] });
  const uncertain = owner.session(session.id)!;
  await owner.recoverUpload(uncertain, port, true); expect(port.upload).toHaveBeenCalledOnce();
  await owner.recoverUpload(uncertain, port, false);
  const failed = owner.session(session.id)!; expect(failed.phase).toBe('failed');
  const reply = gate<UploadReceipt>(); port.upload = vi.fn(() => reply.promise);
  const retry = owner.recoverUpload(failed, port, true); await owner.recoverUpload(failed, port, true);
  expect(port.upload).toHaveBeenCalledOnce(); reply.resolve(receipt); await retry;
  const ready = owner.session(session.id)!; expect(ready.operations[0]).not.toBe(failed.operations[0]);
  owner.removeUpload(ready, ready.attachmentIds[0]);
  expect(owner.session(session.id)?.draft.text).toBe(draft.text);
  expect(owner.session(session.id)?.draft.files.map(file => file.name)).toEqual(['b']);
  expect(port.send).not.toHaveBeenCalled();
});

it('captured retry and continuation receipts survive retirement without a second allocation', async () => {
  const files = [new File(['a'], 'a'), new File(['b'], 'b')];
  const { owner, port, start } = fixture({ upload: async () => { throw Error('lost'); }, status: async () => ({ state: 'failed' }) });
  await start({ ...draft, files });
  await owner.recoverUpload(owner.session(session.id)!, port, false);
  port.upload = vi.fn(async (_, __, acknowledged) => { acknowledged(receipt); throw Error('retired after ready'); });
  await owner.recoverUpload(owner.session(session.id)!, port, true);
  expect(owner.session(session.id)).toMatchObject({ phase: 'paused', receipts: [receipt] });
  const second = { ...receipt, token: 'second' };
  port.upload = vi.fn(async (_, __, acknowledged) => { acknowledged(second); throw Error('retired after ready'); });
  await owner.continueUploads(owner.session(session.id)!, port);
  expect(owner.session(session.id)).toMatchObject({ phase: 'paused', receipts: [receipt, second] });
  expect(port.upload).toHaveBeenCalledOnce(); expect(port.send).not.toHaveBeenCalled();
  await owner.continueUploads(owner.session(session.id)!, port);
  expect(port.upload).toHaveBeenCalledOnce(); expect(port.create).toHaveBeenCalledOnce(); expect(port.send).toHaveBeenCalledOnce();
});

it('removing an unrelated ready card cannot turn an uncertain upload into another upload', async () => {
  const { owner, port, start } = fixture({ upload: vi.fn().mockResolvedValueOnce(receipt).mockRejectedValueOnce(new OutcomeUncertain()) });
  await start({ ...draft, files: [new File(['a'], 'a'), new File(['b'], 'b')] });
  const uncertain = owner.session(session.id)!;
  owner.removeUpload(uncertain, uncertain.attachmentIds[0]);
  const remaining = owner.session(session.id)!;
  expect(remaining).toMatchObject({ phase: 'uncertain', failedPhase: 'uploading', uploadIndex: 0, receipts: [] });
  expect(remaining.operations).toEqual([uncertain.operations[1]]);
  await owner.continueUploads(remaining, port);
  expect(port.upload).toHaveBeenCalledTimes(2); expect(port.send).not.toHaveBeenCalled();
});

it('an uncertain turn admission cannot be reopened by removing an attachment', async () => {
  const { owner, port, start } = fixture({ send: vi.fn(async () => { throw new OutcomeUncertain(); }) });
  await start({ ...draft, files: [new File(['a'], 'a')] });
  const uncertain = owner.session(session.id)!;
  expect(uncertain).toMatchObject({ phase: 'uncertain', failedPhase: 'admitting' });
  owner.removeUpload(uncertain, uncertain.attachmentIds[0]);
  expect(owner.session(session.id)).toBe(uncertain);
  await owner.continueUploads(owner.session(session.id)!, port);
  expect(port.create).toHaveBeenCalledOnce(); expect(port.upload).toHaveBeenCalledOnce(); expect(port.send).toHaveBeenCalledOnce();
});

it.each(['discard', 'admit'])('sealing first submission transfers File ownership and %s releases it', async finish => {
  const { AttachmentIntake } = await import('../src/client/uploads');
  const { capabilities } = await import('./fixture');
  const intake = new AttachmentIntake();
  const file = new File(['owned'], 'owned.md'); intake.add([{ file }], capabilities.upload_policy);
  const creation = gate<CreatedSession>();
  const { owner, port } = fixture({ create: () => creation.promise });
  const work = owner.submit('draft', { ...draft, files: intake.snapshot().map(row => row.file!) }, port, () => intake.clear());
  expect(intake.snapshot()).toEqual([]); expect(owner.draft('draft')?.draft.files).toEqual([file]);
  if (finish === 'discard') { creation.reject(new Error('known pre-create failure')); await work; owner.discard(owner.draft('draft')!); }
  else { creation.resolve(session); expect(await work).toBe(true); }
  expect(owner.draft('draft')?.draft.files).toEqual([]); expect(intake.snapshot()).toEqual([]);
});

it('known creation rejection returns ownership to a live intake for further editing', async () => {
  const { AttachmentIntake } = await import('../src/client/uploads');
  const { capabilities } = await import('./fixture');
  const intake = new AttachmentIntake();
  const file = new File(['owned'], 'owned.md'); intake.add([{ file }], capabilities.upload_policy);
  const id = intake.snapshot()[0].id;
  const creation = gate<CreatedSession>(); const { owner, port } = fixture({ create: () => creation.promise });
  const work = owner.submit('draft', { ...draft, files: [file], attachmentIds: [id] }, port, () => intake.clear(), (files, ids) => intake.restoreDraft(files, ids));
  expect(intake.snapshot()).toEqual([]);
  creation.reject(new Error('known rejection')); expect(await work).toBe(false);
  expect(owner.draft('draft')?.draft.files).toEqual([]);
  expect(intake.snapshot()).toMatchObject([{ file, id, status: 'draft' }]);
  intake.add([{ file: new File(['next'], 'next.md') }], capabilities.upload_policy);
  expect(intake.snapshot().map(row => row.file!.name)).toEqual(['owned.md', 'next.md']);
  intake.remove(id); expect(intake.snapshot().map(row => row.file!.name)).toEqual(['next.md']);
});

it('continuation preserves a known no-commit upload failure instead of degrading it to uncertainty', async () => {
  const { owner, port, start } = fixture({ upload: vi.fn(async () => { throw new UploadFailure('failed'); }) });
  await start({ ...draft, files: [new File(['a'], 'a'), new File(['b'], 'b')] });
  const failed = owner.session(session.id)!;
  port.upload = vi.fn(async () => receipt);
  await owner.recoverUpload(failed, port, true);
  const ready = owner.session(session.id)!;
  port.upload = vi.fn(async () => { throw new UploadFailure('failed'); });
  await owner.continueUploads(ready, port);
  expect(owner.session(session.id)).toMatchObject({ phase: 'failed', failedPhase: 'uploading', uploadIndex: 1, receipts: [receipt] });
  expect(port.send).not.toHaveBeenCalled();
});
