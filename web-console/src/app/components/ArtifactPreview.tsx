import { createContext, useEffect, useState } from 'react';
import { useTranslation } from '../../locale/react';
import type { FilePreviewLease, LoadedFile, PreviewSource } from '../../client/session-files';
import { ArtifactPreview as Preview } from '../../presentation/right-panel/ArtifactPreview';
import { documentKind } from '../../../shared/documents.ts';
import { DocumentPreview } from './documents/DocumentPreview';
import type { PreviewViewStateProps } from '../../presentation/right-panel/preview-view-state';
export interface PreviewArtifact { source: PreviewSource; name: string; image: boolean; mimeType?: string }
export interface PreviewIntents { openPreview: (artifact: PreviewArtifact) => void; download: (artifact: PreviewArtifact) => void }
export const PreviewContext = createContext<PreviewIntents | undefined>(undefined);
export function ArtifactPreview({ artifact, resources, viewState, onViewStateChange, onDownload }: PreviewViewStateProps & { artifact: PreviewArtifact; resources: FilePreviewLease; onDownload: () => void }) {
  const tx = useTranslation();
  const kind = documentKind(artifact.name);
  const saveViewState: PreviewViewStateProps['onViewStateChange'] = patch => { if (resources.current()) onViewStateChange(patch); };
  const [attempt, retry] = useState(0);
  const [content, setContent] = useState<Partial<LoadedFile>>({});
  const [loading, setLoading] = useState(true);
  useEffect(() => {
    const read = new AbortController();
    let live = true, owned: string | undefined;
    setLoading(true); setContent({});
    const current = () => live && !read.signal.aborted && resources.current();
    void resources.load(artifact.mimeType, artifact.image, read.signal, kind !== undefined).then(value => {
      owned = value.url;
      if (!current()) { resources.release(owned); return; }
      setContent(value);
    }).catch(error => { if (current()) setContent({ error: String(error) }); }).finally(() => { if (current()) setLoading(false); });
    return () => { live = false; read.abort(); if (owned) resources.release(owned); };
  }, [artifact.mimeType, artifact.image, resources, attempt, kind]);
  return <Preview name={artifact.name} path={artifact.source.kind === 'session_file' ? artifact.source.file.path : artifact.name} image={artifact.image} markdown={artifact.mimeType === 'text/markdown'} {...content} loading={loading}
    viewState={viewState} onViewStateChange={saveViewState} onDownload={() => { if (resources.current()) onDownload(); }}
    decodeError={() => setContent(value => resources.current() && content.url && value.url === content.url ? { ...value, error: tx('artifacts:copy.image-could-not-be-decoded') } : value)} retry={() => retry(value => value + 1)} >
      {kind && content.bytes && <DocumentPreview kind={kind} bytes={content.bytes} resources={resources} viewState={viewState} onViewStateChange={saveViewState} retry={() => retry(value => value + 1)} />}
    </Preview>;
}
