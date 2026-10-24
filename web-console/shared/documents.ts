import type { AttachmentTarget, CanonicalContentLocator, SessionFileReference } from '../../protocol/app-server/v57.ts';
import { FILE_PREVIEW_LIMITS } from './file-preview.ts';

export type DocumentKind = 'pdf' | 'docx' | 'pptx' | 'xlsx' | 'html';
export function documentKind(name: string): DocumentKind | undefined {
  const extension = name.split('.').pop()?.toLowerCase();
  if (extension === 'htm') return 'html';
  return extension && ['pdf', 'docx', 'pptx', 'xlsx', 'html'].includes(extension) ? extension as DocumentKind : undefined;
}
export type DocumentSource = { kind: 'artifact'; artifact_id: string; agent_id?: string }
  | { kind: 'session_file'; locator: CanonicalContentLocator; delivery_index: number; agent_id?: string };
export interface DocumentRequest { target: AttachmentTarget; source: DocumentSource; extension: 'docx' | 'pptx' }
export type DerivedDocument = { kind: 'pdf'; data: Uint8Array<ArrayBuffer> };
export interface DocumentResult { digest: string; file?: SessionFileReference; preview: DerivedDocument }

/** Maximum derived PDF carrier payload, enforced by producer and consumer. */
export const DERIVED_PDF_MAX_BYTES = FILE_PREVIEW_LIMITS.convertedPdfBytes;
