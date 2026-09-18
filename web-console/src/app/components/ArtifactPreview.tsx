import { createContext, useEffect, useState } from 'react';
import type { ArtifactResources } from '../../client/artifacts';
import { ArtifactPreview as Preview } from '../../presentation/right-panel/ArtifactPreview';
export interface PreviewArtifact { id: string; name: string; image: boolean; mimeType?: string; }
export const PreviewContext = createContext<((artifact: PreviewArtifact) => void) | undefined>(undefined);
export function ArtifactPreview({ artifact, resources }: { artifact: PreviewArtifact; resources: ArtifactResources }) {
  const [attempt, retry] = useState(0);
  const [content, setContent] = useState<{ text?: string; url?: string; error?: string }>({});
  const [loading, setLoading] = useState(true);
  useEffect(() => {
    let live = true, owned: string | undefined;
    setLoading(true); setContent({});
    const text = artifact.mimeType === 'text/plain' || artifact.mimeType === 'application/json' || artifact.mimeType === 'text/markdown';
    void (text ? resources.readText(artifact.id).then(text => ({ text })) : resources.read(artifact.id, artifact.mimeType).then(url => ({ url }))).then(value => {
      if ('url' in value) owned = value.url;
      if (live) setContent(value); else if (owned) resources.release(owned);
    }).catch(error => { if (live) setContent({ error: String(error) }); }).finally(() => { if (live) setLoading(false); });
    return () => { live = false; if (owned) resources.release(owned); };
  }, [artifact, resources, attempt]);
  return <Preview name={artifact.name} image={artifact.image} {...content} loading={loading} retry={() => retry(value => value + 1)} />;
}
