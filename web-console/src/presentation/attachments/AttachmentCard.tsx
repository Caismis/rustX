import { useTranslation } from '../../locale/react';
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
  const tx = useTranslation();
  const [open, setOpen] = useState(false);
  return <div className={css.card}>
    {image && url && !error ? <button className={css.thumbnail} aria-label={tx('artifacts:attachment-card.open-image-value', { p0: name })} onClick={() => setOpen(true)}>
      <img src={url} alt={name} onError={onDecodeError} />
    </button> : <div className={css.file}><strong>{name}</strong><small>{error ?? (loading ? tx('artifacts:attachment-card.loading') : image ? tx('artifacts:attachment-card.image-attachment') : tx('artifacts:attachment-card.file-attachment'))}</small>
      {onLoad && !loading && <Button size="sm" onClick={onLoad}>{error ? tx('artifacts:attachment-card.retry') : tx('artifacts:attachment-card.load-attachment')}</Button>}
      {!image && url && <a href={url} download={name}>{tx('artifacts:attachment-card.download')}</a>}
    </div>}
    {onRemove && <Button size="sm" aria-label={tx('artifacts:attachment-card.remove-value', { p0: name })} onClick={onRemove}>×</Button>}
    {open && url && !error && <Modal closeLabel={tx('artifacts:attachment-card.close-dialog')} open title={name} onClose={() => setOpen(false)}><img className={css.original} src={url} alt={name} onError={onDecodeError} /></Modal>}
  </div>;
}
