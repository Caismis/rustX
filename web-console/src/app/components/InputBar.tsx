/* Copyright (c) 2026 DeepSeek. MIT. Source-derived; see PROVENANCE.md. */
// Presentation extracted from DeepSeek Harness ui-conversation/InputBar.
// Native textarea replaces Lexical/attachment/command/queue machines. No retry.
// The delivery label follows the authoritative attempt; rustX has one native
// inbound path, so there is no separate browser queue/steer mode.
import { useEffect, useState, useRef } from 'react';
import { UPLOAD_MAX_BYTES, UPLOAD_BATCH_MAX_BYTES, DRAFT_MAX_FILES } from '../../client/uploads';
import type { UploadReceipt, UploadedFile } from '../../../../protocol/app-server/v4';
import { isOutcomeUncertain } from '../../client/app-server';
import { AttachmentCard } from '../../presentation/attachments/AttachmentCard';
import { Button } from '../../presentation/primitives/Button';
import css from './InputBar.module.css';
export function InputBar({ disabled, busy, active, onSend, onUpload, onCancel }: {
  disabled: boolean; busy: boolean; active: boolean;
  onSend: (text: string, receipts: readonly UploadReceipt[]) => Promise<boolean>;
  onUpload: (files: readonly File[]) => Promise<UploadedFile[]>; onCancel: () => void;
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
  const [draft, setDraft] = useState('');
  const submit = async () => {
    if (disabled || busy || pending || (!draft.trim() && !files.length)) return;
    const submitted = draft;
    if (await onSend(submitted, files.map(file => file.receipt!))) { setDraft(current => current === submitted ? '' : current); setFiles([]); }
  };
  return <div className={css.root} onDragOver={event => { if (!disabled && !busy && event.dataTransfer.types.includes('Files')) { event.preventDefault(); setDragging(true); } }}
    onDragLeave={event => { if (!event.currentTarget.contains(event.relatedTarget as Node | null)) setDragging(false); }}
    onDrop={event => { event.preventDefault(); setDragging(false); if (!disabled && !busy) pick(Array.from(event.dataTransfer.files)); }}>
    {dragging && <div className="attachment-drop" role="status">Drop attachments · 8 files · 256 KiB each</div>}
    {error && <p role="alert">{error}</p>}
    <div className="attachment-rail" aria-label="Draft attachments">{files.map(item => <div key={item.id}><DraftAttachment file={item.file} remove={() => setFiles(current => current.filter(file => file.id !== item.id))} /><small role="status">{item.status === 'complete' ? 'Uploaded' : item.status === 'uploading' ? 'Uploading…' : item.status === 'uncertain' ? 'Upload outcome uncertain. Reconnect and inspect authoritative state; do not replay.' : item.error}</small></div>)}</div>
    <div className={css.card} data-composer-card>
      <div className={css.scroll}><div className={css.grow}>
        <textarea className={css.input} aria-label="Message" placeholder="Give this Session a task…"
          value={draft} disabled={disabled || busy} rows={3} onChange={event => setDraft(event.target.value)}
          onPaste={event => { const pasted = Array.from(event.clipboardData.files); if (pasted.length) { event.preventDefault(); pick(pasted); } }}
          onKeyDown={event => {
            if (event.key === 'Enter' && !event.shiftKey && !event.nativeEvent.isComposing && event.nativeEvent.keyCode !== 229) {
              event.preventDefault(); void submit();
            }
          }} />
      </div></div>
      <div className={css.row}>
        <label>Attach files<input type="file" multiple aria-label="Attach files" disabled={disabled || busy} onChange={event => { pick(Array.from(event.target.files ?? [])); event.target.value = ''; }} /></label>
        <span className="muted" data-delivery={active ? 'queue' : 'send'}>{active ? 'Attempt running · Enter queues for its next safe boundary' : 'Enter to send · Shift+Enter for newline'}</span>
        <div className={css.trailing}>
          {active && <Button size="sm" variant="outline" disabled={disabled || busy} onClick={onCancel}>Cancel turn</Button>}
          <Button variant="primary" disabled={disabled || busy || pending || (!draft.trim() && !files.length)} onClick={() => void submit()}>{busy ? 'Awaiting acknowledgement…' : active ? 'Queue' : 'Send'}</Button>
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
