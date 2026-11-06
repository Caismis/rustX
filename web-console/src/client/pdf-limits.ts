import { PREVIEW_POLICY } from './preview-policy';
/** Browser presentation admission; deadlines are cooperative watchdogs. */
export const PDF_LIMITS = Object.freeze({
  pdfPages: 100, canvasSide: 4096, canvasPixels: 4 * 1024 * 1024,
  // PDF.js knockout text/groups use several small surfaces per nested group.
  // Keep their count finite without rejecting ordinary Office card layouts.
  scratchCanvases: 128, scratchPixels: 16 * 1024 * 1024,
  textItems: 10000, textCharacters: 100000, workers: PREVIEW_POLICY.pdfWorkers, renders: 1,
  loadWatchdogMs: 15000, renderWatchdogMs: 15000,
});
