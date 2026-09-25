/* Copyright (c) 2026 DeepSeek. MIT. Source-derived; see PROVENANCE.md. */
// Presentation extracted from DeepSeek Harness ui-conversation/AgentComposer.
// Native textarea replaces Lexical. Commands are client grammar; effects are typed.
import { useEffect, useState, useRef, type ReactNode } from 'react';
import { UPLOAD_MAX_BYTES, UPLOAD_BATCH_MAX_BYTES, DRAFT_MAX_FILES } from '../../client/uploads';
import type { UploadReceipt, UploadedFile, UserInputBlock } from '../../../../protocol/app-server/v23';
import { commands, available, parseCommand, type CommandId } from '../commands/registry';
import { matchCommands } from '../commands/matching';
import { CommandMenu } from '../commands/CommandMenu';
import { useInputTrigger } from '../composer/input-trigger';
import { editableContent } from '../composer/editor-content';
import { composerSubmissionPolicy, type SubmitGesture } from '../composer/submission-policy';
import { useTextareaAutosize } from '../composer/useTextareaAutosize';
import { isOutcomeUncertain } from '../../client/app-server';
import { AttachmentCard } from '../../presentation/attachments/AttachmentCard';
import { Button } from '../../presentation/primitives/Button';
import css from '../../presentation/agent/Composer.module.css';
const emptyContent: UserInputBlock[] = [];
export function AgentComposer({ binding = 'default', disabled, submitDisabled = false, busy, active, onSend, onUpload, onCancel, onCommand, commandAvailable, hasGoal = false, lineageSwitchSafe = false, initialContent = emptyContent, consumed, model, permission, onDraftSend, cancellationAvailable = !disabled }: {
  binding?: string; submitDisabled?: boolean; disabled: boolean; busy: boolean; active: boolean; model?: ReactNode; permission?: ReactNode;
  onDraftSend?: (text: string, files: readonly File[]) => Promise<boolean>; cancellationAvailable?: boolean;
  onSend: (text: string, receipts: readonly UploadReceipt[], delivery: 'send' | 'steer') => Promise<boolean>;
  onUpload: (files: readonly File[]) => Promise<UploadedFile[]>; onCancel: () => void;
  onCommand?: (id: CommandId) => void; commandAvailable?: (id: CommandId) => boolean; hasGoal?: boolean; lineageSwitchSafe?: boolean; initialContent?: UserInputBlock[];
  consumed?: { id: string; sequence: number };
}) {
  const [restoreSupported, setRestoreSupported] = useState(() => editableContent(initialContent));
  disabled = disabled || !restoreSupported;
  type DraftFile = { id: number; file: File; status: 'draft' | 'uploading' | 'complete' | 'failed' | 'uncertain'; receipt?: UploadReceipt; error?: string };
  const [files, setFiles] = useState<DraftFile[]>([]);
  const nextId = useRef(0);
  const transferring = useRef(false);
  const [error, setError] = useState('');
  const [dragging, setDragging] = useState(false);
  const pending = files.some(file => file.status !== 'complete' && file.status !== 'draft');
  const pick = (picked: File[]) => {
    if (!picked.length || disabled || busy) return;
    if (transferring.current) { setError('Additional files were not added. Wait for the current upload to finish, then select them again.'); return; }
    if (files.length + picked.length > DRAFT_MAX_FILES || picked.some(file => file.size > UPLOAD_MAX_BYTES)
      || picked.reduce((sum, file) => sum + file.size, 0) > UPLOAD_BATCH_MAX_BYTES) { setError('Choose at most 8 files, 256 KiB each and 512 KiB per batch.'); return; }
    const batch: DraftFile[] = picked.map(file => ({ id: nextId.current++, file, status: onDraftSend ? 'draft' : 'uploading' }));
    if (onDraftSend) { setFiles(current => [...current, ...batch]); setError(''); return; }
    const owner = binding;
    transferring.current = true;
    setError(''); setFiles(current => [...current, ...batch]);
    void onUpload(picked).then(uploaded => {
      if (draftBinding.current !== owner) return;
      if (uploaded.length !== batch.length) throw new Error('Invalid upload response; outcome uncertain.');
      setFiles(current => current.map(file => {
        const index = batch.findIndex(item => item.id === file.id);
        return index < 0 ? file : { ...file, status: 'complete', receipt: uploaded[index].receipt };
      }));
    }).catch(cause => {
      if (draftBinding.current !== owner) return;
      setFiles(current => current.map(file => batch.some(item => item.id === file.id)
        ? { ...file, status: isOutcomeUncertain(cause) ? 'uncertain' : 'failed', error: String(cause) } : file));
    }).finally(() => { if (draftBinding.current === owner) transferring.current = false; });
  };
  const [draft, setDraft] = useState(() => restoreSupported ? initialContent.flatMap(block => block.type === 'text' ? [block.text] : []).join('') : '');
  const draftBinding = useRef(binding);
  const restoredInput = useRef(initialContent);
  const invocation = useRef<{ id: CommandId; draft: string } | undefined>(undefined);
  useEffect(() => {
    const invoked = invocation.current;
    if (!consumed || !invoked || invoked.id !== consumed.id) return;
    invocation.current = undefined;
    setDraft(current => current === invoked.draft ? '' : current);
  }, [consumed]);
  const [restored, setRestored] = useState(() => restoreSupported ? initialContent.flatMap(block => block.type === 'upload' ? [block] : []) : []);
  const picker = useRef<HTMLInputElement>(null);
  const input = useRef<HTMLTextAreaElement>(null), root = useRef<HTMLDivElement>(null);
  const trigger = useInputTrigger(input);
  const highlight = trigger.state?.highlight ?? 0;
  if (draftBinding.current !== binding || restoredInput.current !== initialContent) {
    draftBinding.current = binding;
    restoredInput.current = initialContent;
    const supported = editableContent(initialContent);
    setRestoreSupported(supported);
    setDraft(supported ? initialContent.flatMap(block => block.type === 'text' ? [block.text] : []).join('') : '');
    setRestored(supported ? initialContent.flatMap(block => block.type === 'upload' ? [block] : []) : []);
    setFiles([]); setError(''); setDragging(false);
    invocation.current = undefined; transferring.current = false; trigger.dismiss();
  }
  useTextareaAutosize(input, draft);
  const query = trigger.state?.query;
  const menu = onCommand && !!trigger.state && !disabled && !busy;
  const rows = matchCommands(query ?? '', commands.filter(command => available(command, active, hasGoal, lineageSwitchSafe) && (commandAvailable?.(command.id) ?? true)));
  const parsed = parseCommand(draft);
  const selectedCommand = menu && rows[highlight] ? rows[highlight].id : parsed.type === 'command' ? parsed.id : undefined;
  const facts = { running: active, actionable: !!draft.trim() || files.length > 0 || restored.length > 0,
    draftKind: parsed.type === 'text' ? 'message' as const : selectedCommand ? 'command' as const : 'unsupported-command' as const,
    blocked: disabled || submitDisabled, acknowledging: busy, uploadsPending: pending, cancellationAvailable };
  const primary = composerSubmissionPolicy(facts);
  const invoke = (id: CommandId) => {
    if (disabled || busy) return;
    if (files.length || restored.length) { setError('Remove draft attachments before invoking a command.'); return; }
    const definition = commands.find(command => command.id === id)!;
    if (!onCommand || !available(definition, active, hasGoal, lineageSwitchSafe) || commandAvailable?.(id) === false) { setError('Command unavailable in the current Session state.'); return; }
    invocation.current = { id, draft: trigger.state?.source === 'launcher' ? '' : draft };
    trigger.dismiss(); setError(''); trigger.restore(); onCommand(id);
  };
  useEffect(() => {
    if (!menu) return;
    const outside = (event: PointerEvent) => { if (!root.current?.contains(event.target as Node)) trigger.dismiss(); };
    document.addEventListener('pointerdown', outside);
    return () => document.removeEventListener('pointerdown', outside);
  }, [menu]);
  const submit = async (gesture: SubmitGesture = 'enter') => {
    if (menu && rows[highlight]) { invoke(rows[highlight].id); return; }
    const action = composerSubmissionPolicy(facts, gesture);
    // Enter submits a draft; it never cancels an empty running Session.
    if (action.disabled || action.kind === 'stop') return;
    if (action.kind === 'command') {
      if (selectedCommand) invoke(selectedCommand);
      else setError('Unsupported command. Edit the draft; it will not be sent as a prompt.');
      return;
    }
    const submitted = draft;
    const owner = binding;
    try {
      if (await (onDraftSend ? onDraftSend(submitted, files.map(file => file.file)) : onSend(submitted, [...restored, ...files.map(file => file.receipt!)], action.delivery)) && draftBinding.current === owner) { setDraft(current => current === submitted ? '' : current); setFiles([]); setRestored([]); input.current?.focus(); }
    } catch (cause) {
      if (draftBinding.current === owner) setError(String(cause));
    }
  };
  return <div ref={root} className={css.root} onDragOver={event => { if (!disabled && !busy && event.dataTransfer.types.includes('Files')) { event.preventDefault(); setDragging(true); } }}
    onDragLeave={event => { if (!event.currentTarget.contains(event.relatedTarget as Node | null)) setDragging(false); }}
    onDrop={event => { event.preventDefault(); setDragging(false); if (!disabled && !busy) pick(Array.from(event.dataTransfer.files)); }}>
    {dragging && <div className="attachment-drop" role="status">Drop attachments · 8 files · 256 KiB each</div>}
    {error && <p role="alert">{error}</p>}
    {!restoreSupported && <p role="alert">Cannot restore this ordered native input in the flat Web editor. Only uploads followed by at most one nonempty text block are editable. No input was reordered or sent; use an ordered-block client for this history.</p>}
    <div className={css.card} data-composer-card aria-busy={busy}>
      {menu && <CommandMenu rows={rows} active={highlight} select={invoke} highlight={trigger.highlight} />}
      {restored.map((receipt, index) => <div key={JSON.stringify([receipt.batch_id, receipt.token])}><span>Native restored upload batch {receipt.batch_id}</span><Button disabled={disabled || busy} onClick={() => setRestored(current => current.filter((_, at) => at !== index))}>Remove draft upload</Button></div>)}
      <div className={css.attachments} aria-label="Draft attachments">{files.map(item => <div key={item.id}><DraftAttachment file={item.file} remove={disabled || busy ? undefined : () => setFiles(current => current.filter(file => file.id !== item.id))} /><small role="status">{item.status === 'draft' ? 'Ready to upload on submit' : item.status === 'complete' ? 'Uploaded' : item.status === 'uploading' ? 'Uploading…' : item.status === 'uncertain' ? 'Upload outcome uncertain. Reconnect and inspect authoritative state; do not replay.' : item.error}</small></div>)}</div>
      <div className={css.editor}>
        <textarea ref={input} className={css.input} aria-label="Message" placeholder={onDraftSend ? "Describe what you want to do…" : "Give this Session a task…"}
          aria-controls={menu ? 'composer-commands' : undefined} aria-activedescendant={menu && rows[highlight] ? `command-${rows[highlight].id}` : undefined}
          value={draft} disabled={disabled} readOnly={busy} rows={1} onChange={event => { setDraft(event.target.value); trigger.track(event.target.value); setError(''); }}
          onPaste={event => { const pasted = Array.from(event.clipboardData.files); if (pasted.length) { event.preventDefault(); pick(pasted); } }}
          onKeyDown={event => {
            if (event.nativeEvent.isComposing || event.nativeEvent.keyCode === 229) return;
            if (menu && event.key === 'Escape') { event.preventDefault(); trigger.dismiss(); return; }
            if (menu && event.key === 'Tab') { event.preventDefault(); if (event.shiftKey) trigger.dismiss(); else if (rows[highlight]) invoke(rows[highlight].id); return; }
            if (menu && rows.length && (event.key === 'ArrowDown' || event.key === 'ArrowUp')) { event.preventDefault(); trigger.highlight((highlight + (event.key === 'ArrowDown' ? 1 : rows.length - 1)) % rows.length); return; }
            if (event.key === 'Enter' && !event.shiftKey && !event.nativeEvent.isComposing && event.nativeEvent.keyCode !== 229) {
              event.preventDefault(); void submit(event.ctrlKey || event.metaKey ? 'accelerated' : 'enter');
            }
          }} />
      </div>
      <div className={css.row}>
        <div className={css.tools}>
          {onCommand && <button type="button" className={css.add} aria-label="Commands" title="Commands" aria-haspopup="listbox" aria-expanded={!!menu} disabled={disabled || busy} onMouseDown={event => event.preventDefault()} onClick={trigger.toggle}>+</button>}
          <input ref={picker} type="file" hidden multiple aria-label="Attach files" disabled={disabled || busy} onChange={event => { pick(Array.from(event.target.files ?? [])); event.target.value = ''; }}/>
          <button type="button" className={css.attachment} aria-label="Add attachments" title="Add attachments" disabled={disabled || busy} onClick={() => picker.current?.click()}><svg viewBox="0 0 24 24" width="16" height="16" fill="none" stroke="currentColor" strokeWidth="1.5" aria-hidden><path d="m8 12 6-6a3 3 0 0 1 4 4l-8 8a5 5 0 0 1-7-7l9-9M6 14l8-8" /></svg></button>
          {permission}
        </div>
        <div className={css.trailing}>
          {model}
          <button type="button" className={css.primary} data-composer-primary={primary.kind} aria-label={primary.label} title={primary.title}
            disabled={primary.disabled} onMouseDown={event => event.preventDefault()} onClick={() => { if (primary.kind === 'stop') onCancel(); else void submit(); }}>
            {primary.kind === 'stop'
              ? <svg viewBox="0 0 16 16" width="16" height="16" aria-hidden><rect x="3" y="3" width="10" height="10" rx="3" fill="currentColor"/></svg>
              : <svg viewBox="0 0 16 16" width="16" height="16" aria-hidden><path d="M8 13V3m-4 4 4-4 4 4" fill="none" stroke="currentColor" strokeWidth="2"/></svg>}
          </button>
        </div>
      </div>
    </div>
  </div>;
}

function DraftAttachment({ file, remove }: { file: File; remove?: () => void }) {
  const [url, setUrl] = useState<string>();
  const [failed, setFailed] = useState(false);
  useEffect(() => {
    setFailed(false);
    if (!file.type.startsWith('image/')) return;
    const value = URL.createObjectURL(file); setUrl(value);
    return () => URL.revokeObjectURL(value);
  }, [file]);
  return <AttachmentCard name={file.name} image={file.type.startsWith('image/')} url={url} error={failed ? 'Image preview unavailable' : undefined} onDecodeError={() => setFailed(true)} onRemove={remove} />;
}
