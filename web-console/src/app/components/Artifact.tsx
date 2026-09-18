import type { ToolExecutionResult } from '../../../../protocol/app-server/v6';
import { createContext, useContext, useEffect, useState } from 'react';
import type { ArtifactResources } from '../../client/artifacts';
import { PreviewContext } from './ArtifactPreview';
import { Button } from '../../presentation/primitives/Button';
import { AttachmentCard } from '../../presentation/attachments/AttachmentCard';
export const ArtifactContext = createContext<ArtifactResources | undefined>(undefined);
export function Artifact({ id, name = id, image = false, mimeType }: { id: string; name?: string; image?: boolean; mimeType?: string }) {
  const resources = useContext(ArtifactContext);
  const preview = useContext(PreviewContext);
  const [attempt, setAttempt] = useState(0);
  const [url, setUrl] = useState<string>();
  const [error, setError] = useState<string>();
  const [loading, setLoading] = useState(false);
  useEffect(() => {
    if (!resources || !attempt) return;
    let live = true;
    let owned: string | undefined;
    setLoading(true); setError(undefined); setUrl(undefined);
    void resources.read(id, mimeType).then(value => {
      if (!live) { resources.release(value); return; }
      owned = value; setUrl(value);
    }).catch(cause => { if (live) setError(String(cause)); }).finally(() => { if (live) setLoading(false); });
    return () => { live = false; if (owned) resources.release(owned); };
  }, [resources, id, mimeType, attempt]);
  return <div><AttachmentCard name={name} image={image} url={url} error={error} loading={loading}
    onLoad={resources ? () => setAttempt(value => value + 1) : undefined}
    onDecodeError={() => { if (url) resources?.release(url); setUrl(undefined); setError('Image could not be decoded'); }} />{preview && resources && <Button size="sm" onClick={() => preview({ id, name, image, mimeType })}>Preview {name}</Button>}</div>;
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
