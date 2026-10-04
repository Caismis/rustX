import { createContext, useEffect, useState } from 'react';
import { useTranslation } from '../../locale/react';
import type { FilePreviewResources, LoadedFile, PreviewSource } from '../../client/session-files';
import { ArtifactPreview as Preview } from '../../presentation/right-panel/ArtifactPreview';
import { documentKind } from '../../client/document-types';
import { DocumentPreview } from './documents/DocumentPreview';
export interface PreviewArtifact { source: PreviewSource; name: string; image: boolean; mimeType?: string; download?: boolean }
export const PreviewContext = createContext<((artifact: PreviewArtifact) => void) | undefined>(undefined);
export function ArtifactPreview({ artifact, resources }: { artifact: PreviewArtifact; resources: FilePreviewResources }) {
  const tx = useTranslation();
  const kind = artifact.download ? undefined : documentKind(artifact.name);
  const [attempt, retry] = useState(0);
  const [content, setContent] = useState<Partial<LoadedFile>>({});
  const [loading, setLoading] = useState(true);
  useEffect(() => {
    const read = new AbortController();
    let live = true, owned: string | undefined;
    setLoading(true); setContent({});
    void resources.load(artifact.source, artifact.mimeType, artifact.image, read.signal, kind !== undefined).then(value => {
      owned = value.url;
      if (!live) { resources.release(artifact.source, owned); return; }
      setContent(value);
      if (artifact.download) {
        const anchor = document.createElement('a');
        anchor.href = value.url; anchor.download = artifact.name; anchor.click();
      }
    }).catch(error => { if (live) setContent({ error: String(error) }); }).finally(() => { if (live) setLoading(false); });
    return () => { live = false; read.abort(); if (owned) resources.release(artifact.source, owned); };
  }, [artifact, resources, attempt, kind]);
  return <Preview name={artifact.name} image={artifact.image} markdown={artifact.mimeType === 'text/markdown'} {...content} loading={loading}
    decodeError={() => setContent(value => content.url && value.url === content.url ? { ...value, error: tx('artifacts:copy.image-could-not-be-decoded') } : value)} retry={() => retry(value => value + 1)} >
      {kind && content.bytes && <DocumentPreview kind={kind} bytes={content.bytes} source={artifact.source} resources={resources} retry={() => retry(value => value + 1)} />}
    </Preview>;
}
