import { useTranslation } from '../../locale/react';
import type { ReactNode } from 'react';
import { Button } from '../primitives/Button';
import { MarkdownText } from '../markdown/MarkdownText';
import { PreviewFrame } from './PreviewFrame';
import { TextLines } from './TextLines';
import { LoadingIndicator } from '../primitives/LoadingIndicator';
import { Tooltip } from '../primitives/Tooltip';
import { IconDownloadOutline16, IconRefreshOutline16, IconWrapLinesOutline16 } from '../primitives/icons';
import type { PreviewViewStateProps } from './preview-view-state';
import { useViewScroll } from './use-view-scroll';
/** Finite artifact presentation only; the app supplies bytes from its native resource API. */
export function ArtifactPreview({ name, path = name, text, url, image, markdown = false, error, notice, retryable = true, loading, loadingLabel, refresh, retry, decodeError, children, viewers, renderText, viewState, onViewStateChange, onDownload, hasMore = false, moreLoading = false, onLoadMore }: PreviewViewStateProps & { children?: ReactNode; viewers?: ReactNode; renderText?: (text: string) => ReactNode; name: string; path?: string; text?: string; url?: string; image: boolean; markdown?: boolean; decodeError?: () => void; error?: string; notice?: string; retryable?: boolean; loading: boolean; loadingLabel?: ReactNode; refresh: () => void; retry: () => void; onDownload: () => void; hasMore?: boolean; moreLoading?: boolean; onLoadMore?: () => void }) {
  const tx = useTranslation();
  const wrap = viewState.wrap ?? true;
  const scroll = useViewScroll(viewState.bodyScrollTop ?? 0, viewState.bodyScrollLeft ?? 0,
    (bodyScrollTop, bodyScrollLeft) => onViewStateChange({ bodyScrollTop, bodyScrollLeft }), `${loading}:${markdown}:${text?.length ?? ''}`);
  return <PreviewFrame path={path} label={tx('artifacts:artifact-preview.artifact-preview')} tools={<>
    {viewers}
    {text !== undefined && !markdown && <Tooltip label={tx('artifacts:artifact-preview.wrap-lines')} side="bottom" delayMs={500}><button type="button" className="flex flex-none items-center justify-center w-[28px] h-[28px] p-[6px] [color:var(--dsw-alias-label-secondary)] bg-transparent border-0 rounded-[var(--dsw-radius-sm)] cursor-pointer [&>span]:flex [&_svg]:w-[15px] [&_svg]:h-[15px] hover:not-disabled:bg-[var(--dsw-alias-interactive-bg-hover)] disabled:opacity-35 disabled:cursor-default focus-visible:outline-2 focus-visible:outline-[var(--dsw-alias-label-primary)] focus-visible:outline-offset-[-2px]" aria-label={tx('artifacts:artifact-preview.wrap-lines')} aria-pressed={wrap} onClick={() => onViewStateChange({ wrap: !wrap })}><span aria-hidden="true"><IconWrapLinesOutline16/></span></button></Tooltip>}
    <Tooltip label={tx('artifacts:workbench.refresh')} side="bottom" delayMs={500}><button type="button" className="flex flex-none items-center justify-center w-[28px] h-[28px] p-[6px] [color:var(--dsw-alias-label-secondary)] bg-transparent border-0 rounded-[var(--dsw-radius-sm)] cursor-pointer [&>span]:flex [&_svg]:w-[15px] [&_svg]:h-[15px] hover:not-disabled:bg-[var(--dsw-alias-interactive-bg-hover)] disabled:opacity-35 disabled:cursor-default focus-visible:outline-2 focus-visible:outline-[var(--dsw-alias-label-primary)] focus-visible:outline-offset-[-2px]" aria-label={tx('artifacts:workbench.refresh')} onClick={refresh}><span aria-hidden="true"><IconRefreshOutline16/></span></button></Tooltip>
    <Tooltip label={tx('artifacts:artifact-preview.download-artifact')} side="bottom" delayMs={500}><button type="button" className="flex flex-none items-center justify-center w-[28px] h-[28px] p-[6px] [color:var(--dsw-alias-label-secondary)] bg-transparent border-0 rounded-[var(--dsw-radius-sm)] cursor-pointer [&>span]:flex [&_svg]:w-[15px] [&_svg]:h-[15px] hover:not-disabled:bg-[var(--dsw-alias-interactive-bg-hover)] disabled:opacity-35 disabled:cursor-default focus-visible:outline-2 focus-visible:outline-[var(--dsw-alias-label-primary)] focus-visible:outline-offset-[-2px]" data-preview-download aria-label={tx('artifacts:artifact-preview.download-artifact')} onClick={onDownload}><span aria-hidden="true"><IconDownloadOutline16/></span></button></Tooltip>
  </>}>
    {notice && <p role="note" className="px-[16px] py-[10px] text-[13px]">{notice}</p>}
    {loading && <LoadingIndicator label={loadingLabel ?? tx('artifacts:artifact-preview.reading-native-artifact')}/>}
    {error && <div className="px-[16px] py-[10px] text-[13px]" role="alert"><p>{error}</p>{retryable && <Button onClick={retry}>{tx('artifacts:artifact-preview.retry-preview')}</Button>}</div>}
    {(text !== undefined || image && url && !error) && <div className="flex-1 min-h-0 m-0 px-[12px] py-[10px] overflow-auto [color:var(--dsw-alias-label-primary)] [font-family:var(--ds-font-family-code),monospace] text-[13px] leading-[1.6] whitespace-pre [&[data-kind=text]]:[--dsl-code-block-line-white-space:pre] [&[data-kind=text]_pre]:m-0 [&[data-kind=text]_pre]:max-h-none [&[data-kind=text]_pre]:overflow-visible [&[data-kind=text]_pre]:[font:inherit] [&[data-kind=text]_pre]:[white-space:inherit] [&[data-kind=text][data-wrap=true]]:[--dsl-code-block-line-white-space:pre-wrap] [&[data-kind=text][data-wrap=true]]:whitespace-pre-wrap [&[data-kind=text][data-wrap=true]]:wrap-anywhere [&[data-kind=markdown]]:p-0 [&[data-kind=markdown]]:whitespace-normal [&[data-kind=markdown]]:[font-family:var(--dsw-font,inherit)]" ref={scroll.ref} onScroll={event => {
      scroll.onScroll(event);
      const node = event.currentTarget;
      if (hasMore && !moreLoading && !error && node.scrollHeight - node.scrollTop - node.clientHeight < 160) onLoadMore?.();
    }} data-preview-scroll="body" data-kind={markdown ? 'markdown' : image ? 'image' : 'text'} data-wrap={wrap}>
      {text !== undefined && (renderText ? renderText(text) : markdown ? <div className="min-w-0 px-[12px] py-[10px] whitespace-normal"><MarkdownText text={text} streaming={hasMore}/></div> : <TextLines text={text}/>)}
      {text === '' && !hasMore && <p className="px-[16px] py-[10px] text-[13px]">{tx('artifacts:document.empty')}</p>}
      {image && url && !error && <img key={url} onLoad={scroll.restore} onError={decodeError} className="max-w-full object-contain" src={url} alt={name} />}
      {hasMore && <Button disabled={moreLoading} onClick={onLoadMore}>{tx(moreLoading ? 'artifacts:document.loading' : 'artifacts:artifact-preview.load-more')}</Button>}
    </div>}
    {children}
    {!children && !image && text === undefined && !error && !loading && <p className="px-[16px] py-[10px] text-[13px]">{tx('artifacts:artifact-preview.this-artifact-has-no-supported-inline-viewer')}</p>}
  </PreviewFrame>;
}
