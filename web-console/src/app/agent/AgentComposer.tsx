import { ComposerDraft, type ComposerSubmission, type ComposerConversation, type InputAcceptance } from '../composer/draft';
import { isOutcomeUncertain, RpcFailure } from '../../client/errors';
import type { FirstSubmission } from '../new-conversation/first-submit';
import { message } from '../../locale/translation';
import { useTranslation, useNotice } from '../../locale/react';
/* Copyright (c) 2026 DeepSeek. MIT. Source-derived; see PROVENANCE.md. */
// Presentation extracted from DeepSeek Harness ui-conversation/AgentComposer.
// Native textarea replaces Lexical. Commands are client grammar; effects are typed.
import { useCallback, useEffect, useLayoutEffect, useState, useRef, useSyncExternalStore, type ReactNode } from 'react';
import { AttachmentIntake, transferInputs, pasteText, type IntakeInput, type IntakeFile, type UploadPort } from '../../client/uploads';
import type { AttachmentTarget, UploadPolicy, UploadReceipt, UploadedFile, UserInputBlock } from '../../../../protocol/app-server/v57';
import { commands, available, parseCommand, type CommandId } from '../commands/registry';
import { matchCommands } from '../commands/matching';
import { ModelPicker, type ModelPickerState } from '../composer/ModelPicker';
import { CommandMenu, type MenuAction } from '../commands/CommandMenu';
import { IconPlusOutline16, IconSendOutline14 } from '../../presentation/primitives/icons';
import { useInputTrigger } from '../composer/input-trigger';
import { editableContent } from '../composer/editor-content';
import { composerSubmissionPolicy, type SubmitGesture } from '../composer/submission-policy';
import { useBusyEnter } from '../composer/preferences';
import { StopSequence, type StopScope } from '../composer/stop-sequence';
import { ComposerAttachments } from '../composer/ComposerAttachments';
import { Toast } from '../../presentation/primitives/Toast';
import { IconWarningOutline16 } from '../../presentation/primitives/icons';
import { Button } from '../../presentation/primitives/Button';
import css from '../../presentation/agent/Composer.module.css';
import { observeControlRow } from '../../presentation/agent/control-row-layout';
const emptyContent: UserInputBlock[] = [];
export function AgentComposer({ placeholder, messageLabel, busyEnterBehavior, modelPicker, onRetainedRemove, onRetainedRecover, intakeOwner, uploadPolicy, onReconcile, draftOwner, draftIdentity, receiptAuthority, conversationIdentity, submissionTarget, binding = 'default', firstSubmission, disabled, submitDisabled = false, busy, active, onSend, onUpload, onCancel, onCommand, commandAvailable, hasGoal = false, lineageSwitchSafe = false, initialContent = emptyContent, consumed, model, permission, onDraftSend, cancellationAvailable = active && !disabled, cancellationScope, cancellationOnly = false, separateStop = false }: {
  placeholder?: string; messageLabel?: string; busyEnterBehavior?: import('../composer/preferences').BusyEnterBehavior;
  modelPicker?: ModelPickerState;
  onRetainedRemove?: (id: string) => void; onRetainedRecover?: (retry: boolean) => void;
  intakeOwner?: AttachmentIntake; uploadPolicy?: UploadPolicy; onReconcile?: UploadPort['status'];
  draftOwner?: ComposerDraft; draftIdentity?: string; receiptAuthority?: string; conversationIdentity?: ComposerConversation; submissionTarget?: AttachmentTarget;
  firstSubmission?: FirstSubmission; binding?: string; submitDisabled?: boolean; disabled: boolean; busy: boolean; active: boolean; model?: ReactNode; permission?: ReactNode;
  onDraftSend?: (text: string, files: readonly File[], ids?: readonly string[], submission?: ComposerSubmission) => Promise<boolean>; cancellationAvailable?: boolean; cancellationScope?: StopScope;
  /** Exact live Root attachment permits cancellation while observation is unavailable. */
  cancellationOnly?: boolean;
  separateStop?: boolean;
  onSend: (text: string, receipts: readonly UploadReceipt[], delivery: 'send' | 'steer', acknowledged?: (acceptance?: InputAcceptance, conversation?: ComposerConversation) => void) => Promise<boolean>;
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
  const editorIdentity = draftIdentity ?? binding;
  const [localDraft] = useState(() => new ComposerDraft());
  const owner = draftOwner ?? localDraft;
  const state = useSyncExternalStore(owner.subscribe, owner.snapshot);
  const conversation = conversationIdentity ?? submissionTarget;
  useLayoutEffect(() => { owner.activate(editorIdentity, initialContent, receiptAuthority, conversation); }, [owner, editorIdentity, initialContent, receiptAuthority, conversation?.session_id, conversation?.conversation_id]);
  const editor = state?.key === editorIdentity ? state : undefined;
  const restoreSupported = editor?.supported ?? editableContent(initialContent);
  disabled = disabled || !restoreSupported;
  const cancellationEditor = cancellationOnly && cancellationAvailable && !!cancellationScope;
  const [localIntake] = useState(() => new AttachmentIntake());
  const intake = intakeOwner ?? localIntake;
  const selections = useSyncExternalStore(intake.subscribe, intake.snapshot);
  const retained = firstSubmission && !['rejected', 'discarded', 'admitted'].includes(firstSubmission.phase) ? firstSubmission : undefined;
  const inTranscript = !!retained && !!(retained.session || retained.existingSessionId) && ['attaching', 'uploading', 'admitting'].includes(retained.phase);
  const shownText = inTranscript ? '' : retained?.draft.text;
  const files: readonly Pick<IntakeFile, "id" | "file" | "name" | "receipt" | "status" | "error">[] = inTranscript ? [] : retained ? retained.draft.files.map((file, index) => ({ id: retained.attachmentIds[index], name: file.name, file, receipt: retained.receipts[index], status: retained.receipts[index] ? 'ready' : index === retained.uploadIndex && retained.failedPhase === 'uploading' && retained.phase === 'uncertain' ? 'uncertain' : index === retained.uploadIndex && retained.failedPhase === 'uploading' && retained.phase === 'failed' ? 'failed' : index === retained.uploadIndex && retained.phase === 'uploading' ? 'uploading' : 'draft', error: undefined })) : selections;
  const [error, setError] = useNotice();
  const [uploadNotice, setUploadNotice] = useNotice();
  const [noticeSequence, setNoticeSequence] = useState(0);
  const pending = files.some(file => file.status !== 'ready' && !(onDraftSend && file.status === 'draft'));
  const port: UploadPort = { upload: onUpload, status: onReconcile ?? (async () => ({ state: 'unresolved' })) };
  const pick = useCallback((picked: IntakeInput[]) => {
    if (disabled || busy || !editor) return;
    const rejection = intake.add(picked, uploadPolicy, onDraftSend ? undefined : { upload: onUpload, status: onReconcile ?? (async () => ({ state: 'unresolved' })) });
    if (rejection) { setUploadNotice(message(`agent:upload.${rejection}`)); setNoticeSequence(sequence => sequence + 1); }
    else if (picked.length) owner.attachmentsEdited(editor.incarnation);
  }, [disabled, busy, editor, intake, uploadPolicy, onDraftSend, onUpload, onReconcile, owner]);
  const draft = retained ? shownText ?? '' : editor?.text ?? '';
  const setDraft = (text: string) => { if (editor) owner.edit(editor.incarnation, text); };
  const invocation = useRef<{ id: CommandId; incarnation: number; revision: number } | undefined>(undefined);
  useEffect(() => {
    const invoked = invocation.current;
    if (!consumed || !invoked || invoked.id !== consumed.id) return;
    invocation.current = undefined; owner.consumeRevision(invoked.incarnation, invoked.revision);
  }, [consumed, owner]);
  const restored = editor?.restored ?? [];
  const row = useRef<HTMLDivElement>(null);
  useLayoutEffect(() => observeControlRow(row.current!), []);
  const picker = useRef<HTMLInputElement>(null);
  const input = useRef<HTMLTextAreaElement>(null), root = useRef<HTMLDivElement>(null);
  const pastedCaret = useRef<{ binding: string; caret: number } | undefined>(undefined);
  useLayoutEffect(() => { const next = pastedCaret.current; pastedCaret.current = undefined; if (next?.binding === binding) input.current?.setSelectionRange(next.caret, next.caret); }, [draft, binding]);
  const trigger = useInputTrigger(input);
  const rememberSelection = () => {
    const element = input.current;
    if (element && editor) owner.select(editor.incarnation, { start: element.selectionStart, end: element.selectionEnd, direction: element.selectionDirection });
  };
  useLayoutEffect(() => {
    const selection = owner.snapshot()?.selection;
    if (selection) input.current?.setSelectionRange(selection.start, selection.end, selection.direction);
  }, [owner, editor?.incarnation]);
  useEffect(() => { setError(''); setUploadNotice(''); invocation.current = undefined; if (!intakeOwner) intake.clear(); }, [editorIdentity]);
  const query = trigger.state?.query;
  const modelInvocation = useRef<{ incarnation: number; revision: number } | undefined>(undefined);
  const [modelRequested, setModelRequested] = useState(false);
  useEffect(() => setModelRequested(false), [binding]);
  const modelMenu = !!modelPicker && (modelRequested || trigger.state?.source === 'typed' && trigger.state.query.toLowerCase() === 'model') && commandAvailable?.('model') !== false && !disabled && !busy;
  const modelRevision = modelInvocation.current ?? (trigger.state?.source === 'typed' ? editor : undefined);
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
    blocked: disabled || submitDisabled, acknowledging: busy || owner.blocked(editor), uploadsPending: pending, cancellationAvailable, separateStop };
  const primary = composerSubmissionPolicy(facts);
  useEffect(() => { stopSequence.reset(); }, [stopSequence, binding, cancellationScope?.authority, cancellationScope?.identity, active, cancellationAvailable, menu]);
  useEffect(() => {
    // Window blur only resets; keyboard recognition stays on the focused editor.
    window.addEventListener('blur', stopSequence.reset);
    return () => { window.removeEventListener('blur', stopSequence.reset); stopSequence.reset(); };
  }, [stopSequence]);
  const invoke = (id: MenuAction) => {
    if (disabled || busy) return;
    if (id === 'file') { trigger.dismiss(); trigger.restore(); picker.current?.click(); return; }
    if (files.length || restored.length) { setError(message('agent:copy.remove-draft-attachments-before-invoking-a-command')); return; }
    const definition = commands.find(command => command.id === id)!;
    if (!onCommand || !available(definition, active, hasGoal, lineageSwitchSafe) || commandAvailable?.(id) === false) { setError(message('agent:copy.command-unavailable-in-the-current-session-state')); return; }
    if (id === 'model' && modelPicker) { modelInvocation.current = trigger.state?.source === 'launcher' ? undefined : editor; setModelRequested(true); trigger.dismiss(); return; }
    if (editor && trigger.state?.source !== 'launcher') invocation.current = { id, incarnation: editor.incarnation, revision: editor.revision };
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
    if (!editor) return;
    const ticket = owner.submit(editor.incarnation, intake, action.delivery, submissionTarget);
    if (!ticket) return;
    try {
      const accepted = await (onDraftSend ? onDraftSend(ticket.text, files.map(file => file.file!), files.map(file => file.id), ticket) : onSend(ticket.text, ticket.receipts, action.delivery, ticket.accepted));
      if (accepted) ticket.accepted(); else ticket.failed('unsent');
    } catch (cause) {
      ticket.failed(isOutcomeUncertain(cause) ? 'uncertain' : cause instanceof RpcFailure ? 'rejected' : 'unsent');
      if (owner.snapshot()?.incarnation === ticket.incarnation) setError(String(cause));
    }
  };
  return <div ref={root} className={css.root} data-file-drop-owner="">
    {uploadNotice && <Toast key={noticeSequence} text={uploadNotice} icon={<IconWarningOutline16/>} anchor={root.current} onDone={() => setUploadNotice('')}/>}
    {error && <p role="alert">{error}</p>}
    {!restoreSupported && <p role="alert">{tx('agent:agent-composer.cannot-restore-this-ordered-native-input-in-the-flat-web-editor')}</p>}
    <div className={css.card} data-composer-card data-editor-incarnation={editor?.incarnation} data-editor-revision={editor?.revision} data-submission-outcome={editor?.submission?.outcome} aria-busy={busy}>
      {modelMenu && <ModelPicker state={modelPicker!} close={() => { modelInvocation.current = undefined; setModelRequested(false); trigger.dismiss(); trigger.restore(); }} chosen={() => { if (modelRevision) owner.consumeRevision(modelRevision.incarnation, modelRevision.revision); setModelRequested(false); trigger.dismiss(); modelInvocation.current = undefined; trigger.restore(); }}/>}
      {menu && <CommandMenu rows={rows} active={highlight} select={selectMenu} highlight={trigger.highlight}
        fileDescription={uploadPolicy ? undefined : tx('agent:upload.policy')} />}
      {restored.map((receipt, index) => <div key={JSON.stringify([receipt.batch_id, receipt.token])}><span>{tx('agent:agent-composer.native-restored-upload-batch')}{' '}{receipt.batch_id}</span><Button disabled={disabled || busy} onClick={() => editor && owner.removeRestored(editor.incarnation, index)}>{tx('agent:agent-composer.remove-draft-upload')}</Button></div>)}
      <ComposerAttachments dropOwner={root} files={files} canAcceptDrop={!disabled && !busy && !!editor && !!uploadPolicy} onAddFiles={pick}
        onRemove={firstSubmission && onRetainedRemove ? onRetainedRemove : disabled || busy ? undefined : id => { intake.remove(id); if (editor) owner.attachmentsEdited(editor.incarnation); }}
        onRetry={!onRetainedRecover && (disabled || busy) ? undefined : id => { if (onRetainedRecover) onRetainedRecover(true); else void intake.upload(id, port); }}
        onReconcile={!onRetainedRecover && (disabled || busy) ? undefined : id => { if (onRetainedRecover) onRetainedRecover(false); else void intake.reconcile(id, port); }}/>
      <div className={css.editor}>
        <textarea ref={input} className={css.input} aria-label={messageLabel ?? tx('agent:agent-composer.message')} placeholder={placeholder ?? (onDraftSend ? tx('agent:agent-composer.describe-what-you-want-to-do') : tx('agent:agent-composer.give-this-session-a-task'))}
          aria-controls={menu ? 'composer-commands' : undefined} aria-activedescendant={menu && rows[highlight] ? `command-${rows[highlight].id}` : undefined}
          value={draft} disabled={disabled && !cancellationEditor} readOnly={busy || cancellationEditor} rows={1} onChange={event => { if (cancellationEditor) return; setDraft(event.target.value); trigger.track(event.target.value); setError(''); }}
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
          onSelect={rememberSelection}
          onBlur={() => { rememberSelection(); composing.current = false; stopSequence.reset(); }}
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
              if (!cancellationAvailable || !cancellationScope || document.activeElement !== event.currentTarget) { stopSequence.reset(); return; }
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
      <div ref={row} className={css.row} data-composer-controls>
        <div className={css.tools}>
          <button type="button" className={css.add} aria-label={tx('commands:menu.add')} title={tx('commands:menu.add')} aria-haspopup="listbox" aria-expanded={!!menu} disabled={disabled || busy} onMouseDown={event => event.preventDefault()} onClick={trigger.toggle}><span aria-hidden="true"><IconPlusOutline16 /></span></button>
          <input ref={picker} type="file" hidden multiple aria-label={tx('agent:agent-composer.attach-files')} disabled={disabled || busy} onChange={event => { pick(Array.from(event.target.files ?? []).map(file => ({ file }))); event.target.value = ''; }}/>
          {permission}
        </div>
        <div className={css.trailing}>
          {model}
          {separateStop && active && <button type="button" className={css.primary} data-composer-stop aria-label={tx('agent:submission.stop')} title={tx('agent:submission.stop-double-escape')}
            disabled={!cancellationAvailable} onMouseDown={event => event.preventDefault()} onClick={onCancel}>
            <svg viewBox="0 0 16 16" width="16" height="16" aria-hidden><rect x="3" y="3" width="10" height="10" rx="3" fill="currentColor"/></svg>
          </button>}
          <button type="button" className={css.primary} data-composer-primary={primary.kind} aria-label={tx(primary.label)} title={tx(primary.title)}
            disabled={primary.disabled} onMouseDown={event => event.preventDefault()} onClick={() => { if (primary.kind === 'stop') onCancel(); else void submit(); }}>
            {primary.kind === 'stop'
              ? <svg viewBox="0 0 16 16" width="16" height="16" aria-hidden><rect x="3" y="3" width="10" height="10" rx="3" fill="currentColor"/></svg>
              : <span aria-hidden="true"><IconSendOutline14 /></span>}
          </button>
        </div>
      </div>

    </div>
  </div>;
}
