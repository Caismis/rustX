/** Product Host parsing policies, independent of browser rendering. */
export const PARSER_TIMEOUT_MS = 15000;
export const OFFICE_LIMITS = Object.freeze({
  memory: 512 * 1024 * 1024, tasks: 64, writableBytes: 256 * 1024 * 1024,
  runtimeSeconds: 60, fileBytes: 100 * 1024 * 1024, fileDescriptors: 128,
});
