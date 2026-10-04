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
it('parses real OOXML and preserves the deliberately stale cached value without recalculation', () => {
  expect(inspectOoxml(fixture('docx'), 'docx')).toBeUndefined();
  expect(inspectOoxml(fixture('pptx'), 'pptx')).toBeUndefined();
  const workbook = inspectOoxml(fixture('xlsx'), 'xlsx')!;
  expect(workbook.sheets.map(s => s.name)).toEqual(['Values', '第二页']);
  expect(workbook.sheets[0].cells[2]).toEqual({ address: 'C1', type: 'n', formula: 'A1+B1', value: '99' });
  expect(workbook.sheets[0].cells[3]).toEqual({ address: 'D1', type: 'n', formula: 'NOW()' });
  expect(() => inspectOoxml(fixture('docx'), 'xlsx')).toThrow('malformed');
});
it.each(['../escape', '/absolute', 'a/../b', 'a\\b', 'a:b', 'a%2fb', 'a//b', 'nested.zip', 'vbaProject.bin'])('rejects archive path %s', name => {
  expect(() => admitOoxml(zip([{ name, data: 'x' }]))).toThrow('archive_rejected');
});
it('rejects duplicates, path conflicts, count, declared expansion, actual expansion and CRC corruption', () => {
  for (const names of [['same', 'SAME'], ['a', 'a/b']]) expect(() => admitOoxml(zip(names.map(name => ({ name, data: 'x' }))))).toThrow('archive_rejected');
  expect(() => admitOoxml(zip(Array.from({ length: 257 }, (_, i) => ({ name: `x${i}`, data: '' }))))).toThrow('archive_rejected');
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
