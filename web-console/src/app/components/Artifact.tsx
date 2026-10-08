import { message } from '../../locale/translation';
import { useTranslation, useNotice } from '../../locale/react';
import type { ToolExecutionResult } from '../../../../protocol/app-server/v38';
import { createContext, useContext, useEffect, useState } from 'react';
import type { ArtifactResources } from '../../client/artifacts';
import { PreviewContext } from './ArtifactPreview';
import { Button } from '../../presentation/primitives/Button';
import { AttachmentCard } from '../../presentation/attachments/AttachmentCard';
import { PresentedFileCard, PresentedFiles } from '../../presentation/attachments/PresentedFileCard';
import cardCss from '../../presentation/attachments/Deliverables.module.css';
import { IconDownloadOutline16 } from '../../presentation/primitives/icons';
import { presentedDeliveries, type PresentedDelivery } from '../../bindings/present';
export const ArtifactContext = createContext<ArtifactResources | undefined>(undefined);
export function Artifact({ id, name = id, image = false, mimeType }: { id: string; name?: string; image?: boolean; mimeType?: string }) {
  const tx = useTranslation();
  const resources = useContext(ArtifactContext);
  const preview = useContext(PreviewContext);
  const [attempt, setAttempt] = useState(0);
  const [url, setUrl] = useState<string>();
  const [error, setError] = useNotice();
  const [loading, setLoading] = useState(false);
  useEffect(() => {
    if (!resources || !attempt) return;
    let live = true;
    let owned: string | undefined;
    setLoading(true); setError(''); setUrl(undefined);
    void resources.read(id, mimeType).then(value => {
      if (!live) { resources.release(value); return; }
      owned = value; setUrl(value);
    }).catch(cause => { if (live) setError(String(cause)); }).finally(() => { if (live) setLoading(false); });
    return () => { live = false; if (owned) resources.release(owned); };
  }, [resources, id, mimeType, attempt]);
  return <div><AttachmentCard name={name} image={image} url={url} error={error} loading={loading}
    onDownload={preview && resources ? () => preview.download({ source: { kind: 'artifact', id }, name, image, mimeType }) : undefined}
    onLoad={resources ? () => setAttempt(value => value + 1) : undefined}
    onDecodeError={() => { if (url) resources?.release(url); setUrl(undefined); setError(message('artifacts:copy.image-could-not-be-decoded')); }} />{preview && resources && <Button size="sm" onClick={() => preview.openPreview({ source: { kind: 'artifact', id }, name, image, mimeType })}>{tx('artifacts:artifact.preview')}{' '}{name}</Button>}</div>;
}

/** Only successful committed canonical Tool messages contribute cards. */
export function ToolDeliveries({ messageId, result }: { messageId: string; result: ToolExecutionResult }) {
  const tx = useTranslation(), preview = useContext(PreviewContext);
  const deliveries = presentedDeliveries(messageId, result);
  const intent = (delivery: PresentedDelivery) => ({ source: { kind: 'session_file' as const, messageId: delivery.messageId, index: delivery.index, file: delivery.file },
    name: delivery.file.name, image: delivery.file.mime_type.startsWith('image/'), mimeType: delivery.file.mime_type });
  return <PresentedFiles files={deliveries.map(delivery => ({ key: delivery.key, name: delivery.file.name, path: delivery.file.path, description: delivery.file.description }))}
    card={view => { const delivery = deliveries.find(item => item.key === view.key)!;
      return <PresentedFileCard file={view} onPreview={() => preview?.openPreview(intent(delivery))} actions={preview &&
        <button type="button" className={cardCss.action} aria-label={`${tx('artifacts:attachment-card.download')} ${delivery.file.name}`}
          title={tx('artifacts:attachment-card.download')} onClick={() => preview.download(intent(delivery))}><IconDownloadOutline16 size={14}/></button>}/>; }}/>;
}

/** Only typed artifact/image/file facts; arbitrary tool JSON is never interpreted. */
export function ToolArtifacts({ result }: { result: ToolExecutionResult }) {
  const refs = new Map<string, { id: string; name?: string; image: boolean; mimeType?: string }>();
  for (const reference of result.artifacts ?? []) refs.set(reference.artifact_id, { id: reference.artifact_id, name: reference.name ?? undefined, mimeType: reference.mime_type ?? undefined, image: reference.mime_type?.startsWith('image/') ?? false });
  for (const block of result.content ?? []) {
    if (block.type === 'image' || block.type === 'file') refs.set(block.artifact_id, { id: block.artifact_id, mimeType: block.type === 'file' ? block.mime_type ?? refs.get(block.artifact_id)?.mimeType : refs.get(block.artifact_id)?.mimeType, name: (block.type === 'image' ? block.alt : block.name) ?? undefined, image: block.type === 'image' || (block.mime_type?.startsWith('image/') ?? false) });
  }
  return refs.size ? <div className="attachment-gallery">{[...refs.values()].map(reference => <Artifact key={reference.id} {...reference} />)}</div> : null;
}
