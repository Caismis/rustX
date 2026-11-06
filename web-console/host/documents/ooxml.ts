import { SaxesParser, type SaxesTagNS } from 'saxes';
import { posix } from 'node:path';
import { admitOoxml } from './archive.ts';

function fail(code = 'malformed'): never { throw new Error(code); }
const attribute = (tag: SaxesTagNS, local: string) => {
  const values = Object.values(tag.attributes).filter(a => a.local === local);
  if (values.length > 1) fail('archive_rejected');
  return values[0]?.value;
};
function xml(bytes: Buffer | undefined, open: (tag: SaxesTagNS) => void, text = (_: string) => {}, close = (_: SaxesTagNS) => {}) {
  if (!bytes) return fail();
  const parser = new SaxesParser({ xmlns: true });
  let depth = 0, nodes = 0;
  parser.on('doctype', () => fail('archive_rejected'));
  parser.on('error', () => fail());
  parser.on('opentag', tag => { if (++depth > 64 || ++nodes > 200000) fail('parser_limit'); open(tag); });
  parser.on('text', text); parser.on('cdata', text);
  parser.on('closetag', tag => { close(tag); depth--; });
  const encoding = bytes[0] === 255 && bytes[1] === 254 ? 'utf-16le' : bytes[0] === 254 && bytes[1] === 255 ? 'utf-16be' : 'utf-8';
  const source = new TextDecoder(encoding, { fatal: true }).decode(bytes);
  parser.write(source).close();
}
/** Hyperlinks are inert text destinations, not resources to fetch during preview.
 * Conversion has no network access; the PDF viewer renders no link actions. */
function externalHyperlink(tag: SaxesTagNS): boolean {
  if (attribute(tag, 'TargetMode') !== 'External'
    || !['http://schemas.openxmlformats.org/officeDocument/2006/relationships/hyperlink',
      'http://purl.oclc.org/ooxml/officeDocument/relationships/hyperlink'].includes(attribute(tag, 'Type') ?? '')) return false;
  try { return ['https:', 'http:', 'mailto:'].includes(new URL(attribute(tag, 'Target') ?? '').protocol); }
  catch { return false; }
}
function relationships(files: Map<string, Buffer>, owner: string) {
  const path = owner ? `${posix.dirname(owner)}/_rels/${posix.basename(owner)}.rels` : '_rels/.rels';
  const links = new Map<string, { path: string; type: string }>();
  xml(files.get(path), tag => {
    if (tag.local !== 'Relationship') return;
    if (externalHyperlink(tag)) return;
    const id = attribute(tag, 'Id'), target = attribute(tag, 'Target'), type = attribute(tag, 'Type');
    if (!id || !target || !type || links.has(id) || attribute(tag, 'TargetMode') === 'External' || /[\\:%?#]/.test(target)) fail('archive_rejected');
    const resolved = posix.normalize(target.startsWith('/') ? target.slice(1) : posix.join(posix.dirname(owner), target));
    if (resolved.startsWith('../') || resolved.startsWith('/') || !files.has(resolved)) fail('archive_rejected');
    links.set(id, { path: resolved, type });
  });
  return links;
}

/** Parser-only worker entry: rejects active/external resource relationships for
 * Office document formats. No formula engine, scripts, filesystem or network IO. */
export function inspectOoxml(bytes: Uint8Array, extension: 'docx' | 'pptx'): void {
  const files = admitOoxml(bytes);
  for (const [path, data] of files) {
    if (/vba|activex|embeddings|externallinks/i.test(path)) fail('archive_rejected');
    if (/\.(?:xml|rels)$/i.test(path)) xml(data, tag => {
      if (tag.local === 'Relationship' && ((attribute(tag, 'TargetMode') !== undefined && attribute(tag, 'TargetMode') !== 'Internal')
        || /[\\:%?#]/.test(attribute(tag, 'Target') ?? '') || /(?:vba|oleObject|activeX|externalLink|attachedTemplate)/i.test(attribute(tag, 'Type') ?? ''))
        && !externalHyperlink(tag)) fail('archive_rejected');
      if (['Default', 'Override'].includes(tag.local) && /macroEnabled|vbaProject|activeX/i.test(attribute(tag, 'ContentType') ?? '')) fail('archive_rejected');
    });
  }
  const main = [...relationships(files, '').values()].filter(link => link.type.endsWith('/officeDocument'));
  if (main.length !== 1) fail();
  let root = '';
  xml(files.get(main[0].path), tag => { if (!root) root = tag.local; });
  if (root !== ({ docx: 'document', pptx: 'presentation' } as const)[extension]) fail();
}
