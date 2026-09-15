/* Copyright (c) 2026 DeepSeek. MIT. Source-derived; see PROVENANCE.md. */
// Presentation extracted from DeepSeek Harness ui-conversation/InputBar.
// Native textarea replaces Lexical/attachment/command/queue machines. No retry.
// The delivery label follows the authoritative attempt; rustX has one native
// inbound path, so there is no separate browser queue/steer mode.
import { useEffect, useState } from 'react';
import { ARTIFACT_MAX_BYTES, DRAFT_MAX_FILES } from '../../client/artifacts';
import { AttachmentCard } from '../../presentation/attachments/AttachmentCard';
import { Button } from '../../presentation/primitives/Button';
import css from './InputBar.module.css';
export function InputBar({ disabled, busy, active, onSend, onCancel }: {
  disabled: boolean; busy: boolean; active: boolean;
  onSend: (text: string, files: readonly File[]) => Promise<boolean>; onCancel: () => void;
}) {
  const [files, setFiles] = useState<File[]>([]);
  const [error, setError] = useState('');
  const [dragging, setDragging] = useState(false);
  const pick = (picked: File[]) => {
    if (files.length + picked.length > DRAFT_MAX_FILES || picked.some(file => file.size > ARTIFACT_MAX_BYTES)) { setError('Choose at most 8 files, each at most 256 KiB.'); return; }
    setError(''); setFiles(current => [...current, ...picked]);
  };
  const [draft, setDraft] = useState('');
  const submit = async () => {
    if (disabled || busy || (!draft.trim() && !files.length)) return;
    const submitted = draft;
    if (await onSend(submitted, files)) { setDraft(current => current === submitted ? '' : current); setFiles([]); }
  };
  return <div className={css.root} onDragOver={event => { if (!disabled && !busy && event.dataTransfer.types.includes('Files')) { event.preventDefault(); setDragging(true); } }}
    onDragLeave={event => { if (!event.currentTarget.contains(event.relatedTarget as Node | null)) setDragging(false); }}
    onDrop={event => { event.preventDefault(); setDragging(false); if (!disabled && !busy) pick(Array.from(event.dataTransfer.files)); }}>
    {dragging && <div className="attachment-drop" role="status">Drop attachments · 8 files · 256 KiB each</div>}
    {error && <p role="alert">{error}</p>}
    <div className="attachment-rail" aria-label="Draft attachments">{files.map((file, index) => <DraftAttachment key={index} file={file} remove={() => setFiles(current => current.filter((_, i) => i !== index))} />)}</div>
    <div className={css.card} data-composer-card>
      <div className={css.scroll}><div className={css.grow}>
        <textarea className={css.input} aria-label="Message" placeholder="Give this Session a task…"
          value={draft} disabled={disabled || busy} rows={3} onChange={event => setDraft(event.target.value)}
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
          <Button variant="primary" disabled={disabled || busy || (!draft.trim() && !files.length)} onClick={() => void submit()}>{busy ? 'Awaiting acknowledgement…' : active ? 'Queue' : 'Send'}</Button>
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
