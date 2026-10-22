import { createContext, useEffect, useRef, useState } from 'react';
import { useTranslation } from '../../locale/react';
import type { FilePreviewLease, FileTextPage, LoadedFile, PreviewSource } from '../../client/session-files';
import { isTextMime } from '../../client/artifacts';
import { ArtifactPreview as Preview } from '../../presentation/right-panel/ArtifactPreview';
import { documentKind } from '../../../shared/documents.ts';
import { DocumentPreview } from './documents/DocumentPreview';
import type { PreviewViewStateProps } from '../../presentation/right-panel/preview-view-state';
export interface PreviewArtifact { source: PreviewSource; name: string; image: boolean; mimeType?: string }
export interface PreviewIntents { openPreview: (artifact: PreviewArtifact) => void; download: (artifact: PreviewArtifact) => void }
export const PreviewContext = createContext<PreviewIntents | undefined>(undefined);
export function ArtifactPreview({ artifact, resources, viewState, onViewStateChange, onDownload }: PreviewViewStateProps & { artifact: PreviewArtifact; resources: FilePreviewLease; onDownload: () => void }) {
  const tx = useTranslation();
  const kind = documentKind(artifact.name), office = kind === 'docx' || kind === 'pptx';
  const paged = !kind && isTextMime(artifact.mimeType);
  const saveViewState: PreviewViewStateProps['onViewStateChange'] = patch => { if (resources.current()) onViewStateChange(patch); };
  const [attempt, retry] = useState(0);
  const [content, setContent] = useState<Partial<LoadedFile>>({});
  const [page, setPage] = useState<FileTextPage>(), [loading, setLoading] = useState(true), [moreLoading, setMoreLoading] = useState(false);
  const lifetime = useRef<AbortController | undefined>(undefined), loadingNext = useRef(false);
  const restartText = () => {
    saveViewState({ textLoadedLines: 0, bodyScrollTop: 0, bodyScrollLeft: 0 });
    retry(value => value + 1);
  };
  useEffect(() => {
    const read = new AbortController(); lifetime.current = read;
    let owned: string | undefined;
    setContent({}); setPage(undefined); setMoreLoading(false); loadingNext.current = false;
    setLoading(!office);
    const current = () => !read.signal.aborted && resources.current();
    if (!office) {
      // Sample the restoration target for this lease; metadata changes must not
      // start another read. Restore scrolling only after this prefix exists.
      const savedLines = viewState.textLoadedLines;
      const wanted = Number.isSafeInteger(savedLines) && savedLines! > 0 ? savedLines! : 0;
      const readPages = async () => {
        let offset = 1, prefix = '', version: string | undefined;
        while (current()) {
          const value = await resources.loadPage(offset, read.signal);
          if (!current()) return;
          if (version !== undefined && value.version !== version) { restartText(); return; }
          version = value.version;
          prefix += `${offset > 1 && value.lines ? '\n' : ''}${value.text}`;
          const loaded = value.offset + value.lines - 1;
          setPage(value); setContent({ text: prefix });
          saveViewState({ textLoadedLines: value.eof ? loaded : Math.max(wanted, loaded) });
          if (value.eof || loaded >= wanted) return;
          offset = loaded + 1;
        }
      };
      const work = paged
        ? readPages()
        : resources.load(artifact.mimeType, artifact.image, read.signal, kind !== undefined).then(value => {
          owned = value.url;
          if (!current()) { resources.release(owned); return; }
          setContent(value);
        });
      void work.catch(error => { if (current()) setContent(previous => ({ ...previous, error: String(error) })); }).finally(() => { if (current()) setLoading(false); });
    }
    return () => { read.abort(); if (owned) resources.release(owned); };
  }, [artifact.mimeType, artifact.image, resources, attempt, kind, paged, office]);
  const more = () => {
    const read = lifetime.current;
    if (!read || read.signal.aborted || !page || page.eof || loading || loadingNext.current || !resources.current()) return;
    loadingNext.current = true; setMoreLoading(true);
    const current = () => !read.signal.aborted && resources.current();
    void resources.loadPage(page.offset + page.lines, read.signal).then(next => {
      if (!current()) return;
      if (next.version !== page.version) { restartText(); return; }
      setContent(previous => ({ ...previous, error: undefined, text: `${previous.text ?? ''}${next.lines ? '\n' : ''}${next.text}` }));
      setPage(next);
      saveViewState({ textLoadedLines: next.offset + next.lines - 1 });
    }).catch(error => { if (current()) setContent(previous => ({ ...previous, error: String(error) })); })
      .finally(() => { if (current()) { loadingNext.current = false; setMoreLoading(false); } });
  };
  return <Preview name={artifact.name} path={artifact.source.kind === 'session_file' ? artifact.source.file.path : artifact.name} image={artifact.image} markdown={artifact.mimeType === 'text/markdown'} {...content} loading={loading}
    hasMore={paged && page !== undefined && !page.eof} moreLoading={moreLoading || loading} onLoadMore={more}
    viewState={viewState} onViewStateChange={saveViewState} onDownload={() => { if (resources.current()) onDownload(); }}
    decodeError={() => setContent(value => resources.current() && content.url && value.url === content.url ? { ...value, error: tx('artifacts:copy.image-could-not-be-decoded') } : value)} retry={() => retry(value => value + 1)} >
      {kind && (office || content.bytes) && <DocumentPreview key={attempt} kind={kind} bytes={content.bytes} resources={resources} viewState={viewState} onViewStateChange={saveViewState} retry={() => retry(value => value + 1)} />}
    </Preview>;
}
