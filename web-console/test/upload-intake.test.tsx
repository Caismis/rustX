import { Suspense, startTransition, useLayoutEffect, useState } from 'react';
import { useAttachmentIntake } from '../src/app/composer/use-attachment-intake';
import { act, cleanup, fireEvent, render } from '@testing-library/react';
import { afterEach, expect, it, vi } from 'vitest';
import { AttachmentIntake, AttachmentIntakes, intake, pasteText, transferInputs, type UploadPort } from '../src/client/uploads';
import { AgentComposer } from '../src/app/agent/AgentComposer';
import { capabilities } from './fixture';
import type { UploadedFile, UploadOutcome } from '../../protocol/app-server/v57';
const policy = capabilities.upload_policy;
const file = (name = 'document.md', size = 1) => new File([new Uint8Array(size)], name);
const ready: UploadedFile = { receipt: { session_id: 'A', batch_id: 'original', token: 'original' }, file: { batch_id: 'original', name: 'document.md' }, path: '/native/document.md' };
const deferred = <T,>() => { let resolve!: (value: T) => void; const promise = new Promise<T>(done => { resolve = done; }); return { promise, resolve }; };
afterEach(cleanup);

it('accepts generic files beyond the old count, per-file and aggregate caps', () => {
  const selected = file('large.md', 3 * 1024 * 1024);
  const result = intake(Array.from({ length: 24 }, () => ({ file: selected })), policy).files;
  expect(result).toHaveLength(24);
  expect(result.every(row => row.status === 'draft')).toBe(true);
  expect(new Set(result.map(row => row.id)).size).toBe(24);
  const owner = new AttachmentIntake();
  owner.add([{ file: selected }], policy);
  owner.add(Array.from({ length: 24 }, () => ({ file: selected })), policy);
  expect(owner.snapshot()).toHaveLength(25);
});
it('refuses an invalid addition atomically, preserving existing files and starting no uploads', () => {
  const owner = new AttachmentIntake(); owner.add([{ file: file('keep.md') }], policy);
  const previous = owner.snapshot();
  const port: UploadPort = { upload: vi.fn(async () => []), status: vi.fn(async (): Promise<UploadOutcome> => ({ state: 'absent' })) };
  for (const [input, reason] of [[{ file: file('../unsafe') }, 'name'], [{ file: file('folder'), directory: true }, 'directory'], [{ file: null }, 'unavailable']] as const) {
    expect(owner.add([{ file: file('valid.md') }, input], policy, port)).toBe(reason);
    expect(owner.snapshot()).toBe(previous);
  }
  expect(port.upload).not.toHaveBeenCalled();
});
it('normalizes drop and paste items exactly once and reports directories', () => {
  const selected = file();
  const data = { items: [{ kind: 'file', getAsFile: () => selected, webkitGetAsEntry: () => ({ isDirectory: false }) }], files: [selected] } as unknown as DataTransfer;
  expect(transferInputs(data)).toEqual([{ file: selected, name: undefined, directory: false }]);
  expect(intake(transferInputs(data), policy).files.map(row => row.status)).toEqual(intake([{ file: selected }], policy).files.map(row => row.status));
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
  fireEvent.click(ui.getByRole('button', { name: 'Add' }));
  const option = ui.getByRole('option', { name: /file$/ });
  expect(document.getElementById(option.getAttribute('aria-describedby')!)?.textContent).not.toContain('bytes');
  expect(input.value).toBe('α x\ny ω');
  expect(input.selectionStart).toBe(5); expect(input.selectionEnd).toBe(5);
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
  expect(ui.getAllByRole('button', { name: /^Check status for / })).toHaveLength(1);
  expect(ui.getAllByRole('button', { name: /^Retry uploading / })).toHaveLength(1);
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

it.each(['picker', 'drop', 'paste'])('%s refuses invalid additions with a toast and preserves the text draft', async source => {
  const owner = new AttachmentIntake(); const send = vi.fn(async () => true);
  const ui = render(<AgentComposer intakeOwner={owner} uploadPolicy={policy} disabled={false} busy={false} active={false} onDraftSend={send} onUpload={async () => []} onSend={send} onCancel={() => {}} />);
  const files = [file('ok.md'), file('../unsafe')];
  const input = ui.getByLabelText('Message');
  fireEvent.change(input, { target: { value: 'retained text' } });
  if (source === 'picker') fireEvent.change(ui.getByLabelText('Attach files'), { target: { files } });
  if (source === 'paste') fireEvent.paste(input, { clipboardData: { files, items: [], getData: () => '' } });
  if (source === 'drop') fireEvent.drop(input, { dataTransfer: { files, items: [], types: ['Files'] } });
  expect(owner.snapshot()).toEqual([]);
  expect((await ui.findByRole('alert')).textContent).toContain('Unsafe filename');
  expect(send).not.toHaveBeenCalled();
  expect((input as HTMLTextAreaElement).value).toBe('retained text');
});

it('an admitted first submission releases the attachment presentation for the next message', async () => {
  const owner = new AttachmentIntake();
  const ui = render(<AgentComposer intakeOwner={owner} uploadPolicy={policy} firstSubmission={{ binding: 'draft', authority: 0, draft: { workspaceId: 'workspace', text: '', files: [] }, phase: 'admitted', receipts: [], operations: [], attachmentIds: [] }} disabled={false} busy={false} active={false} onUpload={async () => [ready]} onSend={async () => true} onCancel={() => {}} />);
  await act(async () => fireEvent.change(ui.getByLabelText('Attach files'), { target: { files: [file()] } }));
  expect(ui.getByText('document.md')).toBeTruthy(); expect(ui.container.querySelector('[data-attachment-state=ready]')).toBeTruthy();
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

it('unavailable DataTransfer file items refuse intake with a toast, never empty Files', async () => {
  const data = { types: ['Files'], items: [{ kind: 'file', getAsFile: () => null, webkitGetAsEntry: () => ({ name: 'unavailable.pdf', isDirectory: false }) }], files: [] } as unknown as DataTransfer;
  const owner = new AttachmentIntake(); const upload = vi.fn();
  const ui = render(<AgentComposer intakeOwner={owner} uploadPolicy={policy} disabled={false} busy={false} active={false} onUpload={upload} onSend={async () => false} onCancel={() => {}}/>);
  fireEvent.drop(document.body, { dataTransfer: data });
  expect(owner.snapshot()).toEqual([]);
  expect(upload).not.toHaveBeenCalled();
  expect((await ui.findByRole('alert')).textContent).toBe('This file is unavailable or unsupported. Select it again.');
});

it('document drop overlay balances nested drags, ignores text and blocks unavailable intake', () => {
  const owner = new AttachmentIntake();
  const props = { intakeOwner: owner, uploadPolicy: policy, disabled: false, busy: false, active: false, onDraftSend: async () => false, onUpload: async () => [], onSend: async () => false, onCancel: () => {} };
  const ui = render(<AgentComposer {...props}/>);
  const dataTransfer = { types: ['Files'], files: [file()], items: [], dropEffect: 'none' };
  fireEvent.dragEnter(document.body, { dataTransfer: { types: ['text/plain'] } });
  expect(ui.queryByRole('status')).toBeNull();
  fireEvent.dragEnter(document.body, { dataTransfer });
  fireEvent.dragEnter(ui.getByLabelText('Message'), { dataTransfer });
  expect(ui.getByRole('status').textContent).toBe('Drag files or images here to add them');
  fireEvent.dragOver(document.body, { dataTransfer }); expect(dataTransfer.dropEffect).toBe('copy');
  fireEvent.dragLeave(ui.getByLabelText('Message'), { dataTransfer, clientX: 10, clientY: 10 });
  expect(ui.getByRole('status')).toBeTruthy();
  fireEvent.drop(document.body, { dataTransfer });
  expect(ui.queryByRole('status')).toBeNull(); expect(owner.snapshot()).toHaveLength(1);
  ui.rerender(<AgentComposer {...props} disabled/>);
  fireEvent.dragEnter(document.body, { dataTransfer });
  expect(ui.getByRole('status').textContent).toBe('Files and images cannot be added right now');
  fireEvent.dragOver(document.body, { dataTransfer }); expect(dataTransfer.dropEffect).toBe('none');
  fireEvent.drop(document.body, { dataTransfer }); expect(owner.snapshot()).toHaveLength(1);
  fireEvent.dragEnter(document.body, { dataTransfer });
  fireEvent(window, new Event('dragend'));
  expect(ui.queryByRole('status')).toBeNull();
});

it('one file drop belongs to the visible composer, then to its targeted or focused pane', () => {
  const parent = new AttachmentIntake(), child = new AttachmentIntake();
  const props = { uploadPolicy: policy, disabled: false, busy: false, active: false, onDraftSend: async () => false, onUpload: async () => [], onSend: async () => false, onCancel: () => {} };
  const surface = (hidden: boolean) => <>
    <div hidden={hidden}><AgentComposer {...props} intakeOwner={parent} messageLabel="Parent input"/></div>
    <AgentComposer {...props} intakeOwner={child} messageLabel="Child input"/>
  </>;
  const ui = render(surface(true));
  const dataTransfer = { types: ['Files'], files: [file()], items: [], dropEffect: 'none' };
  fireEvent.dragEnter(document.body, { dataTransfer });
  expect(ui.getAllByRole('status')).toHaveLength(1);
  fireEvent.drop(document.body, { dataTransfer });
  expect(parent.snapshot()).toHaveLength(0); expect(child.snapshot()).toHaveLength(1);
  ui.rerender(surface(false));
  ui.getByLabelText('Child input').focus();
  fireEvent.drop(document.body, { dataTransfer });
  expect(parent.snapshot()).toHaveLength(0); expect(child.snapshot()).toHaveLength(2);
  fireEvent.drop(ui.getByLabelText('Parent input'), { dataTransfer });
  expect(parent.snapshot()).toHaveLength(1); expect(child.snapshot()).toHaveLength(2);
});
