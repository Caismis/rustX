import { useTranslation } from '../../locale/react';
/* Copyright (c) 2026 DeepSeek. MIT. Rewritten from ui-attachment/MessageImage.tsx and FileCard.tsx; see PROVENANCE.md. */
import { useState, type ReactNode } from 'react';
import { Modal } from '../primitives/Modal';
import { IconBrowseOutline16, IconCloseOutline16, IconDownloadOutline16, IconRefreshOutline16 } from '../primitives/icons';
import { StateDot } from '../primitives/StateDot';
import css from './AttachmentCard.module.css';
/** URLs, transfer status and removal are supplied by the resource/draft owner. */
export function AttachmentCard({ name, image, url, error, loading, onLoad, onOpen, onRemove, onDecodeError, onDownload, description, mimeType, actions }: {
  name: string; image: boolean; url?: string; error?: string; loading?: boolean;
  description?: string | null; mimeType?: string; actions?: ReactNode;
  onLoad?: () => void; onOpen?: () => void; onRemove?: () => void; onDecodeError?: () => void; onDownload?: () => void;
}) {
  const tx = useTranslation();
  const [open, setOpen] = useState(false);
  const content = <><strong>{name}</strong><small>{error ?? mimeType ?? (image ? tx('artifacts:attachment-card.image-attachment') : tx('artifacts:attachment-card.file-attachment'))}</small>
    {description && <span className={css.description}>{description}</span>}</>;
  const previewLabel = `${tx('artifacts:artifact.preview')} ${name}`;
  return <div className={css.card}>
    {image && (url && !error || onLoad ? <button type="button" className={css.thumbnail} disabled={loading} aria-busy={loading || undefined}
      aria-label={url && !error ? tx('artifacts:attachment-card.open-image-value', { p0: name }) : `${tx(error ? 'artifacts:attachment-card.retry' : 'artifacts:attachment-card.load-attachment')} ${name}`}
      onClick={url && !error ? onOpen ?? (() => setOpen(true)) : onLoad}>
      {url && !error ? <img src={url} alt={name} onError={onDecodeError}/> : loading ? <StateDot state="ongoing"/> : error ? <IconRefreshOutline16/> : <IconBrowseOutline16/>}
    </button> : <span className={css.thumbnail} aria-hidden><IconBrowseOutline16/></span>)}
    {onOpen ? <button type="button" className={`${css.file} ${css.fileButton}`} onClick={onOpen} aria-label={previewLabel} title={name}>{content}</button>
      : <div className={css.file}>{content}
      {!onDownload && !image && url && <a href={url} download={name}>{tx('artifacts:attachment-card.download')}</a>}
    </div>}
    {actions && <div className={css.actions}>{actions}</div>}
    {onDownload && <button type="button" className={css.action} aria-label={`${tx('artifacts:attachment-card.download')} ${name}`} title={tx('artifacts:attachment-card.download')} onClick={onDownload}><IconDownloadOutline16/></button>}
    {onRemove && <button type="button" className={css.action} aria-label={tx('artifacts:attachment-card.remove-value', { p0: name })} onClick={onRemove}><IconCloseOutline16/></button>}
    {open && url && !error && <Modal closeLabel={tx('artifacts:attachment-card.close-dialog')} open title={name} onClose={() => setOpen(false)}><img className={css.original} src={url} alt={name} onError={onDecodeError} /></Modal>}
  </div>;
}
