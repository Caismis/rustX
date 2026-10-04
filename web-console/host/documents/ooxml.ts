import { SaxesParser, type SaxesTagNS } from 'saxes';
import { posix } from 'node:path';
import { admitOoxml } from './archive.ts';
import { DOCUMENT_LIMITS as limits, type WorkbookCell, type WorkbookPreview } from '../../src/client/document-types.ts';

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
function relationships(files: Map<string, Buffer>, owner: string) {
  const path = owner ? `${posix.dirname(owner)}/_rels/${posix.basename(owner)}.rels` : '_rels/.rels';
  const links = new Map<string, { path: string; type: string }>();
  xml(files.get(path), tag => {
    if (tag.local !== 'Relationship') return;
    const id = attribute(tag, 'Id'), target = attribute(tag, 'Target'), type = attribute(tag, 'Type');
    if (!id || !target || !type || links.has(id) || attribute(tag, 'TargetMode') === 'External' || /[\\:%?#]/.test(target)) fail('archive_rejected');
    const resolved = posix.normalize(target.startsWith('/') ? target.slice(1) : posix.join(posix.dirname(owner), target));
    if (resolved.startsWith('../') || resolved.startsWith('/') || !files.has(resolved)) fail('archive_rejected');
    links.set(id, { path: resolved, type });
  });
  return links;
}

/** Parser-only worker entry: rejects active/external package relationships for
 * all three OOXML formats. No formula engine, scripts, filesystem or network IO. */
export function inspectOoxml(bytes: Uint8Array, extension: 'docx' | 'pptx' | 'xlsx'): WorkbookPreview | undefined {
  const files = admitOoxml(bytes);
  for (const [path, data] of files) {
    if (/vba|activex|embeddings|externallinks/i.test(path)) fail('archive_rejected');
    if (/\.(?:xml|rels)$/i.test(path)) xml(data, tag => {
      if (tag.local === 'Relationship' && ((attribute(tag, 'TargetMode') !== undefined && attribute(tag, 'TargetMode') !== 'Internal')
        || /[\\:%?#]/.test(attribute(tag, 'Target') ?? '') || /(?:vba|oleObject|activeX|externalLink|attachedTemplate)/i.test(attribute(tag, 'Type') ?? ''))) fail('archive_rejected');
      if (tag.local === 'Override' && /macroEnabled|vbaProject|activeX/i.test(attribute(tag, 'ContentType') ?? '')) fail('archive_rejected');
    });
  }
  const main = [...relationships(files, '').values()].filter(link => link.type.endsWith('/officeDocument'));
  if (main.length !== 1) fail();
  let root = '';
  xml(files.get(main[0].path), tag => { if (!root) root = tag.local; });
  if (root !== ({ docx: 'document', pptx: 'presentation', xlsx: 'workbook' } as const)[extension]) fail();
  if (extension !== 'xlsx') return;
  const links = relationships(files, main[0].path);
  const strings: string[] = [];
  let stringSize = 0, current: string | undefined, sharedText = false;
  const shared = [...links.values()].find(link => link.type.endsWith('/sharedStrings'));
  if (shared) xml(files.get(shared.path), tag => { if (tag.local === 'si') current = ''; if (tag.local === 't') sharedText = true; }, text => {
    if (current !== undefined && sharedText) {
      current += text; stringSize += text.length;
      if (current.length > limits.cellCharacters || stringSize > limits.stringCharacters) fail('parser_limit');
    }
  }, tag => {
    if (tag.local === 't') sharedText = false;
    if (tag.local === 'si') { if (strings.length >= limits.sharedStrings) fail('parser_limit'); strings.push(current ?? ''); current = undefined; }
  });
  const sheets: { name: string; path: string }[] = [];
  xml(files.get(main[0].path), tag => {
    if (tag.local !== 'sheet') return;
    const name = attribute(tag, 'name'), id = attribute(tag, 'id'), link = id ? links.get(id) : undefined;
    if (!name || name.length > 128 || !link?.type.endsWith('/worksheet')) fail();
    if (sheets.length >= limits.sheets) fail('parser_limit');
    sheets.push({ name, path: link.path });
  });
  if (!sheets.length) fail();
  let total = 0;
  const result: WorkbookPreview = { kind: 'xlsx', sheets: sheets.map(sheet => {
    const cells: WorkbookCell[] = [], seen = new Set<string>();
    let cell: WorkbookCell | undefined, field: 'value' | 'formula' | undefined, truncated = false;
    xml(files.get(sheet.path), tag => {
      if (tag.local === 'c') {
        const address = attribute(tag, 'r') ?? '';
        const match = /^([A-Z]{1,3})([1-9][0-9]{0,6})$/.exec(address);
        if (!match || seen.has(address)) fail();
        seen.add(address);
        const column = [...match[1]].reduce((n, c) => n * 26 + c.charCodeAt(0) - 64, 0);
        if (+match[2] > limits.rows || column > limits.columns || total >= limits.cells) { truncated = true; cell = undefined; }
        else { total++; cell = { address, type: attribute(tag, 't') ?? 'n' }; }
      }
      if (cell && ['v', 't', 'f'].includes(tag.local)) field = tag.local === 'f' ? 'formula' : 'value';
    }, text => {
      if (cell && field) { cell[field] = (cell[field] ?? '') + text; if (cell[field]!.length > limits.cellCharacters) fail('parser_limit'); }
    }, tag => {
      if (['v', 't', 'f'].includes(tag.local)) field = undefined;
      if (tag.local === 'c' && cell) {
        if (cell.type === 's') {
          if (!/^\d+$/.test(cell.value ?? '') || strings[Number(cell.value)] === undefined) fail();
          cell.value = strings[Number(cell.value)];
        }
        cells.push(cell); cell = undefined;
      }
    });
    return { name: sheet.name, cells, truncated };
  }) };
  if (Buffer.byteLength(JSON.stringify(result)) > limits.modelBytes) fail('parser_limit');
  return result;
}
