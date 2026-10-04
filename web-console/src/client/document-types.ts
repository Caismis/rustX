import type { AttachmentTarget, SessionFileReference } from '../../../protocol/app-server/v34.ts';

export type DocumentKind = 'pdf' | 'docx' | 'pptx' | 'xlsx' | 'html';
export function documentKind(name: string): DocumentKind | undefined {
  const extension = name.split('.').pop()?.toLowerCase();
  if (extension === 'htm') return 'html';
  return extension && ['pdf', 'docx', 'pptx', 'xlsx', 'html'].includes(extension) ? extension as DocumentKind : undefined;
}
export type DocumentSource = { kind: 'artifact'; artifact_id: string }
  | { kind: 'session_file'; message_id: string; delivery_index: number };
export interface DocumentRequest { target: AttachmentTarget; source: DocumentSource; extension: 'docx' | 'pptx' | 'xlsx'; digest: string }
export interface WorkbookCell { address: string; value?: string; formula?: string; type: string }
export interface WorkbookSheet { name: string; cells: WorkbookCell[]; truncated: boolean }
export interface WorkbookPreview { kind: 'xlsx'; sheets: WorkbookSheet[] }
export type DerivedDocument = { kind: 'pdf'; data: string } | WorkbookPreview;
export interface DocumentResult { digest: string; file?: SessionFileReference; preview: DerivedDocument }
export const DOCUMENT_LIMITS = Object.freeze({
  pdfBytes: 4 * 1024 * 1024, pdfPages: 100, canvasSide: 4096, canvasPixels: 4 * 1024 * 1024,
  scratchCanvases: 8, scratchPixels: 16 * 1024 * 1024,
  textItems: 10000, textCharacters: 100000, workers: 1, renders: 1, timeout: 15000,
  sheets: 16, rows: 2000, columns: 128, cells: 20000, sharedStrings: 20000,
  stringCharacters: 1024 * 1024, cellCharacters: 4096, modelBytes: 2 * 1024 * 1024,
});
