/** Product Host parsing policies, independent of browser rendering. */
export const PARSER_TIMEOUT_MS = 15000;
export const WORKBOOK_LIMITS = Object.freeze({
  sheets: 16, rows: 2000, columns: 128, cells: 20000, sharedStrings: 20000,
  stringCharacters: 1024 * 1024, cellCharacters: 4096, modelBytes: 2 * 1024 * 1024,
});
export const OFFICE_LIMITS = Object.freeze({
  memory: 512 * 1024 * 1024, tasks: 64, writableBytes: 64 * 1024 * 1024,
  runtimeSeconds: 15, fileBytes: 8 * 1024 * 1024, fileDescriptors: 128,
});
