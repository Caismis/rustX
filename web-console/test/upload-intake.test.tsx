import { Suspense, startTransition, useLayoutEffect, useState } from 'react';
import { useAttachmentIntake } from '../src/app/composer/use-attachment-intake';
import { act, cleanup, fireEvent, render } from '@testing-library/react';
import { afterEach, expect, it, vi } from 'vitest';
import { AttachmentIntake, AttachmentIntakes, intake, pasteText, transferInputs, type UploadPort } from '../src/client/uploads';
import { AgentComposer } from '../src/app/agent/AgentComposer';
import { capabilities } from './fixture';
import type { UploadedFile, UploadOutcome } from '../../protocol/app-server/v37';
const policy = capabilities.upload_policy;
const file = (name = 'document.md', size = 1) => new File([new Uint8Array(size)], name);
const ready: UploadedFile = { receipt: { session_id: 'A', batch_id: 'original', token: 'original' }, file: { batch_id: 'original', name: 'document.md' }, path: '/native/document.md' };
const deferred = <T,>() => { let resolve!: (value: T) => void; const promise = new Promise<T>(done => { resolve = done; }); return { promise, resolve }; };
afterEach(cleanup);

it('admits per-file outcomes and explicit duplicate identities, with bounded over-count feedback', () => {
  const selected = file();
  const result = intake([{ file: selected }, { file: selected }, { file: file('../unsafe') }, { file: file('folder'), directory: true }, { file: file('large', policy.max_file_bytes + 1) }], [], policy);
  expect(result.map(row => row.reason)).toEqual([undefined, undefined, 'name', 'directory', 'size']);
  expect(new Set(result.map(row => row.id)).size).toBe(5);
  expect(intake(Array.from({ length: 10000 }, () => ({ file: selected })), [], policy)).toMatchObject([{ reason: 'count' }]);
  const owner = new AttachmentIntake();
  for (let i = 0; i < 10; i++) owner.add(Array.from({ length: 10000 }, () => ({ file: selected })), policy);
  expect(owner.snapshot()).toHaveLength(1);
});
it('checks below, at and above every selection storage bound', () => {
  for (const size of [policy.max_file_bytes - 1, policy.max_file_bytes]) expect(intake([{ file: file('x', size) }], [], policy)[0].status).toBe('draft');
  expect(intake([{ file: file('x', policy.max_file_bytes + 1) }], [], policy)[0].reason).toBe('size');
  for (const total of [policy.max_upload_bytes_per_user_input - 1, policy.max_upload_bytes_per_user_input]) expect(intake([{ file: file('a', policy.max_file_bytes) }, { file: file('b', total - policy.max_file_bytes) }], [], policy).every(row => row.status === 'draft')).toBe(true);
  expect(intake([{ file: file('a', policy.max_file_bytes) }, { file: file('b', policy.max_file_bytes) }, { file: file('c') }], [], policy)[2].reason).toBe('batch');
  for (const count of [policy.max_uploads_per_user_input - 1, policy.max_uploads_per_user_input]) expect(intake(Array.from({ length: count }, () => ({ file: file() })), [], policy)).toHaveLength(count);
  expect(intake(Array.from({ length: policy.max_uploads_per_user_input + 1 }, () => ({ file: file() })), [], policy)).toMatchObject([{ reason: 'count' }]);
});
it('normalizes drop and paste items exactly once and reports directories', () => {
  const selected = file();
  const data = { items: [{ kind: 'file', getAsFile: () => selected, webkitGetAsEntry: () => ({ isDirectory: false }) }], files: [selected] } as unknown as DataTransfer;
  expect(transferInputs(data)).toEqual([{ file: selected, name: undefined, directory: false }]);
  expect(intake(transferInputs(data), [], policy).map(row => row.status)).toEqual(intake([{ file: selected }], [], policy).map(row => row.status));
});
it('preserves exact text and selection when inserting a file-plus-text paste', () => {
  expect(pasteText('α FIRST ω', 2, 7, 'x\ny')).toEqual({ value: 'α x\ny ω', caret: 5 });
  const owner = new AttachmentIntake();
  const ui = render(<AgentComposer intakeOwner={owner} uploadPolicy={policy} disabled={false} busy={false} active={false} onDraftSend={async () => false} onUpload={async () => []} onSend={async () => false} onCancel={() => {}} />);
  const input = ui.getByLabelText('Message') as HTMLTextAreaElement;
  fireEvent.change(input, { target: { value: 'α FIRST ω' } }); input.setSelectionRange(2, 7);
  fireEvent.paste(input, { clipboardData: { files: [file()], items: [], getData: () => 'x\ny' } });
  expect(input.value).toBe('α x\ny ω'); expect(input.selectionStart).toBe(5); expect(input.selectionEnd).toBe(5);
  expect(owner.snapshot()).toHaveLength(1);
  expect(ui.getByTitle(/2097152 bytes per file/)).toBeTruthy();
});
it('uncertain reconciliation reuses original receipts and performs no second upload', async () => {
  const owner = new AttachmentIntake();
  owner.add([{ file: file() }], policy);
  const port: UploadPort = { upload: vi.fn(async () => { throw Error('lost reply'); }), status: vi.fn(async (): Promise<UploadOutcome> => ({ state: 'unresolved' })) };
  const id = owner.snapshot()[0].id;
  await owner.upload(id, port);
  expect(owner.snapshot()[0].status).toBe('uncertain');
  port.status = vi.fn(async (): Promise<UploadOutcome> => ({ state: 'ready', files: [ready] }));
  await owner.reconcile(id, port);
  expect(owner.snapshot()[0].receipt).toEqual(ready.receipt);
  expect(port.upload).toHaveBeenCalledOnce(); expect(port.status).toHaveBeenCalledOnce();
});
it('known failure retries once, preserves order, and remove never fabricates rollback', async () => {
  const owner = new AttachmentIntake(); owner.add([{ file: file() }, { file: file('other') }], policy);
  const first = owner.snapshot()[0], other = owner.snapshot()[1];
  const port: UploadPort = { upload: vi.fn(async () => { throw Error('failed'); }), status: vi.fn(async (): Promise<UploadOutcome> => ({ state: 'failed' })) };
  await owner.upload(first.id, port);
  const gate = deferred<UploadedFile[]>(); port.upload = vi.fn(() => gate.promise);
  const retry = owner.upload(first.id, port); const duplicate = owner.upload(first.id, port);
  expect(port.upload).toHaveBeenCalledOnce(); expect(owner.snapshot()[0].operation).not.toBe(first.operation);
  expect(owner.snapshot()[1]).toBe(other);
  owner.remove(first.id); gate.resolve([ready]); await retry; await duplicate;
  expect(owner.snapshot()).toEqual([other]);
});
it('a gated result cannot publish into a replacement Composer binding', async () => {
  const old = new AttachmentIntake(), replacement = new AttachmentIntake();
  const gate = deferred<UploadedFile[]>();
  const props = { uploadPolicy: policy, disabled: false, busy: false, active: false, onUpload: () => gate.promise, onSend: vi.fn(async () => true), onCancel: () => {} };
  const ui = render(<AgentComposer {...props} binding="old" intakeOwner={old} />);
  fireEvent.change(ui.getByLabelText('Attach files'), { target: { files: [file()] } });
  ui.rerender(<AgentComposer {...props} binding="replacement" intakeOwner={replacement} />);
  await act(async () => gate.resolve([ready]));
  expect(ui.queryByText('document.md')).toBeNull(); expect(replacement.snapshot()).toEqual([]);
  expect(old.snapshot()[0].receipt).toEqual(ready.receipt);
  expect(props.onSend).not.toHaveBeenCalled();
});

it('rebind stops a finite selection and gates old publication until explicit native repair', async () => {
  const owner = new AttachmentIntake(); owner.bind('authority-A');
  const gate = deferred<UploadedFile[]>();
  const statusGate = deferred<UploadOutcome>();
  const port: UploadPort = { upload: vi.fn(() => gate.promise), status: vi.fn(() => statusGate.promise) };
  owner.add([{ file: file() }, { file: file('second') }], policy, port);
  const [first, second] = owner.snapshot();
  expect(first.status).toBe('uploading'); expect(second.status).toBe('queued');
  owner.bind('authority-B');
  expect(owner.snapshot().map(row => row.status)).toEqual(['uncertain', 'failed']);
  const ui = render(<AgentComposer intakeOwner={owner} uploadPolicy={policy} disabled={false} busy={false} active={false} onUpload={async () => []} onSend={async () => false} onCancel={() => {}}/>);
  expect(ui.getAllByRole('button', { name: 'Check status' })).toHaveLength(1);
  expect(ui.getAllByRole('button', { name: 'Retry' })).toHaveLength(1);
  ui.unmount();
  expect(owner.snapshot()[0].receipt).toBeUndefined(); expect(port.upload).toHaveBeenCalledOnce();
  const repair = owner.reconcile(first.id, port);
  expect(port.status).toHaveBeenCalledExactlyOnceWith(first.operation);
  gate.resolve([ready]); await gate.promise; await Promise.resolve();
  expect(owner.snapshot()[0].status).toBe('reconciling');
  expect(owner.snapshot()[0].receipt).toBeUndefined();
  await owner.reconcile(first.id, port);
  expect(port.status).toHaveBeenCalledOnce();
  statusGate.resolve({ state: 'ready', files: [ready] }); await repair;
  expect(owner.snapshot()[0].receipt).toEqual(ready.receipt); expect(port.upload).toHaveBeenCalledOnce();
  await owner.reconcile(second.id, port);
  expect(port.status).toHaveBeenCalledExactlyOnceWith(first.operation);
  port.upload = vi.fn(async () => [ready]);
  await owner.upload(second.id, port);
  expect(port.upload).toHaveBeenCalledOnce();
  expect(port.upload).toHaveBeenCalledWith([second.file], expect.any(String));
  expect(owner.snapshot()[1].operation).not.toBe(second.operation);
});

it.each(['picker', 'drop', 'paste'])('%s produces the same visible mixed outcomes and blocks send', source => {
  const owner = new AttachmentIntake(); const send = vi.fn(async () => true);
  const ui = render(<AgentComposer intakeOwner={owner} uploadPolicy={policy} disabled={false} busy={false} active={false} onDraftSend={send} onUpload={async () => []} onSend={send} onCancel={() => {}} />);
  const files = [file('ok.md'), file('../unsafe')];
  const input = ui.getByLabelText('Message');
  fireEvent.change(input, { target: { value: 'retained text' } });
  if (source === 'picker') fireEvent.change(ui.getByLabelText('Attach files'), { target: { files } });
  if (source === 'paste') fireEvent.paste(input, { clipboardData: { files, items: [], getData: () => '' } });
  if (source === 'drop') fireEvent.drop(input, { dataTransfer: { files, items: [] } });
  expect(owner.snapshot().map(row => row.status)).toEqual(['draft', 'rejected']);
  fireEvent.keyDown(input, { key: 'Enter' }); expect(send).not.toHaveBeenCalled();
  expect((input as HTMLTextAreaElement).value).toBe('retained text');
});

it('an admitted first submission releases the attachment presentation for the next message', async () => {
  const owner = new AttachmentIntake();
  const ui = render(<AgentComposer intakeOwner={owner} uploadPolicy={policy} firstSubmission={{ binding: 'draft', authority: 0, draft: { workspaceId: 'workspace', text: '', files: [] }, phase: 'admitted', receipts: [], operations: [], attachmentIds: [] }} disabled={false} busy={false} active={false} onUpload={async () => [ready]} onSend={async () => true} onCancel={() => {}} />);
  await act(async () => fireEvent.change(ui.getByLabelText('Attach files'), { target: { files: [file()] } }));
  expect(ui.getByText('document.md')).toBeTruthy(); expect(ui.getByText('Uploaded', { exact: true })).toBeTruthy();
});

it('file-only paste preserves selected text and a busy editor cannot be changed by paste', () => {
  const owner = new AttachmentIntake();
  const props = { intakeOwner: owner, uploadPolicy: policy, disabled: false, busy: false, active: false, onDraftSend: async () => false, onUpload: async () => [], onSend: async () => false, onCancel: () => {} };
  const ui = render(<AgentComposer {...props} />); const input = ui.getByLabelText('Message') as HTMLTextAreaElement;
  fireEvent.change(input, { target: { value: 'keep selection' } }); input.setSelectionRange(0, 4);
  fireEvent.paste(input, { clipboardData: { files: [file()], items: [], getData: () => '' } });
  expect(input.value).toBe('keep selection'); expect(input.selectionStart).toBe(0); expect(input.selectionEnd).toBe(4);
  ui.rerender(<AgentComposer {...props} busy />);
  fireEvent.paste(input, { clipboardData: { files: [file()], items: [], getData: () => 'replacement' } });
  expect(input.value).toBe('keep selection'); expect(owner.snapshot()).toHaveLength(1);
});

it('semantic navigation retires File owners while remount retains the current binding', () => {
  const owners = new AttachmentIntakes();
  let previous: AttachmentIntake | undefined;
  for (let index = 0; index < 100; index++) {
    const owner = owners.activate(`session-${index}`, `session-${index}`, new AttachmentIntake()); owner.add([{ file: file() }], policy);
    expect(previous?.snapshot() ?? []).toEqual([]);
    expect(owners.size).toBe(1); expect(owners.lookup(`session-${index}`)).toBe(owner);
    previous = owner;
  }
  owners.dispose(); expect(owners.size).toBe(0); expect(previous!.snapshot()).toEqual([]);
  previous!.add([{ file: file() }], policy);
  owners.lookup("after-dispose")!.add([{ file: file() }], policy);
  expect(previous!.snapshot()).toEqual([]); expect(owners.lookup("after-dispose")!.snapshot()).toEqual([]); expect(owners.size).toBe(0);
});
it('remove and successful draft clear release every retained File', () => {
  const owner = new AttachmentIntake(); owner.add([{ file: file() }, { file: file('other') }], policy);
  owner.remove(owner.snapshot()[0].id); expect(owner.snapshot()).toHaveLength(1);
  owner.clear(); expect(owner.snapshot()).toEqual([]);
});

it('native message acknowledgement releases Files even while the sending promise is held', async () => {
  const owner = new AttachmentIntake(); owner.add([{ file: file() }], policy);
  await owner.upload(owner.snapshot()[0].id, { upload: async () => [ready], status: async () => ({ state: 'unresolved' }) });
  const reply = deferred<boolean>(); let acknowledged: (() => void) | undefined;
  const ui = render(<AgentComposer intakeOwner={owner} uploadPolicy={policy} disabled={false} busy={false} active={false} onUpload={async () => []} onSend={(_text, _receipts, _delivery, ack) => { acknowledged = ack; return reply.promise; }} onCancel={() => {}} />);
  fireEvent.keyDown(ui.getByLabelText('Message'), { key: 'Enter' });
  expect(owner.snapshot()).toHaveLength(1);
  act(() => acknowledged!()); expect(owner.snapshot()).toEqual([]);
  await act(async () => reply.resolve(true));
});

it('only a committed replacement retires the visible Composer owner; suspended render and remount preserve it', async () => {
  const owners = new AttachmentIntakes();
  const gate = deferred<void>();
  const rendered = vi.fn(), committed = vi.fn();
  let blocked = true;
  let navigate!: (key: string) => void;
  function CommitGate({ selected }: { selected: string }) {
    rendered(selected);
    useLayoutEffect(() => { committed(selected); }, [selected]);
    if (selected === 'B' && blocked) throw gate.promise;
    return null;
  }
  function Composer({ selected }: { selected: string }) {
    const intake = useAttachmentIntake(owners, selected, selected);
    return <AgentComposer intakeOwner={intake} uploadPolicy={policy} disabled={false} busy={false} active={false} onDraftSend={async () => false} onUpload={async () => []} onSend={async () => false} onCancel={() => {}} />;
  }
  function Fixture() {
    const [selected, setSelected] = useState('A'); navigate = setSelected;
    return <Suspense fallback={<p>Suspended</p>}><Composer key={selected} selected={selected}/><CommitGate selected={selected}/></Suspense>;
  }
  const ui = render(<Fixture/>);
  const a = owners.lookup('A')!;
  const retire = vi.spyOn(a, 'retire');
  const selectedFile = file('A.md');
  fireEvent.change(ui.getByLabelText('Attach files'), { target: { files: [selectedFile] } });
  expect(ui.getByText('A.md')).toBeTruthy(); expect(a.snapshot()[0].file).toBe(selectedFile);
  await act(async () => { startTransition(() => navigate('B')); });
  expect(rendered).toHaveBeenCalledWith('B'); expect(committed).not.toHaveBeenCalledWith('B');
  expect(ui.queryByText('Suspended')).toBeNull(); expect(ui.getByText('A.md')).toBeTruthy();
  expect(owners.lookup('A')).toBe(a); expect(owners.lookup('B')).toBeUndefined();
  expect(a.snapshot()[0].file).toBe(selectedFile); expect(retire).not.toHaveBeenCalled();
  await act(async () => { blocked = false; gate.resolve(); });
  expect(committed).toHaveBeenCalledWith('B'); expect(retire).toHaveBeenCalledOnce();
  expect(a.snapshot()).toEqual([]); expect(owners.lookup('A')).toBeUndefined();
  const b = owners.lookup('B')!; expect(b).toBeDefined(); expect(owners.size).toBe(1);
  expect(ui.queryByText('A.md')).toBeNull();
  fireEvent.change(ui.getByLabelText('Attach files'), { target: { files: [file('B.md')] } });
  ui.unmount(); expect(owners.lookup('B')).toBe(b); expect(b.snapshot()).toHaveLength(1);
  const remount = render(<Composer selected="B"/>);
  expect(owners.lookup('B')).toBe(b); expect(remount.getByText('B.md')).toBeTruthy();
  expect(retire).toHaveBeenCalledOnce();
});

it('unavailable DataTransfer file items remain visible rejected metadata, never empty Files', () => {
  const data = { items: [{ kind: 'file', getAsFile: () => null, webkitGetAsEntry: () => ({ name: 'unavailable.pdf', isDirectory: false }) }], files: [] } as unknown as DataTransfer;
  const owner = new AttachmentIntake(); const upload = vi.fn();
  owner.add(transferInputs(data), policy, { upload, status: async () => ({ state: 'absent' }) });
  expect(owner.snapshot()).toMatchObject([{ file: null, name: 'unavailable.pdf', status: 'rejected', reason: 'unavailable' }]);
  expect(upload).not.toHaveBeenCalled();
  const ui = render(<AgentComposer intakeOwner={owner} uploadPolicy={policy} disabled={false} busy={false} active={false} onUpload={upload} onSend={async () => false} onCancel={() => {}}/>);
  expect(ui.getByText('unavailable.pdf')).toBeTruthy();
  expect(ui.getByText('This file is unavailable or unsupported. Select it again.')).toBeTruthy();
});
