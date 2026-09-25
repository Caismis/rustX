import { createActor, waitFor } from 'xstate';
import { expect, it, vi } from 'vitest';
import { firstSubmitMachine, type FirstSubmitPort, type FirstDraft, type CreatedSession } from '../src/app/new-conversation/first-submit';
import { OutcomeUncertain, RpcFailure } from '../src/client/app-server';
import { WorkspaceHostError } from '../src/workspaces/host';
import type { UploadReceipt } from '../../protocol/app-server/v21';
function gate<T>() { let resolve!: (value: T) => void, reject!: (reason: unknown) => void; const promise = new Promise<T>((a, b) => { resolve = a; reject = b; }); return { promise, resolve, reject }; }
const session: CreatedSession = { id: 'native-session', node: 'native-node', conversation: 'native-conversation' };
const draft: FirstDraft = { workspaceId: 'registered', text: 'Task', files: [], model: { model: 'explicit' } };
function fixture(overrides: Partial<FirstSubmitPort> = {}) {
  const port: FirstSubmitPort = { current: () => true, create: vi.fn(async () => session), attach: vi.fn(async () => {}), model: vi.fn(async () => {}), upload: vi.fn(async () => ({ batch_id: 'batch', token: 'token' } as UploadReceipt)), send: vi.fn(async () => {}), ...overrides };
  const actor = createActor(firstSubmitMachine, { input: { port } }).start(); return { actor, port };
}
it('opening and missing authorized Workspace create nothing; repeated submit creates exactly one native Session', async () => {
  const creation = gate<CreatedSession>(); const { actor, port } = fixture({ create: vi.fn(() => creation.promise) });
  expect(port.create).not.toHaveBeenCalled(); actor.send({ type: 'SUBMIT', draft: { ...draft, workspaceId: '' } }); expect(port.create).not.toHaveBeenCalled();
  actor.send({ type: 'SUBMIT', draft }); actor.send({ type: 'SUBMIT', draft: { ...draft, workspaceId: 'other' } });
  expect(port.create).toHaveBeenCalledTimes(1); expect(actor.getSnapshot().context.draft?.workspaceId).toBe('registered');
  creation.resolve(session); await waitFor(actor, s => s.matches('session')); expect(port.send).toHaveBeenCalledTimes(1); actor.stop();
});
it('explicit model observation fences every upload and the first turn', async () => {
  const model = gate<void>(); const { actor, port } = fixture({ model: vi.fn(() => model.promise) });
  actor.send({ type: 'SUBMIT', draft: { ...draft, files: [new File(['x'], 'x')] } }); await waitFor(actor, s => s.matches('applying_session_model'));
  expect(port.model).toHaveBeenCalledWith(session, draft.model); expect(port.upload).not.toHaveBeenCalled(); expect(port.send).not.toHaveBeenCalled();
  model.resolve(); await waitFor(actor, s => s.matches('session')); expect(port.send).toHaveBeenCalledTimes(1); actor.stop();
});
it.each(['attach', 'model', 'send'] as const)('confirmed Session survives %s failure with no automatic mutation replay', async phase => {
  const failed = vi.fn(async () => { throw new Error('Outcome uncertain'); }); const { actor, port } = fixture({ [phase]: failed });
  actor.send({ type: 'SUBMIT', draft }); await waitFor(actor, s => s.matches('failed'));
  expect(actor.getSnapshot().context.session).toEqual(session); actor.send({ type: 'SUBMIT', draft }); expect(port.create).toHaveBeenCalledTimes(1); expect(failed).toHaveBeenCalledTimes(1);
  if (phase !== 'send') expect(port.send).not.toHaveBeenCalled(); actor.stop();
});
it('individual upload acknowledgements survive a later failed upload', async () => {
  const receipt = { batch_id: 'committed', token: 'receipt' } as UploadReceipt;
  const upload = vi.fn().mockResolvedValueOnce(receipt).mockRejectedValueOnce(new Error('uncertain'));
  const { actor, port } = fixture({ upload }); actor.send({ type: 'SUBMIT', draft: { ...draft, files: [new File(['a'], 'a'), new File(['b'], 'b')] } });
  await waitFor(actor, s => s.matches('failed')); expect(actor.getSnapshot().context.receipts).toEqual([receipt]); expect(port.send).not.toHaveBeenCalled(); expect(upload).toHaveBeenCalledTimes(2); actor.stop();
});
it.each(['creating_session', 'applying_session_model', 'submitting_turn'] as const)('retired authority fences a pending %s completion', async phase => {
  const pending = gate<CreatedSession>();
  const { actor, port } = fixture(phase === 'creating_session' ? { create: () => pending.promise } : phase === 'applying_session_model' ? { model: async () => { await pending.promise; } } : { send: async () => { await pending.promise; } });
  actor.send({ type: 'SUBMIT', draft }); await waitFor(actor, s => s.matches(phase)); actor.send({ type: 'RETIRE' }); pending.resolve(session);
  await pending.promise; expect(actor.getSnapshot().matches('retired')).toBe(true); if (phase !== 'submitting_turn') expect(port.send).not.toHaveBeenCalled(); actor.stop();
});
it.each(['create', 'model', 'upload', 'send'] as const)('authority replacement fences %s completion even before navigation retires the actor', async phase => {
  let generation = 1;
  const pending = gate<CreatedSession>();
  const completion = vi.fn(async () => { await pending.promise; return phase === 'create' ? session : { batch_id: 'committed', token: 'receipt' }; });
  const { actor, port } = fixture({ current: () => generation === 1, [phase]: completion });
  actor.send({ type: 'SUBMIT', draft: { ...draft, files: [new File(['native'], 'native.txt')] } });
  await waitFor(actor, () => completion.mock.calls.length === 1);
  generation = 2; pending.resolve(session);
  await waitFor(actor, state => state.matches('failed'));
  expect(completion).toHaveBeenCalledTimes(1);
  if (phase !== 'send') expect(port.send).not.toHaveBeenCalled();
  actor.send({ type: 'SUBMIT', draft }); expect(completion).toHaveBeenCalledTimes(1);
  actor.stop();
});

it.each([
  new WorkspaceHostError('Workspace revoked'),
  new RpcFailure({ code: -32000, message: 'Creation rejected' }),
])('known pre-commit rejection preserves an editable draft and only explicit SUBMIT retries: %s', async error => {
  const create = vi.fn().mockRejectedValueOnce(error).mockResolvedValueOnce(session);
  const { actor, port } = fixture({ create });
  actor.send({ type: 'SUBMIT', draft });
  await waitFor(actor, s => s.matches('drafting') && s.context.error === error);
  expect(actor.getSnapshot().context.draft).toEqual(draft);
  expect(actor.getSnapshot().context.session).toBeUndefined();
  expect(create).toHaveBeenCalledTimes(1); expect(port.attach).not.toHaveBeenCalled();
  const corrected = { ...draft, workspaceId: 'corrected', text: 'Corrected task' };
  actor.send({ type: 'SUBMIT', draft: corrected });
  await waitFor(actor, s => s.matches('session'));
  expect(create).toHaveBeenCalledTimes(2); expect(create).toHaveBeenLastCalledWith(corrected);
  expect(actor.getSnapshot().context.error).toBeUndefined(); actor.stop();
});
it.each([new OutcomeUncertain(), new WorkspaceHostError('Unknown outcome', undefined, true)])('uncertain creation cannot be replayed by SUBMIT: %s', async error => {
  const { actor, port } = fixture({ create: vi.fn(async () => { throw error; }) });
  actor.send({ type: 'SUBMIT', draft }); await waitFor(actor, s => s.matches('uncertain_creation'));
  expect(actor.getSnapshot().context.session).toBeUndefined();
  expect(actor.getSnapshot().context.draft).toEqual(draft);
  actor.send({ type: 'SUBMIT', draft });
  expect(port.create).toHaveBeenCalledTimes(1); expect(port.attach).not.toHaveBeenCalled(); actor.stop();
});
it('records a confirmed create before the authority fence can stop attachment', async () => {
  const creation = gate<CreatedSession>(); let current = true;
  const { actor, port } = fixture({ current: () => current, create: vi.fn(() => creation.promise) });
  actor.send({ type: 'SUBMIT', draft });
  creation.resolve(session); current = false;
  await waitFor(actor, s => s.matches('failed'));
  expect(actor.getSnapshot().context.session).toEqual(session);
  for (const effect of [port.attach, port.model, port.upload, port.send]) expect(effect).not.toHaveBeenCalled();
  current = true; actor.send({ type: 'SUBMIT', draft });
  expect(port.create).toHaveBeenCalledTimes(1); expect(actor.getSnapshot().context.session).toEqual(session); actor.stop();
});
