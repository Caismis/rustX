/* Copyright (c) 2026 DeepSeek. MIT. ImageBody presentation adapted; see PROVENANCE.md. */
import { useCallback, useEffect, useRef, useState } from 'react';
import { useTranslation } from '../../../locale/react';
import { ZoomViewport, zoomSurfaceClass } from '../workbench-documents/zoom/ZoomViewport';
import type { PreviewViewStateProps } from '../../../presentation/right-panel/preview-view-state';
import { bindImagePan } from './image-pan';
import css from './ImagePreview.module.css';
import { LoadingIndicator } from '../../../presentation/primitives/LoadingIndicator';
import { useViewScroll } from '../../../presentation/right-panel/use-view-scroll';
export function ImagePreview({ url, name, signal, decodeError, viewState, onViewStateChange }: PreviewViewStateProps & { url: string; name: string; signal: AbortSignal; decodeError: () => void }) {
  const image = useRef<HTMLImageElement>(null);
  useEffect(() => image.current ? bindImagePan(image.current) : undefined, [url]);
  const tx = useTranslation(), [metadata, setMetadata] = useState<{ url: string; width: number }>();
  const width = metadata?.url === url ? metadata.width : undefined;
  const scroll = useViewScroll<HTMLElement>(viewState.bodyScrollTop ?? 0, viewState.bodyScrollLeft ?? 0, (bodyScrollTop, bodyScrollLeft) => onViewStateChange({ bodyScrollTop, bodyScrollLeft }), width);
  const attach = useCallback((node: HTMLElement | null) => {
    scroll.ref.current = node;
    if (node) { scroll.restore(); node.onscroll = () => scroll.onPosition(node); }
  }, []);
  return <ZoomViewport preference={viewState.imageZoom ?? { kind: 'fit-width' }} intrinsicWidth={width} horizontalInset={24} signal={signal} scrollportRef={attach}
    onPreference={imageZoom => onViewStateChange({ imageZoom })} labels={{ controls: tx('artifacts:document.zoom'), menu: tx('artifacts:document.zoom'), out: tx('artifacts:workbench.zoom-out'), into: tx('artifacts:workbench.zoom-in'), fitWidth: tx('artifacts:workbench.fit-width'), value: percent => `${percent}%` }}>
    <div className={css.frame} data-image-preview>{width === undefined && <LoadingIndicator label={tx('artifacts:workbench.loading')}/>}<div className={`${css.surface} ${zoomSurfaceClass}`} hidden={width === undefined} data-document-zoom-surface style={{ '--document-zoom-width': `${width ?? 0}px` } as import('react').CSSProperties}><img ref={image} key={url} src={url} alt={name} decoding="async" referrerPolicy="no-referrer" onError={decodeError} onLoad={event => { setMetadata({ url, width: event.currentTarget.naturalWidth }); scroll.restore(); }} className={css.image}/></div></div>
  </ZoomViewport>;
}
