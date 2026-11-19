import { createContext, useEffect, useState, useSyncExternalStore } from 'react';
import { useTranslation } from '../../locale/react';
import type { FilePreviewLease, PreviewSource } from '../../client/session-files';
import { validateRaster } from '../../client/raster';
import { ArtifactPreview as Preview } from '../../presentation/right-panel/ArtifactPreview';
import { documentKind } from '../../../shared/documents.ts';
import { DocumentPreview } from './documents/DocumentPreview';
import type { PreviewViewStateProps } from '../../presentation/right-panel/preview-view-state';
import { viewersForArtifact, loadsBytes } from './workbench-documents/definitions';
import { ImagePreview } from './documents/ImagePreview';
import { CodeBlock } from '../../presentation/markdown/CodeBlock';
import { languageForPath } from '../../presentation/primitives/file-language';
import { ViewerMenu } from './workbench-documents/ViewerMenu';
import { MarkdownBody } from './workbench-documents/MarkdownBody';
import { documentFailure } from './documents/errors';
import { GeometryDocument, preloadGeometry } from './documents/GeometryDocument';
import { GeometryLoading } from './documents/GeometryLoading';
export interface PreviewArtifact { source: PreviewSource; name: string; image: boolean; mimeType?: string }
export interface PreviewIntents { openPreview: (artifact: PreviewArtifact) => void; download: (artifact: PreviewArtifact) => void }
export const PreviewContext = createContext<PreviewIntents | undefined>(undefined);
export function ArtifactPreview({ artifact, resources, viewState, onViewStateChange, onDownload, visible = true }: PreviewViewStateProps & { visible?: boolean; artifact: PreviewArtifact; resources: FilePreviewLease; onDownload: () => void }) {
  const tx = useTranslation();
  // Managed display names can be opaque IDs or alt text. Preserve their typed
  // media semantics; filesystem deliveries still use the suffix policy.
  const managed = artifact.source.kind === 'artifact', mime = managed ? artifact.mimeType?.split(';')[0].trim().toLowerCase() : undefined;
  const typedMarkdown = mime === 'text/markdown';
  const choices = viewersForArtifact(artifact);
  const mode = viewState.viewer && choices.includes(viewState.viewer) ? viewState.viewer : choices[0];
  const kind = mode === 'html' ? 'html' : loadsBytes(mode) || mode === 'office' ? documentKind(artifact.name) : undefined;
  const office = mode === 'office' && (kind === 'docx' || kind === 'pptx');
  const paged = mode !== undefined && !loadsBytes(mode) && !office;
  const owner = resources.content!, content = useSyncExternalStore(owner.subscribe, owner.getSnapshot);
  const [image, setImage] = useState<{ url?: string; error?: string }>({});
  const saveViewState: PreviewViewStateProps['onViewStateChange'] = patch => { if (resources.current()) onViewStateChange(patch); };
  const sameGeometryCopy = () => owner.getSnapshot().revision === content.revision && owner.getSnapshot().version === content.version && owner.getSnapshot().data === content.data;
  useEffect(() => { if (mode === 'geometry') preloadGeometry(artifact.name); if (mode) owner.ensure(mode === 'geometry' ? 'geometry' : office ? kind : paged ? 'pages' : 'bytes', viewState.textLoadedLines); }, [owner, kind, office, paged, mode, artifact.name]);
  useEffect(() => {
    if (content.text !== undefined) saveViewState({ textLoadedLines: content.loadedLines });
  }, [owner, content.text, content.loadedLines]);
  useEffect(() => {
    setImage({});
    if (mode !== 'image' || !content.data || !resources.current()) return;
    try {
      validateRaster(content.data);
      const url = resources.retain(content.data, artifact.mimeType); setImage({ url });
      return () => resources.release(url);
    } catch { setImage({ error: tx('artifacts:document.malformed') }); }
  }, [resources, content.data, mode, artifact.mimeType]);
  const initialLoading = content.loading && content.text === undefined && content.data === undefined;
  const retry = () => owner.retry();
  const decodeError = () => setImage(value => ({ ...value, error: tx('artifacts:copy.image-could-not-be-decoded') }));
  const failure = content.error === undefined ? undefined : documentFailure(content.error);
  const error = failure === undefined ? image.error : tx(`artifacts:document.${failure}`);
  return <Preview name={artifact.name} path={artifact.source.kind === 'session_file' ? artifact.source.file.path : artifact.name} image={false} markdown={mode === 'markdown'}
    viewers={<ViewerMenu choices={choices} mode={mode} select={viewer => saveViewState({ viewer })}/>}
    renderText={mode === 'code' ? text => <CodeBlock code={text} lang={typedMarkdown ? 'markdown' : mime === 'text/html' ? 'html' : languageForPath(artifact.name)} lineNumbers copyLabel={tx('common:search-block.copy')} copiedLabel={tx('common:search-block.copied')}/> : mode === 'markdown' ? text => <MarkdownBody text={text} streaming={content.page !== undefined && !content.page.eof}/> : undefined}
    text={paged ? content.text : undefined} error={error} retryable={!['source_missing','source_unavailable','authorization_revoked','obsolete'].includes(failure ?? '') && !(failure === 'too_large' && content.text === undefined)} loading={initialLoading} loadingLabel={mode === 'geometry' ? <GeometryLoading stage="reading"/> : undefined}
    notice={mode !== 'geometry' && !owner.automaticRefresh ? tx('artifacts:document.manual-refresh') : undefined}
    hasMore={paged && content.page !== undefined && !content.page.eof} moreLoading={content.loading} onLoadMore={() => owner.more()}
    viewState={viewState} onViewStateChange={saveViewState} onDownload={() => { if (resources.current()) onDownload(); }}
    refresh={() => owner.refresh()} retry={retry}>
    {mode === 'geometry' ? <GeometryDocument visible={visible} bytes={content.data} name={artifact.name} copyId={`${content.revision}:${content.version}`} signal={resources.signal} current={() => resources.current() && sameGeometryCopy()} takeSource={() => content.data && sameGeometryCopy() ? owner.takeGeometryBytes(content.data) : undefined} releaseSource={() => { if (content.data && sameGeometryCopy()) owner.releaseGeometryBytes(content.data); }} viewState={viewState} onViewStateChange={saveViewState} onGeometryRetire={geometry => { if (resources.sourceCurrent() && sameGeometryCopy()) onViewStateChange({ geometry }); }}/>
      : mode === 'image' ? image.url && !error ? <ImagePreview url={image.url} name={artifact.name} signal={resources.signal} decodeError={decodeError} viewState={viewState} onViewStateChange={saveViewState}/> : undefined
      : kind && content.data ? <DocumentPreview kind={kind === 'docx' || kind === 'pptx' ? 'pdf' : kind} derived={office} bytes={content.data} resources={resources} viewState={viewState} onViewStateChange={saveViewState} retry={retry}/> : undefined}
  </Preview>;
}
