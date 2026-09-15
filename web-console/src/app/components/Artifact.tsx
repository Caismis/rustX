import type { ToolExecutionResult } from '../../../../protocol/app-server/v1';
import { createContext, useContext, useEffect, useState } from 'react';
import type { ArtifactResources } from '../../client/artifacts';
import { AttachmentCard } from '../../presentation/attachments/AttachmentCard';
export const ArtifactContext = createContext<ArtifactResources | undefined>(undefined);
export function Artifact({ id, name = id, image = false }: { id: string; name?: string; image?: boolean }) {
  const resources = useContext(ArtifactContext);
  const [attempt, setAttempt] = useState(0);
  const [url, setUrl] = useState<string>();
  const [error, setError] = useState<string>();
  const [loading, setLoading] = useState(false);
  useEffect(() => {
    if (!resources || !attempt) return;
    let live = true;
    let owned: string | undefined;
    setLoading(true); setError(undefined); setUrl(undefined);
    void resources.read(id).then(value => {
      if (!live) { resources.release(value); return; }
      owned = value; setUrl(value);
    }).catch(cause => { if (live) setError(String(cause)); }).finally(() => { if (live) setLoading(false); });
    return () => { live = false; if (owned) resources.release(owned); };
  }, [resources, id, attempt]);
  return <AttachmentCard name={name} image={image} url={url} error={error} loading={loading}
    onLoad={resources ? () => setAttempt(value => value + 1) : undefined}
    onDecodeError={() => { if (url) resources?.release(url); setUrl(undefined); setError('Image could not be decoded'); }} />;
}

/** Only typed artifact/image/file facts; arbitrary tool JSON is never interpreted. */
export function ToolArtifacts({ result }: { result: ToolExecutionResult }) {
  const refs = new Map<string, { id: string; name?: string; image: boolean }>();
  for (const reference of result.artifacts ?? []) refs.set(reference.artifact_id, { id: reference.artifact_id, name: reference.name ?? undefined, image: reference.mime_type?.startsWith('image/') ?? false });
  for (const block of result.content ?? []) {
    if (block.type === 'image' || block.type === 'file') refs.set(block.artifact_id, { id: block.artifact_id, name: (block.type === 'image' ? block.alt : block.name) ?? undefined, image: block.type === 'image' || (block.mime_type?.startsWith('image/') ?? false) });
  }
  return refs.size ? <div className="attachment-gallery">{[...refs.values()].map(reference => <Artifact key={reference.id} {...reference} />)}</div> : null;
}
