/* Copyright (c) 2026 DeepSeek. MIT. Adapted from ui-attachment/ComposerAttachments; see PROVENANCE.md. */
import { useCallback, useEffect, useRef, useState } from 'react';
import type { IntakeFile, IntakeInput } from '../../client/uploads';
import { useTranslation } from '../../locale/react';
import { AttachmentRail } from '../../presentation/attachments/AttachmentRail';
import { DropOverlay } from '../../presentation/attachments/DropOverlay';
import { FileCard } from '../../presentation/attachments/FileCard';
import { ImageLightbox } from '../../presentation/primitives/ImageLightbox';
import { IconCloseFill14 } from '../../presentation/primitives/icons';
import { installDocumentDropEvents } from './attachment-drop-events';
import css from '../../presentation/attachments/ComposerAttachments.module.css';

type Attachment = Pick<IntakeFile, 'id' | 'name' | 'file' | 'status'>;
/** DSH attachment presentation over the native draft/receipt owner. */
export function ComposerAttachments({ files, canAcceptDrop, onAddFiles, onRemove, onRetry, onReconcile, dropOwner }: {
  files: readonly Attachment[]; canAcceptDrop: boolean; onAddFiles: (inputs: IntakeInput[]) => void;
  dropOwner: { readonly current: HTMLElement | null };
  onRemove?: (id: string) => void; onRetry?: (id: string) => void; onReconcile?: (id: string) => void;
}) {
  const tx = useTranslation();
  const [dragActive, setDragActive] = useState(false);
  const depth = useRef(0);
  useEffect(() => installDocumentDropEvents(canAcceptDrop, onAddFiles, depth, setDragActive, dropOwner), [canAcceptDrop, onAddFiles, dropOwner]);
  return <>
    {dragActive && <DropOverlay disabled={!canAcceptDrop} labels={{ title: tx(canAcceptDrop ? 'agent:upload.drop' : 'agent:upload.dropBlocked') }}/>}
    {files.length > 0 && <div className={css.rail}>
      <AttachmentRail items={files} labels={{ group: tx('agent:agent-composer.draft-attachments'), scrollLeft: tx('agent:upload.scrollLeft'), scrollRight: tx('agent:upload.scrollRight') }} renderItem={item => {
        const uncertain = item.status === 'uncertain' || item.status === 'reconciling';
        const failed = item.status === 'failed';
        const loading = ['uploading', 'queued', 'reconciling'].includes(item.status);
        const remove = onRemove ? () => onRemove(item.id) : undefined;
        const recover = uncertain && onReconcile ? () => onReconcile(item.id) : failed && onRetry ? () => onRetry(item.id) : undefined;
        return item.file?.type.startsWith('image/') && !failed && !uncertain
          ? <DraftImage file={item.file} remove={remove} loading={loading}/>
          : <FileCard name={item.name} bytes={item.file?.size ?? 0} state={loading ? 'uploading' : failed || uncertain ? 'error' : 'ready'}
              status={uncertain ? tx(item.status === 'reconciling' ? 'agent:upload.reconciling' : 'agent:upload.uncertain') : undefined}
              labels={{ label: tx('agent:upload.pendingFile', { name: item.name }), remove: tx('artifacts:attachment-card.remove-value', { p0: item.name }), uploading: tx('agent:upload.uploading'), failed: tx('agent:upload.failed'), retry: tx(uncertain ? 'agent:upload.reconcile' : 'agent:upload.retry', { name: item.name }) }}
              onRemove={remove} onRetry={item.status === 'reconciling' ? undefined : recover}/>;
      }}/>
    </div>}
  </>;
}

function DraftImage({ file, remove, loading }: { file: File; remove?: () => void; loading: boolean }) {
  const tx = useTranslation();
  const [preview, setPreview] = useState(false), [failed, setFailed] = useState(false);
  const [url, setUrl] = useState<string>();
  const close = useCallback(() => setPreview(false), []);
  useEffect(() => {
    setFailed(false);
    const value = URL.createObjectURL(file); setUrl(value);
    return () => URL.revokeObjectURL(value);
  }, [file]);
  return <div className={css.imageItem} data-attachment-state={loading ? 'uploading' : 'ready'} aria-busy={loading}>
    <button type="button" className={css.thumbnail} aria-label={tx('artifacts:attachment-card.open-image-value', { p0: file.name })} onClick={() => setPreview(true)} disabled={!url || failed}>
      {failed ? <span>{tx('agent:copy.image-preview-unavailable')}</span> : <img src={url} alt={file.name} onError={() => { setFailed(true); setPreview(false); }}/>}
    </button>
    {loading && <span className={css.pending} role="status" aria-label={tx('agent:upload.uploading')}/>}
    {remove && <button type="button" className={css.remove} aria-label={tx('artifacts:attachment-card.remove-value', { p0: file.name })} onClick={remove}><IconCloseFill14 size={12}/></button>}
    {preview && url && !failed && <ImageLightbox src={url} alt={file.name} labels={{ dialog: file.name, close: tx('artifacts:attachment-card.close-dialog') }} onClose={close}/>}
  </div>;
}
