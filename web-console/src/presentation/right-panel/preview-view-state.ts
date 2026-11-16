/** Inert occurrence metadata only. No bytes, URLs, source permissions or viewer owners. */
export interface PreviewViewState {
  viewer?: 'markdown' | 'html' | 'image' | 'pdf' | 'excel' | 'office' | 'geometry' | 'code' | 'text';
  geometry?: {
    copyId: string;
    camera?: { position: [number, number, number]; target: [number, number, number]; up: [number, number, number] }
      & ({ projection: 'perspective' } | { projection: 'orthographic'; verticalSpan: number });
    selected?: string;
    hidden?: string[];
    isolation?: { target: string; hidden: string[] };
    outlines?: boolean;
    navigationMode?: 'orbit' | 'pan';
  };
  imageZoom?: { readonly kind: 'fit-width' } | { readonly kind: 'fixed'; readonly scale: number };
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
