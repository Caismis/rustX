import { useTranslation } from '../../locale/react';
import type { ReactNode } from 'react';
import { Button } from '../primitives/Button';
import { MarkdownText } from '../markdown/MarkdownText';
import { PathLabel } from '../primitives/PathLabel';
import { Tooltip } from '../primitives/Tooltip';
import { IconDownloadOutline16, IconRefreshOutline16, IconWrapLinesOutline16 } from '../primitives/icons';
import css from './ArtifactPreview.module.css';
import type { PreviewViewStateProps } from './preview-view-state';
import { useViewScroll } from './use-view-scroll';
/** Finite artifact presentation only; the app supplies bytes from its native resource API. */
export function ArtifactPreview({ name, path = name, text, url, image, markdown = false, error, notice, retryable = true, loading, refresh, retry, decodeError, children, viewers, renderText, viewState, onViewStateChange, onDownload, hasMore = false, moreLoading = false, onLoadMore }: PreviewViewStateProps & { children?: ReactNode; viewers?: ReactNode; renderText?: (text: string) => ReactNode; name: string; path?: string; text?: string; url?: string; image: boolean; markdown?: boolean; decodeError?: () => void; error?: string; notice?: string; retryable?: boolean; loading: boolean; refresh: () => void; retry: () => void; onDownload: () => void; hasMore?: boolean; moreLoading?: boolean; onLoadMore?: () => void }) {
  const tx = useTranslation();
  const wrap = viewState.wrap ?? true;
  const scroll = useViewScroll(viewState.bodyScrollTop ?? 0, viewState.bodyScrollLeft ?? 0,
    (bodyScrollTop, bodyScrollLeft) => onViewStateChange({ bodyScrollTop, bodyScrollLeft }), `${loading}:${markdown}:${text?.length ?? ''}`);
  return <section className={css.preview} aria-label={tx('artifacts:artifact-preview.artifact-preview')}><header className={css.header} data-preview-header>
    <PathLabel path={path} className={css.path}/>
    {viewers}
    {text !== undefined && !markdown && <Tooltip label={tx('artifacts:artifact-preview.wrap-lines')} side="bottom" delayMs={500}><button type="button" className={css.tool} aria-label={tx('artifacts:artifact-preview.wrap-lines')} aria-pressed={wrap} onClick={() => onViewStateChange({ wrap: !wrap })}><span aria-hidden="true"><IconWrapLinesOutline16/></span></button></Tooltip>}
    <Tooltip label={tx('artifacts:workbench.refresh')} side="bottom" delayMs={500}><button type="button" className={css.tool} aria-label={tx('artifacts:workbench.refresh')} onClick={refresh}><span aria-hidden="true"><IconRefreshOutline16/></span></button></Tooltip>
    <Tooltip label={tx('artifacts:artifact-preview.download-artifact')} side="bottom" delayMs={500}><button type="button" className={css.tool} data-preview-download aria-label={tx('artifacts:artifact-preview.download-artifact')} onClick={onDownload}><span aria-hidden="true"><IconDownloadOutline16/></span></button></Tooltip>
  </header>
    {notice && <p role="note" className={css.status}>{notice}</p>}
    {loading && <p className={css.status} role="status">{tx('artifacts:artifact-preview.reading-native-artifact')}</p>}
    {error && <div className={css.status} role="alert"><p>{error}</p>{retryable && <Button onClick={retry}>{tx('artifacts:artifact-preview.retry-preview')}</Button>}</div>}
    {(text !== undefined || image && url && !error) && <div className={css.body} ref={scroll.ref} onScroll={event => {
      scroll.onScroll(event);
      const node = event.currentTarget;
      if (hasMore && !moreLoading && !error && node.scrollHeight - node.scrollTop - node.clientHeight < 160) onLoadMore?.();
    }} data-preview-scroll="body" data-kind={markdown ? 'markdown' : image ? 'image' : 'text'} data-wrap={wrap}>
      {text !== undefined && (renderText ? renderText(text) : markdown ? <div className={css.document}><MarkdownText text={text} streaming={hasMore}/></div> : <pre>{text}</pre>)}
      {text === '' && !hasMore && <p className={css.status}>{tx('artifacts:document.empty')}</p>}
      {image && url && !error && <img key={url} onLoad={scroll.restore} onError={decodeError} className={css.image} src={url} alt={name} />}
      {hasMore && <Button disabled={moreLoading} onClick={onLoadMore}>{tx(moreLoading ? 'artifacts:document.loading' : 'artifacts:artifact-preview.load-more')}</Button>}
    </div>}
    {children}
    {!children && !image && text === undefined && !error && !loading && <p className={css.status}>{tx('artifacts:artifact-preview.this-artifact-has-no-supported-inline-viewer')}</p>}
  </section>;
}
