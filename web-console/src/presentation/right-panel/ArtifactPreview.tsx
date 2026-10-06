import { useTranslation } from '../../locale/react';
import type { ReactNode } from 'react';
import { Button } from '../primitives/Button';
import { MarkdownText } from '../markdown/MarkdownText';
import css from './ArtifactPreview.module.css';
import type { PreviewViewStateProps } from './preview-view-state';
import { useViewScroll } from './use-view-scroll';
/** Finite artifact presentation only; the app supplies bytes from its native resource API. */
export function ArtifactPreview({ name, text, url, image, markdown = false, error, loading, retry, decodeError, children, viewState, onViewStateChange, onDownload }: PreviewViewStateProps & { children?: ReactNode; name: string; text?: string; url?: string; image: boolean; markdown?: boolean; decodeError?: () => void; error?: string; loading: boolean; retry: () => void; onDownload: () => void }) {
  const tx = useTranslation();
  const wrap = viewState.wrap ?? true;
  const scroll = useViewScroll(viewState.bodyScrollTop ?? 0, viewState.bodyScrollLeft ?? 0,
    (bodyScrollTop, bodyScrollLeft) => onViewStateChange({ bodyScrollTop, bodyScrollLeft }), loading);
  return <section className={css.preview} aria-label={tx('artifacts:artifact-preview.artifact-preview')}><header className={css.header}><strong title={name}>{name}</strong>{text !== undefined && <Button size="sm" aria-pressed={wrap} onClick={() => onViewStateChange({ wrap: !wrap })}>{tx('artifacts:artifact-preview.wrap-lines')}</Button>}</header>
    {loading && <p role="status">{tx('artifacts:artifact-preview.reading-native-artifact')}</p>}
    {error && <div role="alert"><p>{error}</p><Button onClick={retry}>{tx('artifacts:artifact-preview.retry-preview')}</Button></div>}
    {(text !== undefined || image && url && !error) && <div className={css.body} ref={scroll.ref} onScroll={scroll.onScroll} data-preview-scroll="body" data-wrap={wrap}>
      {text !== undefined && (markdown ? <MarkdownText text={text}/> : <pre>{text}</pre>)}
      {image && url && !error && <img key={url} onLoad={scroll.restore} onError={decodeError} className={css.image} src={url} alt={name} />}
    </div>}
    {children}
    {!children && !image && text === undefined && !error && !loading && <p>{tx('artifacts:artifact-preview.this-artifact-has-no-supported-inline-viewer')}</p>}
    {url && <footer><Button data-preview-download onClick={onDownload}>{tx('artifacts:artifact-preview.download-artifact')}</Button></footer>}
  </section>;
}
