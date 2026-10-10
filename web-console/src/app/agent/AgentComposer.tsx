import type { FirstSubmission } from '../new-conversation/first-submit';
import { message } from '../../locale/translation';
import { useTranslation, useNotice } from '../../locale/react';
/* Copyright (c) 2026 DeepSeek. MIT. Source-derived; see PROVENANCE.md. */
// Presentation extracted from DeepSeek Harness ui-conversation/AgentComposer.
// Native textarea replaces Lexical. Commands are client grammar; effects are typed.
import { useEffect, useLayoutEffect, useState, useRef, useSyncExternalStore, type ReactNode } from 'react';
import { AttachmentIntake, transferInputs, pasteText, type IntakeInput, type IntakeFile, type UploadPort } from '../../client/uploads';
import type { UploadPolicy, UploadReceipt, UploadedFile, UserInputBlock } from '../../../../protocol/app-server/v41';
import { commands, available, parseCommand, type CommandId } from '../commands/registry';
import { matchCommands } from '../commands/matching';
import { ModelPicker, type ModelPickerState } from '../composer/ModelPicker';
import { CommandMenu, type MenuAction } from '../commands/CommandMenu';
import { useInputTrigger } from '../composer/input-trigger';
import { editableContent } from '../composer/editor-content';
import { composerSubmissionPolicy, type SubmitGesture } from '../composer/submission-policy';
import { useBusyEnter } from '../composer/preferences';
import { StopSequence, type StopScope } from '../composer/stop-sequence';
import { AttachmentCard } from '../../presentation/attachments/AttachmentCard';
import { Button } from '../../presentation/primitives/Button';
import css from '../../presentation/agent/Composer.module.css';
const emptyContent: UserInputBlock[] = [];
export function AgentComposer({ messageLabel, busyEnterBehavior, modelPicker, onRetainedRemove, onRetainedRecover, intakeOwner, uploadPolicy, onReconcile, binding = 'default', firstSubmission, disabled, submitDisabled = false, busy, active, onSend, onUpload, onCancel, onCommand, commandAvailable, hasGoal = false, lineageSwitchSafe = false, initialContent = emptyContent, consumed, model, permission, onDraftSend, cancellationAvailable = !disabled, cancellationScope }: {
  messageLabel?: string; busyEnterBehavior?: import('../composer/preferences').BusyEnterBehavior;
  modelPicker?: ModelPickerState;
  onRetainedRemove?: (id: string) => void; onRetainedRecover?: (retry: boolean) => void;
  intakeOwner?: AttachmentIntake; uploadPolicy?: UploadPolicy; onReconcile?: UploadPort['status'];
  firstSubmission?: FirstSubmission; binding?: string; submitDisabled?: boolean; disabled: boolean; busy: boolean; active: boolean; model?: ReactNode; permission?: ReactNode;
  onDraftSend?: (text: string, files: readonly File[], ids?: readonly string[]) => Promise<boolean>; cancellationAvailable?: boolean; cancellationScope?: StopScope;
  onSend: (text: string, receipts: readonly UploadReceipt[], delivery: 'send' | 'steer', acknowledged?: () => void) => Promise<boolean>;
  onUpload: (files: readonly File[], operation?: string) => Promise<UploadedFile[]>; onCancel: () => void;
  onCommand?: (id: CommandId) => void; commandAvailable?: (id: CommandId) => boolean; hasGoal?: boolean; lineageSwitchSafe?: boolean; initialContent?: UserInputBlock[];
  consumed?: { id: string; sequence: number };
}) {
  const tx = useTranslation();
  const [preferredBusyEnter] = useBusyEnter();
  const busyEnter = busyEnterBehavior ?? preferredBusyEnter;
  const [stopSequence] = useState(() => new StopSequence());
  const composing = useRef(false);
  const escapePress = useRef<{ event: KeyboardEvent; accept: ReturnType<StopSequence['prepare']> } | undefined>(undefined);
  const [restoreSupported, setRestoreSupported] = useState(() => editableContent(initialContent));
  disabled = disabled || !restoreSupported;
  const [localIntake] = useState(() => new AttachmentIntake());
  const intake = intakeOwner ?? localIntake;
  const selections = useSyncExternalStore(intake.subscribe, intake.snapshot);
  const retained = firstSubmission && !['rejected', 'discarded', 'admitted'].includes(firstSubmission.phase) ? firstSubmission : undefined;
  const inTranscript = !!retained && !!(retained.session || retained.existingSessionId) && ['attaching', 'uploading', 'admitting'].includes(retained.phase);
  const shownText = inTranscript ? '' : retained?.draft.text;
  const files: readonly Pick<IntakeFile, "id" | "file" | "name" | "receipt" | "status" | "reason" | "error">[] = inTranscript ? [] : retained ? retained.draft.files.map((file, index) => ({ id: retained.attachmentIds[index], name: file.name, file, receipt: retained.receipts[index], status: retained.receipts[index] ? 'ready' : index === retained.uploadIndex && retained.failedPhase === 'uploading' && retained.phase === 'uncertain' ? 'uncertain' : index === retained.uploadIndex && retained.failedPhase === 'uploading' && retained.phase === 'failed' ? 'failed' : index === retained.uploadIndex && retained.phase === 'uploading' ? 'uploading' : 'draft', reason: undefined, error: undefined })) : selections;
  const [error, setError] = useNotice();
  const [dragging, setDragging] = useState(false);
  const pending = files.some(file => file.status !== 'ready' && !(onDraftSend && file.status === 'draft'));
  const port: UploadPort = { upload: onUpload, status: onReconcile ?? (async () => ({ state: 'unresolved' })) };
  const pick = (picked: IntakeInput[]) => {
    if (!disabled && !busy) intake.add(picked, uploadPolicy, onDraftSend ? undefined : port);
  };
  const [draft, setDraft] = useState(() => (inTranscript ? '' : firstSubmission?.draft.text) ?? (restoreSupported ? initialContent.flatMap(block => block.type === 'text' ? [block.text] : []).join('') : ''));
  const draftBinding = useRef(binding);
  const restoredInput = useRef(initialContent);
  const retainedInput = useRef(retained);
  if (retained && (retainedInput.current !== retained || draft !== shownText)) {
    retainedInput.current = retained;
    setDraft(shownText ?? '');

  }
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
  const pastedCaret = useRef<{ binding: string; caret: number } | undefined>(undefined);
  useLayoutEffect(() => { const next = pastedCaret.current; pastedCaret.current = undefined; if (next?.binding === binding) input.current?.setSelectionRange(next.caret, next.caret); }, [draft, binding]);
  const trigger = useInputTrigger(input);
  if (draftBinding.current !== binding || restoredInput.current !== initialContent) {
    draftBinding.current = binding;
    restoredInput.current = initialContent;
    const supported = editableContent(initialContent);
    setRestoreSupported(supported);
    setDraft(supported ? initialContent.flatMap(block => block.type === 'text' ? [block.text] : []).join('') : '');
    setRestored(supported ? initialContent.flatMap(block => block.type === 'upload' ? [block] : []) : []);
    if (!intakeOwner) intake.clear(); setError(''); setDragging(false);
    invocation.current = undefined; trigger.dismiss();
  }
  const query = trigger.state?.query;
  const modelInvocation = useRef<string | undefined>(undefined);
  const [modelRequested, setModelRequested] = useState(false);
  useEffect(() => setModelRequested(false), [binding]);
  const modelMenu = !!modelPicker && (modelRequested || trigger.state?.source === 'typed' && trigger.state.query.toLowerCase() === 'model') && commandAvailable?.('model') !== false && !disabled && !busy;
  const menu = !modelMenu && !!trigger.state && !disabled && !busy;
  const commandRows = onCommand ? matchCommands(query ?? '', commands.filter(command => available(command, active, hasGoal, lineageSwitchSafe) && (commandAvailable?.(command.id) ?? true))) : [];
  const rows: readonly { id: MenuAction }[] = [
    ...(trigger.state?.source === 'launcher' ? [{ id: 'file' as const }] : []),
    ...commandRows.filter(command => command.id === 'goal'),
    ...commandRows.filter(command => command.id !== 'goal'),
  ];
  const highlight = Math.min(trigger.state?.highlight ?? 0, Math.max(0, rows.length - 1));
  const parsed = parseCommand(draft);
  const selected = menu ? rows[highlight]?.id : undefined;
  const selectedCommand = selected && selected !== 'file' ? selected : parsed.type === 'command' ? parsed.id : undefined;
  const facts = { running: active, busyEnter, actionable: !!draft.trim() || files.length > 0 || restored.length > 0,
    draftKind: parsed.type === 'text' ? 'message' as const : selectedCommand ? 'command' as const : 'unsupported-command' as const,
    blocked: disabled || submitDisabled, acknowledging: busy, uploadsPending: pending, cancellationAvailable };
  const primary = composerSubmissionPolicy(facts);
  useEffect(() => { stopSequence.reset(); }, [stopSequence, binding, cancellationScope?.authority, cancellationScope?.identity, active, cancellationAvailable, menu]);
  useEffect(() => {
    // Window blur only resets; keyboard recognition stays on the focused editor.
    window.addEventListener('blur', stopSequence.reset);
    return () => { window.removeEventListener('blur', stopSequence.reset); stopSequence.reset(); };
  }, [stopSequence]);
  const invoke = (id: CommandId) => {
    if (disabled || busy) return;
    if (files.length || restored.length) { setError(message('agent:copy.remove-draft-attachments-before-invoking-a-command')); return; }
    const definition = commands.find(command => command.id === id)!;
    if (!onCommand || !available(definition, active, hasGoal, lineageSwitchSafe) || commandAvailable?.(id) === false) { setError(message('agent:copy.command-unavailable-in-the-current-session-state')); return; }
    if (id === 'model' && modelPicker) { modelInvocation.current = trigger.state?.source === 'launcher' ? undefined : draft; setModelRequested(true); trigger.dismiss(); return; }
    invocation.current = { id, draft: trigger.state?.source === 'launcher' ? '' : draft };
    trigger.dismiss(); setError(''); trigger.restore(); onCommand(id);
  };
  const selectMenu = (id: MenuAction) => {
    if (disabled || busy) return;
    if (id === 'file') { trigger.dismiss(); trigger.restore(); picker.current?.click(); }
    else invoke(id);
  };
  useEffect(() => {
    if (!menu) return;
    const outside = (event: PointerEvent) => { if (!root.current?.contains(event.target as Node)) trigger.dismiss(); };
    document.addEventListener('pointerdown', outside);
    return () => document.removeEventListener('pointerdown', outside);
  }, [menu]);
  const submit = async (gesture: SubmitGesture = 'enter') => {
    if (menu && rows[highlight]) { selectMenu(rows[highlight].id); return; }
    const action = composerSubmissionPolicy(facts, gesture);
    // Enter submits a draft; it never cancels an empty running Session.
    if (action.disabled || action.kind === 'stop') return;
    if (action.kind === 'command') {
      if (selectedCommand) invoke(selectedCommand);
      else setError(message('agent:copy.unsupported-command-edit-the-draft-it-will-not-be-sent-as-a-prompt'));
      return;
    }
    const submitted = draft;
    const owner = binding;
    try {
      if (await (onDraftSend ? onDraftSend(submitted, files.map(file => file.file!), files.map(file => file.id)) : onSend(submitted, [...restored, ...files.map(file => file.receipt!)], action.delivery, () => intake.clear())) && draftBinding.current === owner) { setDraft(current => current === submitted ? '' : current); intake.clear(); setRestored([]); input.current?.focus(); }
    } catch (cause) {
      if (draftBinding.current === owner) setError(String(cause));
    }
  };
  return <div ref={root} className={css.root} onDragOver={event => { if (!disabled && !busy && event.dataTransfer.types.includes('Files')) { event.preventDefault(); setDragging(true); } }}
    onDragLeave={event => { if (!event.currentTarget.contains(event.relatedTarget as Node | null)) setDragging(false); }}
    onDrop={event => { event.preventDefault(); setDragging(false); if (!disabled && !busy) pick(transferInputs(event.dataTransfer)); }}>
    {dragging && <div className="attachment-drop" role="status">{tx('agent:upload.drop')}</div>}
    {error && <p role="alert">{error}</p>}
    {!restoreSupported && <p role="alert">{tx('agent:agent-composer.cannot-restore-this-ordered-native-input-in-the-flat-web-editor')}</p>}
    <div className={css.card} data-composer-card aria-busy={busy}>
      {modelMenu && <ModelPicker state={modelPicker!} close={() => { setModelRequested(false); trigger.dismiss(); trigger.restore(); }} chosen={() => { setModelRequested(false); trigger.dismiss(); setDraft(value => value === modelInvocation.current || value.trim().toLowerCase() === '/model' ? '' : value); trigger.restore(); }}/>}
      {menu && <CommandMenu rows={rows} active={highlight} select={selectMenu} highlight={trigger.highlight}
        fileDescription={uploadPolicy ? tx('agent:upload.limits', { count: uploadPolicy.max_uploads_per_user_input, file: uploadPolicy.max_file_bytes, batch: uploadPolicy.max_upload_bytes_per_user_input }) : tx('agent:upload.policy')} />}
      {restored.map((receipt, index) => <div key={JSON.stringify([receipt.batch_id, receipt.token])}><span>{tx('agent:agent-composer.native-restored-upload-batch')}{' '}{receipt.batch_id}</span><Button disabled={disabled || busy} onClick={() => setRestored(current => current.filter((_, at) => at !== index))}>{tx('agent:agent-composer.remove-draft-upload')}</Button></div>)}
      <div className={css.attachments} aria-label={tx('agent:agent-composer.draft-attachments')}>{files.map(item => <div key={item.id}>
        <DraftAttachment name={item.name} file={item.file} remove={firstSubmission && onRetainedRemove ? () => onRetainedRemove(item.id) : disabled || busy ? undefined : () => intake.remove(item.id)} />
        <small role="status">{item.reason ? tx(`agent:upload.${item.reason}`) : tx(`agent:upload.${item.status}`)}</small>
        {item.status === 'failed' && <Button disabled={!onRetainedRecover && (disabled || busy)} onClick={() => onRetainedRecover ? onRetainedRecover(true) : void intake.upload(item.id, port)}>{tx('agent:upload.retry')}</Button>}
        {item.status === 'uncertain' && <Button disabled={!onRetainedRecover && (disabled || busy)} onClick={() => onRetainedRecover ? onRetainedRecover(false) : void intake.reconcile(item.id, port)}>{tx('agent:upload.reconcile')}</Button>}
      </div>)}</div>
      <div className={css.editor}>
        <textarea ref={input} className={css.input} aria-label={messageLabel ?? tx('agent:agent-composer.message')} placeholder={onDraftSend ? tx('agent:agent-composer.describe-what-you-want-to-do') : tx('agent:agent-composer.give-this-session-a-task')}
          aria-controls={menu ? 'composer-commands' : undefined} aria-activedescendant={menu && rows[highlight] ? `command-${rows[highlight].id}` : undefined}
          value={draft} disabled={disabled} readOnly={busy} rows={1} onChange={event => { setDraft(event.target.value); trigger.track(event.target.value); setError(''); }}
          onPaste={event => {
            if (disabled || busy) return;
            const pasted = transferInputs(event.clipboardData);
            if (!pasted.length) return;
            pick(pasted);
            if (composing.current) return;
            event.preventDefault();
            const text = event.clipboardData.getData('text/plain');
            if (!text) return;
            const element = event.currentTarget;
            const next = pasteText(draft, element.selectionStart, element.selectionEnd, text);
            setDraft(next.value); trigger.track(next.value);
            pastedCaret.current = { binding, caret: next.caret };
          }}
          onBlur={() => { composing.current = false; stopSequence.reset(); }}
          onCompositionStart={() => { composing.current = true; stopSequence.reset(); }} onCompositionEnd={() => { composing.current = false; }}
          onKeyDownCapture={event => {
            if (event.key === 'Escape') escapePress.current = { event: event.nativeEvent, accept: stopSequence.prepare() };
          }}
          onKeyDown={event => {
            if (event.key === 'Escape') {
              const press = escapePress.current; escapePress.current = undefined;
              if (event.defaultPrevented || composing.current || event.nativeEvent.isComposing || event.nativeEvent.keyCode === 229
                || event.repeat || event.ctrlKey || event.metaKey || event.altKey || event.shiftKey) { stopSequence.reset(); return; }
              if (menu) { stopSequence.reset(); event.preventDefault(); trigger.dismiss(); return; }
              if (!active || !cancellationAvailable || !cancellationScope || document.activeElement !== event.currentTarget) { stopSequence.reset(); return; }
              event.preventDefault(); if (press?.event === event.nativeEvent) press.accept(cancellationScope, onCancel); return;
            }
            if (event.defaultPrevented || composing.current || event.nativeEvent.isComposing || event.nativeEvent.keyCode === 229) return;
            if (menu && event.key === 'Tab') { event.preventDefault(); if (event.shiftKey) trigger.dismiss(); else if (rows[highlight]) selectMenu(rows[highlight].id); return; }
            if (menu && rows.length && (event.key === 'ArrowDown' || event.key === 'ArrowUp')) { event.preventDefault(); trigger.highlight((highlight + (event.key === 'ArrowDown' ? 1 : rows.length - 1)) % rows.length); return; }
            if (event.key === 'Enter' && !event.shiftKey && !event.nativeEvent.isComposing && event.nativeEvent.keyCode !== 229) {
              event.preventDefault(); void submit(event.ctrlKey || event.metaKey ? 'accelerated' : 'enter');
            }
          }} />
      </div>
      <div className={css.row}>
        <div className={css.tools}>
          <button type="button" className={css.add} aria-label={tx('commands:menu.add')} title={tx('commands:menu.add')} aria-haspopup="listbox" aria-expanded={!!menu} disabled={disabled || busy} onMouseDown={event => event.preventDefault()} onClick={trigger.toggle}>+</button>
          <input ref={picker} type="file" hidden multiple aria-label={tx('agent:agent-composer.attach-files')} disabled={disabled || busy} onChange={event => { pick(Array.from(event.target.files ?? []).map(file => ({ file }))); event.target.value = ''; }}/>
          {permission}
        </div>
        <div className={css.trailing}>
          {model}
          <button type="button" className={css.primary} data-composer-primary={primary.kind} aria-label={tx(primary.label)} title={tx(primary.title)}
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

function DraftAttachment({ file, name, remove }: { file: File | null; name: string; remove?: () => void }) {
  const tx = useTranslation();
  const [url, setUrl] = useState<string>();
  const [failed, setFailed] = useState(false);
  useEffect(() => {
    setFailed(false);
    if (!file?.type.startsWith('image/')) return;
    const value = URL.createObjectURL(file); setUrl(value);
    return () => URL.revokeObjectURL(value);
  }, [file]);
  return <AttachmentCard name={name} image={!!file?.type.startsWith('image/')} url={url} error={failed ? tx('agent:copy.image-preview-unavailable') : undefined} onDecodeError={() => setFailed(true)} onRemove={remove} />;
}
