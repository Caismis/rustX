/** Browser presentation admission; deadlines are cooperative watchdogs. */
export const PDF_LIMITS = Object.freeze({
  pdfPages: 100, canvasSide: 4096, canvasPixels: 4 * 1024 * 1024,
  scratchCanvases: 8, scratchPixels: 16 * 1024 * 1024,
  textItems: 10000, textCharacters: 100000, workers: 1, renders: 1,
  loadWatchdogMs: 15000, renderWatchdogMs: 15000,
});
