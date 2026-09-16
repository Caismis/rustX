/* Copyright (c) 2026 DeepSeek. MIT. Source-derived; see PROVENANCE.md. */
// Presentation extracted from DeepSeek Harness ui-conversation/InputBar.
// Native textarea replaces Lexical. Commands are client grammar; effects are typed.
import { useEffect, useState, useRef } from 'react';
import { UPLOAD_MAX_BYTES, UPLOAD_BATCH_MAX_BYTES, DRAFT_MAX_FILES } from '../../client/uploads';
import type { UploadReceipt, UploadedFile, UserInputBlock } from '../../../../protocol/app-server/v4';
import { commands, available, discoveryQuery, parseCommand, type CommandId } from '../commands/registry';
import { matchCommands } from '../commands/matching';
import { CommandMenu } from '../commands/CommandMenu';
import { isOutcomeUncertain } from '../../client/app-server';
import { AttachmentCard } from '../../presentation/attachments/AttachmentCard';
import { Button } from '../../presentation/primitives/Button';
import css from './InputBar.module.css';
export function InputBar({ disabled, busy, active, onSend, onUpload, onCancel, onCommand, hasGoal = false, initialContent = [], consumed }: {
  disabled: boolean; busy: boolean; active: boolean;
  onSend: (text: string, receipts: readonly UploadReceipt[], delivery: 'send' | 'steer') => Promise<boolean>;
  onUpload: (files: readonly File[]) => Promise<UploadedFile[]>; onCancel: () => void;
  onCommand?: (id: CommandId) => void; hasGoal?: boolean; initialContent?: UserInputBlock[];
  consumed?: { id: string; sequence: number };
}) {
  type DraftFile = { id: number; file: File; status: 'uploading' | 'complete' | 'failed' | 'uncertain'; receipt?: UploadReceipt; error?: string };
  const [files, setFiles] = useState<DraftFile[]>([]);
  const nextId = useRef(0);
  const transferring = useRef(false);
  const [error, setError] = useState('');
  const [dragging, setDragging] = useState(false);
  const pending = files.some(file => file.status !== 'complete');
  const pick = (picked: File[]) => {
    if (!picked.length) return;
    if (transferring.current) { setError('Additional files were not added. Wait for the current upload to finish, then select them again.'); return; }
    if (files.length + picked.length > DRAFT_MAX_FILES || picked.some(file => file.size > UPLOAD_MAX_BYTES)
      || picked.reduce((sum, file) => sum + file.size, 0) > UPLOAD_BATCH_MAX_BYTES) { setError('Choose at most 8 files, 256 KiB each and 512 KiB per batch.'); return; }
    const batch: DraftFile[] = picked.map(file => ({ id: nextId.current++, file, status: 'uploading' }));
    transferring.current = true;
    setError(''); setFiles(current => [...current, ...batch]);
    void onUpload(picked).then(uploaded => {
      if (uploaded.length !== batch.length) throw new Error('Invalid upload response; outcome uncertain.');
      setFiles(current => current.map(file => {
        const index = batch.findIndex(item => item.id === file.id);
        return index < 0 ? file : { ...file, status: 'complete', receipt: uploaded[index].receipt };
      }));
    }).catch(cause => {
      setFiles(current => current.map(file => batch.some(item => item.id === file.id)
        ? { ...file, status: isOutcomeUncertain(cause) ? 'uncertain' : 'failed', error: String(cause) } : file));
    }).finally(() => { transferring.current = false; });
  };
  const [draft, setDraft] = useState(() => initialContent.flatMap(block => block.type === 'text' ? [block.text] : []).join(''));
  useEffect(() => {
    if (!consumed) return;
    setDraft(current => { const parsed = parseCommand(current); return parsed.type === 'command' && parsed.id === consumed.id ? '' : current; });
  }, [consumed]);
  const [restored, setRestored] = useState(() => initialContent.flatMap(block => block.type === 'upload' ? [block] : []));
  const [dismissed, setDismissed] = useState(false), [highlight, setHighlight] = useState(0);
  const [delivery, setDelivery] = useState<'send' | 'steer'>('send');
  const input = useRef<HTMLTextAreaElement>(null), root = useRef<HTMLDivElement>(null);
  const query = discoveryQuery(draft);
  const menu = onCommand && query !== undefined && !dismissed && !disabled && !busy;
  const rows = matchCommands(query ?? '', commands.filter(command => available(command, active, hasGoal)));
  const invoke = (id: CommandId) => {
    if (disabled || busy) return;
    if (files.length || restored.length) { setError('Remove draft attachments before invoking a command.'); return; }
    const definition = commands.find(command => command.id === id)!;
    if (!onCommand || !available(definition, active, hasGoal)) { setError('Command unavailable in the current Session state.'); return; }
    setDismissed(true); setError(''); input.current?.focus(); onCommand(id);
  };
  useEffect(() => {
    if (!menu) return;
    const outside = (event: PointerEvent) => { if (!root.current?.contains(event.target as Node)) setDismissed(true); };
    document.addEventListener('pointerdown', outside);
    return () => document.removeEventListener('pointerdown', outside);
  }, [menu]);
  const submit = async () => {
    if (disabled || busy || pending || (!draft.trim() && !files.length && !restored.length)) return;
    const parsed = parseCommand(draft);
    if (parsed.type !== 'text') {
      if (parsed.type === 'command') invoke(parsed.id);
      else setError('Unsupported command. Edit the draft; it will not be sent as a prompt.');
      return;
    }
    const submitted = draft;
    if (await onSend(submitted, [...restored, ...files.map(file => file.receipt!)], active ? delivery : 'send')) { setDraft(current => current === submitted ? '' : current); setFiles([]); setRestored([]); }
  };
  return <div ref={root} className={css.root} onDragOver={event => { if (!disabled && !busy && event.dataTransfer.types.includes('Files')) { event.preventDefault(); setDragging(true); } }}
    onDragLeave={event => { if (!event.currentTarget.contains(event.relatedTarget as Node | null)) setDragging(false); }}
    onDrop={event => { event.preventDefault(); setDragging(false); if (!disabled && !busy) pick(Array.from(event.dataTransfer.files)); }}>
    {dragging && <div className="attachment-drop" role="status">Drop attachments · 8 files · 256 KiB each</div>}
    {error && <p role="alert">{error}</p>}
    {restored.map((receipt, index) => <div key={receipt.batch_id}><span>Native restored upload batch {receipt.batch_id}</span><Button onClick={() => setRestored(current => current.filter((_, at) => at !== index))}>Remove draft upload</Button></div>)}
    <div className="attachment-rail" aria-label="Draft attachments">{files.map(item => <div key={item.id}><DraftAttachment file={item.file} remove={() => setFiles(current => current.filter(file => file.id !== item.id))} /><small role="status">{item.status === 'complete' ? 'Uploaded' : item.status === 'uploading' ? 'Uploading…' : item.status === 'uncertain' ? 'Upload outcome uncertain. Reconnect and inspect authoritative state; do not replay.' : item.error}</small></div>)}</div>
    <div className={css.card} data-composer-card>
      {menu && <CommandMenu rows={rows} active={highlight} select={invoke} highlight={setHighlight} />}
      <div className={css.scroll}><div className={css.grow}>
        <textarea ref={input} className={css.input} aria-label="Message" placeholder="Give this Session a task…"
          aria-controls={menu ? 'composer-commands' : undefined} aria-expanded={!!menu} aria-activedescendant={menu && rows[highlight] ? `command-${rows[highlight].id}` : undefined}
          value={draft} disabled={disabled || busy} rows={3} onChange={event => { setDraft(event.target.value); setDismissed(false); setHighlight(0); setError(''); }}
          onPaste={event => { const pasted = Array.from(event.clipboardData.files); if (pasted.length) { event.preventDefault(); pick(pasted); } }}
          onKeyDown={event => {
            if (event.nativeEvent.isComposing || event.nativeEvent.keyCode === 229) return;
            if (menu && event.key === 'Escape') { event.preventDefault(); setDismissed(true); return; }
            if (menu && rows.length && (event.key === 'ArrowDown' || event.key === 'ArrowUp')) { event.preventDefault(); setHighlight(index => (index + (event.key === 'ArrowDown' ? 1 : rows.length - 1)) % rows.length); return; }
            if (menu && rows[highlight] && event.key === 'Enter' && !event.shiftKey) { event.preventDefault(); invoke(rows[highlight].id); return; }
            if (event.key === 'Enter' && !event.shiftKey && !event.nativeEvent.isComposing && event.nativeEvent.keyCode !== 229) {
              event.preventDefault(); void submit();
            }
          }} />
      </div></div>
      <div className={css.row}>
        {onCommand && <Button aria-label="Commands" disabled={disabled || busy} onClick={() => { if (draft.trim() && discoveryQuery(draft) === undefined) { setError('Type / in an empty composer to discover commands. Your draft is preserved.'); return; } setDraft('/'); setDismissed(false); setHighlight(0); input.current?.focus(); }}>+</Button>}
        <label>Attach files<input type="file" multiple aria-label="Attach files" disabled={disabled || busy} onChange={event => { pick(Array.from(event.target.files ?? [])); event.target.value = ''; }} /></label>
        <span className="muted" data-delivery={active ? delivery === 'steer' ? 'steer' : 'queue' : 'send'}>{active ? 'Attempt running · Queue and Steer both enter the native mailbox at a safe boundary' : 'Enter to send · Shift+Enter for newline'}</span>
        {active && <label>Delivery<select aria-label="Delivery" disabled={disabled || busy} value={delivery} onChange={event => setDelivery(event.target.value as 'send' | 'steer')}><option value="send">Queue</option><option value="steer">Steer (same mailbox)</option></select></label>}
        <div className={css.trailing}>
          {active && <Button size="sm" variant="outline" disabled={disabled || busy} onClick={onCancel}>Cancel turn</Button>}
          <Button variant="primary" disabled={disabled || busy || pending || (!draft.trim() && !files.length && !restored.length)} onClick={() => void submit()}>{busy ? 'Awaiting acknowledgement…' : active ? delivery === 'steer' ? 'Steer' : 'Queue' : 'Send'}</Button>
        </div>
      </div>
    </div>
  </div>;
}

function DraftAttachment({ file, remove }: { file: File; remove: () => void }) {
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
