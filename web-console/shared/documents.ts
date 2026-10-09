import type { AttachmentTarget, SessionFileReference } from '../../protocol/app-server/v38.ts';

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

/** Maximum derived PDF carrier payload, enforced by producer and consumer. */
export const DERIVED_PDF_MAX_BYTES = 4 * 1024 * 1024;
