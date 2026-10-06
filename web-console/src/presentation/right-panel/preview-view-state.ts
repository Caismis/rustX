/** Inert occurrence metadata only. No bytes, URLs, source permissions or viewer owners. */
export interface PreviewViewState {
  wrap?: boolean;
  bodyScrollTop?: number;
  bodyScrollLeft?: number;
  pdfPage?: number;
  pdfZoom?: number;
  pdfScrollTop?: number;
  pdfScrollLeft?: number;
  workbookSheet?: number;
  workbookOffset?: number;
  workbookScrollTop?: number;
  workbookScrollLeft?: number;
  htmlMode?: 'rendered' | 'source';
  htmlScrollTop?: number;
  htmlScrollLeft?: number;
}

export interface PreviewViewStateProps {
  viewState: PreviewViewState;
  onViewStateChange: (patch: Partial<PreviewViewState>) => void;
}
