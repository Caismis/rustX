// @vitest-environment node
import { readFileSync } from 'node:fs';
import { beforeEach, expect, it, vi } from 'vitest';
import { inflateSync, unzipSync, zipSync } from 'fflate';
import ExcelJS from 'exceljs';
import { zip } from './document-fixtures';
import { XlsxPreviewArchive } from '../src/app/components/workbench-documents/excel/xlsx-archive';
import { convertExcel } from '../src/app/components/workbench-documents/excel/convert';

vi.mock('fflate', async importOriginal => {
  const actual = await importOriginal<typeof import('fflate')>();
  return { ...actual, inflateSync: vi.fn(actual.inflateSync) };
});
beforeEach(() => vi.clearAllMocks());
const limits = { maxBytes: 16 * 1024 * 1024, maxCells: 250000, timeoutMs: 15000 };
const source = () => Uint8Array.from(readFileSync('test/fixtures/documents/sample.xlsx'));
const archive = (bytes: Uint8Array) => new XlsxPreviewArchive(Uint8Array.from(bytes), limits.maxCells);

it('rejects excessive entry count before inflating any member', () => {
  const bytes = zip(Array.from({ length: 10001 }, (_, i) => ({ name: `part${i}`, data: '' })));
  expect(() => archive(bytes)).toThrow();
  expect(inflateSync).not.toHaveBeenCalled();
});

it('rejects excessive declared entry size before allocating its output', () => {
  expect(() => archive(zip([{ name: 'unused', data: 'x', size: 32 * 1024 * 1024 + 1 }]))).toThrow();
  expect(inflateSync).not.toHaveBeenCalled();
});

it('rejects aggregate expansion before inflating the first member', () => {
  // Harmless fake DEFLATE payloads: each declaration fits the entry/ratio bounds.
  const bytes = zip(Array.from({ length: 3 }, (_, i) => ({ name: `part${i}`, data: 'x'.repeat(170000), size: 32 * 1024 * 1024 })));
  let at = bytes.readUInt32LE(bytes.length - 6);
  for (let i = 0; i < 3; i++) {
    bytes.writeUInt16LE(8, bytes.readUInt32LE(at + 42) + 8);
    bytes.writeUInt16LE(8, at + 10);
    at += 46 + bytes.readUInt16LE(at + 28);
  }
  expect(() => archive(bytes)).toThrow();
  expect(inflateSync).not.toHaveBeenCalled();
});

it('rejects a small unused high-ratio member before ExcelJS loading', async () => {
  const files = unzipSync(source());
  files['unused.txt'] = new Uint8Array(64000).fill(65);
  const bytes = zipSync(files);
  const load = vi.spyOn(ExcelJS.Workbook.prototype, 'xlsx', 'get');
  await expect(convertExcel(bytes, 'xlsx', limits)).rejects.toMatchObject({ code: 'invalid' });
  expect(inflateSync).not.toHaveBeenCalled();
  expect(load).not.toHaveBeenCalled();
});

it.each([0, 16])('rejects actual expansion beyond declared size %s with a fixed output buffer', size => {
  // Only 64 KiB actually expands; both headers understate it consistently.
  expect(() => archive(zip([{ name: 'unused', data: 'x'.repeat(64000), compress: true, size }]))).toThrow();
  expect(inflateSync).toHaveBeenCalledOnce();
  expect(vi.mocked(inflateSync).mock.calls[0][1]?.out?.byteLength).toBe(size + 1);
});

it('rejects expansion shorter than the declared size and CRC corruption', () => {
  expect(() => archive(zip([{ name: 'unused', data: 'hello', compress: true, size: 6 }]))).toThrow();
  const corrupted = zip([{ name: 'unused', data: 'hello', compress: true }]);
  corrupted.writeUInt32LE(123, 14);
  corrupted.writeUInt32LE(123, corrupted.readUInt32LE(corrupted.length - 6) + 16);
  expect(() => archive(corrupted)).toThrow();
});

it('preserves a normal workbook, formula caches and original bytes', async () => {
  const bytes = source(), original = bytes.slice();
  const preview = await convertExcel(bytes, 'xlsx', limits);
  expect(preview.sheets[0].celldata?.some(cell => cell.v?.v === 99)).toBe(true);
  expect(bytes).toEqual(original);
});

it.each([
  ['encrypted entry', (bytes: Buffer, at: number) => { bytes.writeUInt16LE(1, 6); bytes.writeUInt16LE(1, at + 8); }],
  ['disagreeing local size', (bytes: Buffer) => { bytes.writeUInt32LE(1, 22); }],
  ['disagreeing local name', (bytes: Buffer) => { bytes[30] ^= 1; }],
  ['unindexed local bytes', (bytes: Buffer, at: number) => { bytes.writeUInt32LE(1, at + 42); }],
  ['unsupported compression', (bytes: Buffer, at: number) => { bytes.writeUInt16LE(9, 8); bytes.writeUInt16LE(9, at + 10); }],
] as const)('rejects %s before inflation', (_name, corrupt) => {
  const bytes = zip([{ name: 'part', data: 'hello', compress: true }]);
  corrupt(bytes, bytes.readUInt32LE(bytes.length - 6));
  expect(() => archive(bytes)).toThrow('archive_rejected');
  expect(inflateSync).not.toHaveBeenCalled();
});

it.each([['same', 'SAME'], ['a', 'a/b'], ['a/b', 'a'], ['a/', 'A']])('rejects ambiguous package names %j', (...names) => {
  expect(() => archive(zip(names.map(name => ({ name, data: '' }))))).toThrow('archive_rejected');
  expect(inflateSync).not.toHaveBeenCalled();
});

it.each([1, 0x7075])('rejects alternate ZIP64/Unicode metadata %s', id => {
  const original = zip([{ name: 'part', data: 'hello', compress: true }]);
  const directory = original.readUInt32LE(original.length - 6);
  const field = Buffer.alloc(4); field.writeUInt16LE(id);
  const bytes = Buffer.concat([original.subarray(0, directory + 50), field, original.subarray(directory + 50)]);
  bytes.writeUInt16LE(4, directory + 30);
  bytes.writeUInt32LE(54, bytes.length - 10);
  expect(() => archive(bytes)).toThrow('archive_rejected');
  expect(inflateSync).not.toHaveBeenCalled();
});

it.each([false, true])('admits an ordinary streaming ZIP descriptor (signature: %s)', signed => {
  const original = zip([{ name: 'part', data: 'hello', compress: true }]);
  const directory = original.readUInt32LE(original.length - 6);
  const descriptor = Buffer.alloc(signed ? 16 : 12);
  if (signed) descriptor.writeUInt32LE(0x08074b50);
  original.copy(descriptor, signed ? 4 : 0, 14, 26);
  const bytes = Buffer.concat([original.subarray(0, directory), descriptor, original.subarray(directory)]);
  bytes.writeUInt16LE(8, 6); bytes.fill(0, 14, 26);
  bytes.writeUInt16LE(8, directory + descriptor.length + 8);
  bytes.writeUInt32LE(directory + descriptor.length, bytes.length - 6);
  const preview = archive(bytes).withoutDrawings();
  expect(new TextDecoder().decode(unzipSync(preview).part)).toBe('hello');
  expect(new DataView(preview.buffer, preview.byteOffset).getUint16(8, true)).toBe(0);
});

it('preserves ExcelJS-generated folders, Unicode, styling and cached formulas while omitting drawings', async () => {
  const workbook = new ExcelJS.Workbook(), sheet = workbook.addWorksheet('工作表');
  sheet.getCell('A1').value = { formula: '1+1', result: 2 };
  sheet.getCell('A1').font = { bold: true };
  sheet.getCell('B1').value = { text: 'Documentation', hyperlink: 'https://example.invalid/' };
  const image = workbook.addImage({ base64: 'data:image/png;base64,iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAQAAAC1HAwCAAAAC0lEQVR42mP8/x8AAwMCAO+aT9sAAAAASUVORK5CYII=', extension: 'png' });
  sheet.addImage(image, 'C1:D2');
  const bytes = new Uint8Array(await workbook.xlsx.writeBuffer());
  const preview = await convertExcel(bytes, 'xlsx', limits);
  expect(preview.sheets[0].name).toBe('工作表');
  expect(preview.sheets[0].celldata?.find(cell => cell.r === 0 && cell.c === 0)?.v).toMatchObject({ v: 2, f: '=1+1', bl: 1 });
  expect(preview.unsupportedFeatures).toContain('images');
});
