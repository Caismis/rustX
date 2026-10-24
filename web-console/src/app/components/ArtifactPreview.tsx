import { createContext, useEffect, useState, useSyncExternalStore } from 'react';
import { useTranslation } from '../../locale/react';
import type { FilePreviewLease, PreviewSource } from '../../client/session-files';
import { validateRaster } from '../../client/raster';
import { ArtifactPreview as Preview } from '../../presentation/right-panel/ArtifactPreview';
import { documentKind } from '../../../shared/documents.ts';
import { DocumentPreview } from './documents/DocumentPreview';
import type { PreviewViewStateProps } from '../../presentation/right-panel/preview-view-state';
import { viewersForPath, loadsBytes } from './workbench-documents/definitions';
import { ImagePreview } from './documents/ImagePreview';
import { CodeBlock } from '../../presentation/markdown/CodeBlock';
import { languageForPath } from '../../presentation/primitives/file-language';
import { Menu } from '../../presentation/primitives/Menu';
import { MarkdownText } from '../../presentation/markdown/MarkdownText';
import { splitFrontmatter } from './workbench-documents/frontmatter';
import { FrontmatterFields } from './workbench-documents/frontmatter-fields';
import { documentFailure } from './documents/errors';
export interface PreviewArtifact { source: PreviewSource; name: string; image: boolean; mimeType?: string }
export interface PreviewIntents { openPreview: (artifact: PreviewArtifact) => void; download: (artifact: PreviewArtifact) => void }
export const PreviewContext = createContext<PreviewIntents | undefined>(undefined);
export function ArtifactPreview({ artifact, resources, viewState, onViewStateChange, onDownload }: PreviewViewStateProps & { artifact: PreviewArtifact; resources: FilePreviewLease; onDownload: () => void }) {
  const tx = useTranslation(), choices = viewersForPath(artifact.name), [menu, setMenu] = useState(false);
  const mode = viewState.viewer && choices.includes(viewState.viewer) ? viewState.viewer : choices[0];
  const kind = loadsBytes(mode) || mode === 'office' ? documentKind(artifact.name) : undefined;
  const office = mode === 'office' && (kind === 'docx' || kind === 'pptx');
  const paged = mode !== undefined && !loadsBytes(mode) && !office;
  const owner = resources.content!, content = useSyncExternalStore(owner.subscribe, owner.getSnapshot);
  const [image, setImage] = useState<{ url?: string; error?: string }>({});
  const saveViewState: PreviewViewStateProps['onViewStateChange'] = patch => { if (resources.current()) onViewStateChange(patch); };
  useEffect(() => { if (mode) owner.ensure(office ? kind : paged ? 'pages' : 'bytes', viewState.textLoadedLines); }, [owner, kind, office, paged, mode]);
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
    viewers={choices.length > 1 && <Menu open={menu} onClose={() => setMenu(false)} autoFocus selectedId={mode} items={choices.map(id => ({ id, label: tx(`artifacts:workbench.${id}`) }))} onSelect={id => { saveViewState({ viewer: id as typeof mode }); setMenu(false); }} anchor={<button type="button" aria-label={tx('artifacts:workbench.viewer')} aria-haspopup="menu" aria-expanded={menu} onClick={() => setMenu(!menu)}>{tx(`artifacts:workbench.${mode}`)}</button>}/>}
    renderText={mode === 'code' ? text => <CodeBlock code={text} lang={languageForPath(artifact.name)} lineNumbers streaming={content.loading} copyLabel={tx('common:search-block.copy')} copiedLabel={tx('common:search-block.copied')}/> : mode === 'markdown' ? text => { const split = splitFrontmatter(text); return <div>{split && <FrontmatterFields source={split.source} fallback={<pre>{split.source}</pre>}/>}<MarkdownText text={split?.body ?? text} streaming={content.page !== undefined && !content.page.eof}/></div>; } : undefined}
    text={paged ? content.text : undefined} error={error} retryable={!['source_missing','source_unavailable','authorization_revoked','obsolete'].includes(failure ?? '') && !(failure === 'too_large' && content.text === undefined)} loading={initialLoading}
    notice={!owner.automaticRefresh ? tx('artifacts:document.manual-refresh') : undefined}
    hasMore={paged && content.page !== undefined && !content.page.eof} moreLoading={content.loading} onLoadMore={() => owner.more()}
    viewState={viewState} onViewStateChange={saveViewState} onDownload={() => { if (resources.current()) onDownload(); }}
    refresh={() => owner.refresh()} retry={retry}>
    {mode === 'image' ? image.url && !error ? <ImagePreview url={image.url} name={artifact.name} signal={resources.signal} decodeError={decodeError} viewState={viewState} onViewStateChange={saveViewState}/> : undefined
      : kind && content.data ? <DocumentPreview kind={kind === 'docx' || kind === 'pptx' ? 'pdf' : kind} derived={office} bytes={content.data} resources={resources} viewState={viewState} onViewStateChange={saveViewState} retry={retry}/> : undefined}
  </Preview>;
}
