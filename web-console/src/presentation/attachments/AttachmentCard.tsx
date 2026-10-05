import { useTranslation } from '../../locale/react';
/* Copyright (c) 2026 DeepSeek. MIT. Rewritten from ui-attachment/MessageImage.tsx and FileCard.tsx; see PROVENANCE.md. */
import { useState, type ReactNode } from 'react';
import { Modal } from '../primitives/Modal';
import { Button } from '../primitives/Button';
import css from './AttachmentCard.module.css';
/** URLs, transfer status and removal are supplied by the resource/draft owner. */
export function AttachmentCard({ name, image, url, error, loading, onLoad, onRemove, onDecodeError, onDownload, description, mimeType, actions }: {
  name: string; image: boolean; url?: string; error?: string; loading?: boolean;
  description?: string | null; mimeType?: string; actions?: ReactNode;
  onLoad?: () => void; onRemove?: () => void; onDecodeError?: () => void; onDownload?: () => void;
}) {
  const tx = useTranslation();
  const [open, setOpen] = useState(false);
  return <div className={css.card}>
    {image && url && !error ? <button className={css.thumbnail} aria-label={tx('artifacts:attachment-card.open-image-value', { p0: name })} onClick={() => setOpen(true)}>
      <img src={url} alt={name} onError={onDecodeError} />
    </button> : <div className={css.file}><strong>{name}</strong><small>{error ?? mimeType ?? (loading ? tx('artifacts:attachment-card.loading') : image ? tx('artifacts:attachment-card.image-attachment') : tx('artifacts:attachment-card.file-attachment'))}</small>
      {description && <p className={css.description}>{description}</p>}
      {actions && <div className={css.actions}>{actions}</div>}
      {onLoad && !loading && <Button size="sm" onClick={onLoad}>{error ? tx('artifacts:attachment-card.retry') : tx('artifacts:attachment-card.load-attachment')}</Button>}
      {!onDownload && !image && url && <a href={url} download={name}>{tx('artifacts:attachment-card.download')}</a>}
    </div>}
    {onDownload && <Button size="sm" aria-label={`${tx('artifacts:attachment-card.download')} ${name}`} onClick={onDownload}>{tx('artifacts:attachment-card.download')}</Button>}
    {onRemove && <Button size="sm" aria-label={tx('artifacts:attachment-card.remove-value', { p0: name })} onClick={onRemove}>×</Button>}
    {open && url && !error && <Modal closeLabel={tx('artifacts:attachment-card.close-dialog')} open title={name} onClose={() => setOpen(false)}><img className={css.original} src={url} alt={name} onError={onDecodeError} /></Modal>}
  </div>;
}
