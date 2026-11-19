/* Copyright (c) 2026 DeepSeek. MIT. Source port; see PROVENANCE.md. */
/** Static HTML preview with no scripts, network resources, forms or nested frames. */
import { decodeText } from './bytes.ts'
import DOMPurify from 'dompurify'

/**
 * Prepare static content before the browser can load any document resources.
 * @param data - complete UTF-8 HTML bytes.
 * @returns document with the restrictive CSP first in its head.
 */
export function createBasicHtmlDocument(data: Uint8Array<ArrayBuffer>): string {
  const clean = DOMPurify.sanitize(decodeText(data), {
    WHOLE_DOCUMENT: true,
    FORBID_TAGS: ['noscript', 'base', 'link', 'meta', 'form', 'iframe', 'frame', 'object', 'embed', 'set', 'animate', 'animateMotion', 'animateTransform'],
    FORBID_ATTR: ['href', 'xlink:href', 'action', 'formaction', 'srcset', 'poster', 'background', 'target', 'ping'],
  })
  const parsed = new DOMParser().parseFromString(clean, 'text/html')
  const resourceCss = /\\|@import\b|(?:url|image-set|src)\s*\(/i
  for (const sheet of parsed.querySelectorAll('style')) {
    if (resourceCss.test(sheet.textContent ?? '')) sheet.remove()
  }
  for (const element of parsed.querySelectorAll('[style]')) {
    if (resourceCss.test(element.getAttribute('style') ?? '')) element.removeAttribute('style')
  }
  for (const element of parsed.querySelectorAll('[src]')) {
    if (!element.getAttribute('src')?.startsWith('data:')) element.removeAttribute('src')
  }
  const policy = parsed.createElement('meta')
  policy.setAttribute('http-equiv', 'Content-Security-Policy')
  policy.setAttribute('content', "default-src 'none'; script-src 'none'; style-src 'unsafe-inline'; img-src data:; font-src data:; media-src data:; connect-src 'none'; frame-src 'none'; form-action 'none'; base-uri 'none'")
  parsed.head.prepend(policy)
  return `<!doctype html>${parsed.documentElement.outerHTML}`
}
