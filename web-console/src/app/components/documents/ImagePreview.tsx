/* Copyright (c) 2026 DeepSeek. MIT. ImageBody presentation adapted; see PROVENANCE.md. */
import { useCallback, useState } from 'react';
import { useTranslation } from '../../../locale/react';
import { ZoomViewport, zoomSurfaceClass } from '../workbench-documents/zoom/ZoomViewport';
import type { PreviewViewStateProps } from '../../../presentation/right-panel/preview-view-state';
import { useViewScroll } from '../../../presentation/right-panel/use-view-scroll';
export function ImagePreview({ url, name, signal, decodeError, viewState, onViewStateChange }: PreviewViewStateProps & { url: string; name: string; signal: AbortSignal; decodeError: () => void }) {
  const tx = useTranslation(), [width, setWidth] = useState<number>();
  const scroll = useViewScroll<HTMLElement>(viewState.bodyScrollTop ?? 0, viewState.bodyScrollLeft ?? 0, (bodyScrollTop, bodyScrollLeft) => onViewStateChange({ bodyScrollTop, bodyScrollLeft }), width);
  const attach = useCallback((node: HTMLElement | null) => {
    scroll.ref.current = node;
    if (node) { scroll.restore(); node.onscroll = () => scroll.onPosition(node); }
  }, []);
  return <ZoomViewport preference={viewState.imageZoom ?? { kind: 'fit-width' }} intrinsicWidth={width} signal={signal} scrollportRef={attach}
    onPreference={imageZoom => onViewStateChange({ imageZoom })} labels={{ controls: tx('artifacts:document.zoom'), menu: tx('artifacts:document.zoom'), out: tx('artifacts:workbench.zoom-out'), into: tx('artifacts:workbench.zoom-in'), fitWidth: tx('artifacts:workbench.fit-width'), value: percent => `${percent}%` }}>
    <img key={url} src={url} alt={name} onError={decodeError} onLoad={event => { setWidth(event.currentTarget.naturalWidth); scroll.restore(); }} className={zoomSurfaceClass} data-document-zoom-surface style={{ '--document-zoom-width': `${width ?? 1}px` } as import('react').CSSProperties}/>
  </ZoomViewport>;
}
