import { useTranslation } from '../../locale/react';
/* Copyright (c) 2026 DeepSeek. MIT. Rewritten from ui-attachment/MessageImage.tsx and FileCard.tsx; see PROVENANCE.md. */
import { useCallback, useState, type ReactNode } from 'react';
import { Modal } from '../primitives/Modal';
import { ImageLightbox } from '../primitives/ImageLightbox';
import { IconBrowseOutline16, IconCloseOutline16, IconDownloadOutline16, IconRefreshOutline16 } from '../primitives/icons';
import { StateDot } from '../primitives/StateDot';
/** URLs, transfer status and removal are supplied by the resource/draft owner. */
export function AttachmentCard({ name, image, url, error, loading, onLoad, onOpen, onRemove, onDecodeError, onDownload, description, mimeType, actions }: {
 name: string; image: boolean; url?: string; error?: string; loading?: boolean;
 description?: string | null; mimeType?: string; actions?: ReactNode;
 onLoad?: () => void; onOpen?: () => void; onRemove?: () => void; onDecodeError?: () => void; onDownload?: () => void;
}) {
 const tx = useTranslation();
 const [open, setOpen] = useState(false);
 const content = <><strong>{name}</strong><small>{error ?? mimeType ?? (image ? tx('artifacts:attachment-card.image-attachment') : tx('artifacts:attachment-card.file-attachment'))}</small>
 {description && <span className="m-0 wrap-anywhere text-[13px]">{description}</span>}</>;
 const previewLabel = `${tx('artifacts:artifact.preview')} ${name}`;
 return <div className="flex flex-none items-center gap-[6px] max-w-full [border:0.5px_solid_var(--dsw-alias-border-l2)] rounded-[16px] overflow-hidden">
 {image && (url && !error || onLoad ? <button type="button" className="grid place-items-center flex-none w-[64px] h-[64px] border-0 p-0 [color:var(--dsw-alias-label-tertiary)] bg-[var(--dsw-alias-bg-l1)] cursor-zoom-in disabled:cursor-default [&[aria-hidden]]:cursor-default [&_img]:w-full [&_img]:h-full [&_img]:object-cover focus-visible:outline-2 focus-visible:outline-[var(--dsw-alias-state-business-primary)] focus-visible:outline-offset-[-2px]" disabled={loading} aria-busy={loading || undefined}
 aria-label={url && !error ? tx('artifacts:attachment-card.open-image-value', { p0: name }) : `${tx(error ? 'artifacts:attachment-card.retry' : 'artifacts:attachment-card.load-attachment')} ${name}`}
 onClick={url && !error ? onOpen ?? (() => setOpen(true)) : onLoad}>
 {url && !error ? <img src={url} alt={name} onError={onDecodeError}/> : loading ? <StateDot state="ongoing"/> : error ? <IconRefreshOutline16/> : <IconBrowseOutline16/>}
 </button> : <span className="grid place-items-center flex-none w-[64px] h-[64px] border-0 p-0 [color:var(--dsw-alias-label-tertiary)] bg-[var(--dsw-alias-bg-l1)] cursor-zoom-in disabled:cursor-default [&[aria-hidden]]:cursor-default [&_img]:w-full [&_img]:h-full [&_img]:object-cover focus-visible:outline-2 focus-visible:outline-[var(--dsw-alias-state-business-primary)] focus-visible:outline-offset-[-2px]" aria-hidden><IconBrowseOutline16/></span>)}
 {onOpen ? <button type="button" className={`flex flex-col justify-center gap-[4px] flex-1 min-w-0 w-[240px] min-h-[64px] p-[10px] box-border border-0 bg-transparent [color:inherit] text-left cursor-pointer hover:bg-[var(--dsw-alias-interactive-bg-hover)] focus-visible:outline-2 focus-visible:outline-[var(--dsw-alias-state-business-primary)] focus-visible:outline-offset-[-2px] [&_strong]:overflow-hidden [&_strong]:text-ellipsis [&_strong]:whitespace-nowrap [&_small]:[color:var(--dsw-alias-label-tertiary)]`} onClick={onOpen} aria-label={previewLabel} title={name}>{content}</button>
 : <div className="flex flex-col justify-center gap-[4px] flex-1 min-w-0 w-[240px] min-h-[64px] p-[10px] box-border [&_strong]:overflow-hidden [&_strong]:text-ellipsis [&_strong]:whitespace-nowrap [&_small]:[color:var(--dsw-alias-label-tertiary)]">{content}
 {!onDownload && !image && url && <a href={url} download={name}>{tx('artifacts:attachment-card.download')}</a>}
 </div>}
 {actions && <div className="flex gap-[4px] flex-wrap">{actions}</div>}
 {onDownload && <button type="button" className="grid flex-none place-items-center w-[28px] h-[28px] mr-[6px] p-0 border-0 rounded-[6px] bg-transparent [color:var(--dsw-alias-label-tertiary)] cursor-pointer hover:bg-[var(--dsw-alias-interactive-bg-hover)] focus-visible:outline-2 focus-visible:outline-[var(--dsw-alias-state-business-primary)] focus-visible:outline-offset-[-2px]" aria-label={`${tx('artifacts:attachment-card.download')} ${name}`} title={tx('artifacts:attachment-card.download')} onClick={onDownload}><IconDownloadOutline16/></button>}
 {onRemove && <button type="button" className="grid flex-none place-items-center w-[28px] h-[28px] mr-[6px] p-0 border-0 rounded-[6px] bg-transparent [color:var(--dsw-alias-label-tertiary)] cursor-pointer hover:bg-[var(--dsw-alias-interactive-bg-hover)] focus-visible:outline-2 focus-visible:outline-[var(--dsw-alias-state-business-primary)] focus-visible:outline-offset-[-2px]" aria-label={tx('artifacts:attachment-card.remove-value', { p0: name })} onClick={onRemove}><IconCloseOutline16/></button>}
 {open && url && !error && <Modal closeLabel={tx('artifacts:attachment-card.close-dialog')} open title={name} onClose={() => setOpen(false)}><img className="block max-w-full max-h-[75vh] object-contain" src={url} alt={name} onError={onDecodeError} /></Modal>}
 </div>;
}

/** Inline gallery image. Its owner supplies the authorized, occurrence-scoped URL. */
export function InlineImage({ name, url, compact, error, retry, onDecodeError }: {
 name: string; url?: string; compact: boolean; error?: string; retry?: () => void; onDecodeError: () => void;
}) {
 const tx = useTranslation();
 const [open, setOpen] = useState(false);
 const close = useCallback(() => setOpen(false), []);
 const label = url ? tx('artifacts:attachment-card.open-image-value', { p0: name })
 : error ? `${tx('artifacts:attachment-card.retry')} ${name}` : tx('common:activity.loading');
 return <>
 <button type="button" className="grid place-items-center w-fit max-w-[min(240px,100%)] min-w-[44px] min-h-[44px] p-0 overflow-hidden [border:0.5px_solid_var(--dsw-alias-border-l2)] rounded-[var(--dsw-radius-xl)] bg-[var(--dsw-alias-interactive-bg-hover)] [color:var(--dsw-alias-label-tertiary)] cursor-pointer [&[data-compact]]:w-[48px] [&[data-compact]]:h-[48px] [&[data-compact]]:p-[2px] [&[aria-busy]]:cursor-default [&:has(img)]:cursor-zoom-in [&_img]:block [&_img]:max-w-full [&_img]:max-h-[240px] [&_img]:object-contain [&[data-compact]_img]:w-full [&[data-compact]_img]:h-full focus-visible:outline-2 focus-visible:outline-[var(--dsw-alias-state-business-primary)] focus-visible:outline-offset-2" data-compact={compact || undefined}
 aria-label={label} title={error || name} aria-busy={!url && !error || undefined}
 disabled={!url && (!error || !retry)} onClick={url ? () => setOpen(true) : retry}>
 {url ? <img src={url} alt={name} onError={onDecodeError}/> : error ? <IconRefreshOutline16/> : <StateDot state="ongoing"/>}
 </button>
 {open && url && <ImageLightbox src={url} alt={name} labels={{ dialog: name, close: tx('artifacts:attachment-card.close-dialog') }} onClose={close}/>}
 </>;
}
