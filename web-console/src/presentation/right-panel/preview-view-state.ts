/** Inert occurrence metadata only. No bytes, URLs, source permissions or viewer owners. */
export interface PreviewViewState {
  wrap?: boolean;
  bodyScrollTop?: number;
  bodyScrollLeft?: number;
  textLoadedLines?: number;
  pdfZoom?: { readonly kind: 'fit-width' } | { readonly kind: 'fixed'; readonly scale: number };
  pdfScrollTop?: number;
  pdfScrollLeft?: number;
  workbookSheet?: number;
  htmlMode?: 'rendered' | 'source';
  htmlScrollTop?: number;
  htmlScrollLeft?: number;
}

export interface PreviewViewStateProps {
  viewState: PreviewViewState;
  onViewStateChange: (patch: Partial<PreviewViewState>) => void;
}
