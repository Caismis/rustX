import { useMemo } from 'react';
import DOMPurify from 'dompurify';
import { useTranslation } from '../../../locale/react';
import type { PreviewViewStateProps } from '../../../presentation/right-panel/preview-view-state';
import { useViewScroll } from '../../../presentation/right-panel/use-view-scroll';

export const HTML_CSP = "default-src 'none'; script-src 'none'; style-src 'none'; connect-src 'none'; frame-src 'none'; form-action 'none'; base-uri 'none'; object-src 'none'";
export function isolatedHtml(source: string): string {
  const clean = DOMPurify.sanitize(source, {
    WHOLE_DOCUMENT: false,
    ALLOWED_TAGS: ['h1', 'h2', 'h3', 'h4', 'h5', 'h6', 'p', 'div', 'span', 'br', 'hr', 'pre', 'code', 'blockquote', 'ol', 'ul', 'li', 'dl', 'dt', 'dd', 'table', 'caption', 'thead', 'tbody', 'tfoot', 'tr', 'th', 'td', 'strong', 'b', 'em', 'i', 's', 'u', 'sup', 'sub', 'a'],
    ALLOWED_ATTR: ['title', 'colspan', 'rowspan', 'scope', 'dir', 'lang'],
    ALLOW_DATA_ATTR: false,
    FORBID_TAGS: ['script', 'base', 'meta', 'link', 'iframe', 'frame', 'object', 'embed', 'form', 'input', 'button', 'textarea', 'select', 'svg', 'math', 'noscript'],
    FORBID_ATTR: ['href', 'src', 'srcset', 'action', 'formaction', 'target', 'ping', 'background', 'xlink:href'],
  });
  return `<!doctype html><html><head><meta http-equiv="Content-Security-Policy" content="${HTML_CSP}"></head><body>${clean}</body></html>`;
}
export function HtmlPreview({ bytes, viewState, onViewStateChange }: PreviewViewStateProps & { bytes: Uint8Array<ArrayBuffer> }) {
  const tx = useTranslation(), sourceMode = viewState.htmlMode === 'source';
  const source = useMemo(() => new TextDecoder('utf-8', { fatal: true }).decode(bytes), [bytes]);
  const document = useMemo(() => isolatedHtml(source), [source]);
  const scroll = useViewScroll(viewState.htmlScrollTop ?? 0, viewState.htmlScrollLeft ?? 0,
    (htmlScrollTop, htmlScrollLeft) => onViewStateChange({ htmlScrollTop, htmlScrollLeft }), sourceMode);
  return <div className="document-view" ref={scroll.ref} onScroll={scroll.onScroll} data-preview-scroll="html"><div className="document-toolbar">
    <button aria-pressed={!sourceMode} onClick={() => onViewStateChange({ htmlMode: 'rendered', htmlScrollTop: 0, htmlScrollLeft: 0 })}>{tx('artifacts:document.rendered')}</button>
    <button aria-pressed={sourceMode} onClick={() => onViewStateChange({ htmlMode: 'source', htmlScrollTop: 0, htmlScrollLeft: 0 })}>{tx('artifacts:document.source')}</button>
  </div>{sourceMode ? <pre className="document-source">{source}</pre>
    : <iframe title={tx('artifacts:document.html')} sandbox="" referrerPolicy="no-referrer" srcDoc={document} />}</div>;
}
