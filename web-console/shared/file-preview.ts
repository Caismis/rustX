/** File content budgets are independent of the ordinary App Server record size. */
export const FILE_PREVIEW_LIMITS = Object.freeze({
  pageBytes: 2 * 1024 * 1024, pageLines: 5000, fileBytes: 32 * 1024 * 1024,
  officeBytes: 50 * 1024 * 1024, pdfBytes: 32 * 1024 * 1024,
  convertedPdfBytes: 100 * 1024 * 1024,
  spreadsheetBytes: 16 * 1024 * 1024, spreadsheetCells: 250000, parserTimeoutMs: 15000,
});
export type FileReadOptions = { kind: 'bytes'; range?: { offset: number; length: number } }
  | { kind: 'text'; offset: number; limit: number };
export interface FileBytes {
  data: Uint8Array<ArrayBuffer>;
  version: string;
  bytes: number;
  offset: number;
  eof: boolean;
  /** A line count marks a UTF-8 text page; absent for original byte reads. */
  lines?: number;
}

/** Typed arrays can come from another browser/Node realm. */
export function isFileData(value: unknown): value is Uint8Array<ArrayBuffer> {
  return ArrayBuffer.isView(value) && Object.prototype.toString.call(value) === '[object Uint8Array]'
    && Object.prototype.toString.call(value.buffer) === '[object ArrayBuffer]';
}

export function validateFileRead(read: FileReadOptions): void {
  if (!read || typeof read !== 'object') throw new Error('Invalid file read');
  if (read.kind === 'text') {
    if (Object.keys(read).some(key => !['kind', 'offset', 'limit'].includes(key))
      || !Number.isSafeInteger(read.offset) || read.offset < 1 || !Number.isSafeInteger(read.limit)
      || read.limit < 1 || read.limit > FILE_PREVIEW_LIMITS.pageLines || !Number.isSafeInteger(read.offset + read.limit)) throw new Error('Invalid text page');
  } else if (read.kind === 'bytes') {
    if (Object.keys(read).some(key => !['kind', 'range'].includes(key))) throw new Error('Invalid byte read');
    const range = read.range;
    if (range !== undefined && (!range || Object.keys(range).some(key => !['offset', 'length'].includes(key))
      || !Number.isSafeInteger(range.offset) || range.offset < 0 || !Number.isSafeInteger(range.length)
      || range.length < 1 || range.length > FILE_PREVIEW_LIMITS.pageBytes || !Number.isSafeInteger(range.offset + range.length))) throw new Error('Invalid byte range');
  } else throw new Error('Invalid file read kind');
}
