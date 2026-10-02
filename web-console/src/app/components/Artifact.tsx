import { message } from '../../locale/translation';
import { useTranslation, useNotice } from '../../locale/react';
import type { ToolExecutionResult } from '../../../../protocol/app-server/v33';
import { createContext, useContext, useEffect, useState } from 'react';
import type { ArtifactResources } from '../../client/artifacts';
import { PreviewContext } from './ArtifactPreview';
import { Button } from '../../presentation/primitives/Button';
import { AttachmentCard } from '../../presentation/attachments/AttachmentCard';
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
    onLoad={resources ? () => setAttempt(value => value + 1) : undefined}
    onDecodeError={() => { if (url) resources?.release(url); setUrl(undefined); setError(message('artifacts:copy.image-could-not-be-decoded')); }} />{preview && resources && <Button size="sm" onClick={() => preview({ source: { kind: 'artifact', id }, name, image, mimeType })}>{tx('artifacts:artifact.preview')}{' '}{name}</Button>}</div>;
}

/** Only successful committed canonical Tool messages contribute cards. */
export function ToolDeliveries({ messageId, result }: { messageId: string; result: ToolExecutionResult }) {
  const tx = useTranslation(), preview = useContext(PreviewContext);
  if (result.status.type !== 'success') return null;
  return <div className="attachment-gallery">{result.deliveries?.map((file, index) => {
    const artifact = { source: { kind: 'session_file' as const, messageId, index, file }, name: file.name, image: file.mime_type.startsWith('image/'), mimeType: file.mime_type };
    return <div key={index} data-delivery-card role="group" aria-label={file.name} title={file.path}>
      <AttachmentCard name={file.name} image={artifact.image} mimeType={file.mime_type} description={file.description} actions={preview && <>
        <Button size="sm" aria-label={`${tx('artifacts:artifact.preview')} ${file.name}`} onClick={() => preview(artifact)}>{tx('artifacts:artifact.preview')}</Button>
        <Button size="sm" aria-label={`${tx('artifacts:attachment-card.download')} ${file.name}`} onClick={() => preview({ ...artifact, download: true })}>{tx('artifacts:attachment-card.download')}</Button>
      </>}/>
    </div>;
  })}</div>;
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
