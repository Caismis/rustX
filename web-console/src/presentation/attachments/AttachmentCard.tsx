/* Copyright (c) 2026 DeepSeek. MIT. Rewritten from ui-attachment/MessageImage.tsx and FileCard.tsx; see PROVENANCE.md. */
import { useState } from 'react';
import { Modal } from '../primitives/Modal';
import { Button } from '../primitives/Button';
import css from './AttachmentCard.module.css';
/** URLs, transfer status and removal are supplied by the resource/draft owner. */
export function AttachmentCard({ name, image, url, error, loading, onLoad, onRemove, onDecodeError }: {
  name: string; image: boolean; url?: string; error?: string; loading?: boolean;
  onLoad?: () => void; onRemove?: () => void; onDecodeError?: () => void;
}) {
  const [open, setOpen] = useState(false);
  return <div className={css.card}>
    {image && url && !error ? <button className={css.thumbnail} aria-label={`Open image ${name}`} onClick={() => setOpen(true)}>
      <img src={url} alt={name} onError={onDecodeError} />
    </button> : <div className={css.file}><strong>{name}</strong><small>{error ?? (loading ? 'Loading…' : image ? 'Image attachment' : 'File attachment')}</small>
      {onLoad && !loading && <Button size="sm" onClick={onLoad}>{error ? 'Retry' : 'Load attachment'}</Button>}
      {!image && url && <a href={url} download={name}>Download</a>}
    </div>}
    {onRemove && <Button size="sm" aria-label={`Remove ${name}`} onClick={onRemove}>×</Button>}
    {open && url && !error && <Modal closeLabel="Close dialog" open title={name} onClose={() => setOpen(false)}><img className={css.original} src={url} alt={name} onError={onDecodeError} /></Modal>}
  </div>;
}
