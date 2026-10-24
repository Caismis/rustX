// @vitest-environment node
import { readFileSync } from 'node:fs';
import { zip } from './document-fixtures';
import { expect, it } from 'vitest';
import { admitOoxml, OOXML_LIMITS } from '../host/documents/archive';
import { inspectOoxml } from '../host/documents/ooxml';
import { documentKind } from '../shared/documents.ts';

const fixture = (ext: string) => readFileSync(new URL(`./fixtures/documents/sample.${ext}`, import.meta.url));
it('uses a closed extension matrix, never MIME/basename authority', () => {
  for (const [name, kind] of [['a.PDF', 'pdf'], ['a.docx', 'docx'], ['a.pptx', 'pptx'], ['a.xlsx', 'xlsx'], ['a.htm', 'html'], ['a.html', 'html']]) expect(documentKind(name)).toBe(kind);
  for (const extension of ['doc', 'ppt', 'xls', 'xlsm', 'csv', 'zip', 'exe']) expect(documentKind(`file.${extension}`)).toBeUndefined();
});
it('preflights real Office formats and rejects a mismatched container', () => {
  expect(inspectOoxml(fixture('docx'), 'docx')).toBeUndefined();
  expect(inspectOoxml(fixture('pptx'), 'pptx')).toBeUndefined();
  expect(() => inspectOoxml(fixture('docx'), 'pptx')).toThrow('malformed');
});
it.each(['docx', 'pptx'] as const)('admits ordinary hyperlinks and printer settings in %s', extension => {
  const bytes = readFileSync(new URL(`./fixtures/documents/linked.${extension}`, import.meta.url));
  expect(() => inspectOoxml(bytes, extension)).not.toThrow();
});
it.each(['http://example.invalid/a?q=1#anchor', 'https://example.invalid/%20?q=1', 'mailto:reader@example.invalid'])('admits inert hyperlink %s', target => {
  const original = admitOoxml(fixture('docx'));
  original.set('word/_rels/document.xml.rels', Buffer.from(`<Relationships><Relationship Id="link" TargetMode="External" Target="${target}" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/hyperlink"/></Relationships>`));
  expect(() => inspectOoxml(zip([...original].map(([name, data]) => ({ name, data: data.toString() }))), 'docx')).not.toThrow();
});
it.each([
  '<Default Extension="bin" ContentType="application/vnd.ms-office.vbaProject"/>',
  '<Override PartName="/word/printerSettings/printerSettings1.bin" ContentType="application/vnd.ms-office.activeX"/>',
])('rejects active binary content disguised as printer settings: %s', declaration => {
  const original = admitOoxml(fixture('docx'));
  original.set('word/printerSettings/printerSettings1.bin', Buffer.from('binary payload'));
  original.set('[Content_Types].xml', Buffer.from(original.get('[Content_Types].xml')!.toString().replace('</Types>', `${declaration}</Types>`)));
  expect(() => inspectOoxml(zip([...original].map(([name, data]) => ({ name, data: data.toString() }))), 'docx')).toThrow('archive_rejected');
});
it.each([
  ['hyperlink', 'file:///etc/passwd'], ['hyperlink', 'javascript:alert(1)'], ['hyperlink', 'data:text/html,test'],
  ['image', 'https://example.invalid/image.png'], ['attachedTemplate', 'https://example.invalid/template.dotx'],
  ['externalLink', 'https://example.invalid/book.xlsx'], ['oleObject', 'https://example.invalid/object.bin'],
])('rejects external %s resource %s', (type, target) => {
  const original = admitOoxml(fixture('docx'));
  original.set('word/_rels/document.xml.rels', Buffer.from(`<Relationships><Relationship Id="link" TargetMode="External" Target="${target}" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/${type}"/></Relationships>`));
  expect(() => inspectOoxml(zip([...original].map(([name, data]) => ({ name, data: data.toString() }))), 'docx')).toThrow('archive_rejected');
});
it.each(['../escape', '/absolute', 'a/../b', 'a\\b', 'a:b', 'a%2fb', 'a//b', 'nested.zip', 'vbaProject.bin'])('rejects archive path %s', name => {
  expect(() => admitOoxml(zip([{ name, data: 'x' }]))).toThrow('archive_rejected');
});
it('rejects duplicates, path conflicts, count, declared expansion, actual expansion and CRC corruption', () => {
  for (const names of [['same', 'SAME'], ['a', 'a/b']]) expect(() => admitOoxml(zip(names.map(name => ({ name, data: 'x' }))))).toThrow('archive_rejected');
  expect(() => admitOoxml(zip(Array.from({ length: OOXML_LIMITS.entries + 1 }, (_, i) => ({ name: `x${i}`, data: '' }))))).toThrow('archive_rejected');
  expect(() => admitOoxml(zip([{ name: 'x', data: 'x', size: OOXML_LIMITS.entry + 1 }]))).toThrow('archive_rejected');
  expect(() => admitOoxml(zip([{ name: 'x', data: 'x'.repeat(50000), compress: true, size: 10 }]))).toThrow('archive_rejected');
  const corrupted = fixture('xlsx'); corrupted[60] ^= 255;
  expect(() => admitOoxml(corrupted)).toThrow('archive_rejected');
});
it('rejects external relationships, macros, entity declarations and malformed directories', () => {
  const original = admitOoxml(fixture('docx'));
  const pack = (extra: Record<string, string>) => zip([...new Map([...original].map(([name, data]) => [name, data.toString()])).entries(), ...Object.entries(extra)].map(([name, data]) => ({ name, data })));
  expect(() => inspectOoxml(pack({ 'word/_rels/document.xml.rels': '<Relationships><Relationship Id="x" TargetMode="External" Target="https://example.invalid" Type="x"/></Relationships>' }), 'docx')).toThrow('archive_rejected');
  expect(() => inspectOoxml(pack({ 'word/activeX/active.xml': '<x/>' }), 'docx')).toThrow('archive_rejected');
  expect(() => inspectOoxml(pack({ 'word/custom.xml': '<!DOCTYPE x [<!ENTITY boom "boom">]><x>&boom;</x>' }), 'docx')).toThrow('archive_rejected');
  expect(() => admitOoxml(fixture('xlsx').subarray(0, 100))).toThrow('archive_rejected');
});
