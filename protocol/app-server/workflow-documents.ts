/** Native document scratch keeps complete Workflows out of control records.
 * Both Web and Product Host use this carrier; scope and CAS remain native. */
import type { MethodResult, Request1, SourceTarget, WorkflowDocumentRef, WorkflowFileDocument, WorkflowCheckReport, WorkflowSource as WireSource, WorkflowEdit as WireEdit } from './v72.ts';

export type WorkflowSource = Omit<WireSource, 'document'> & { document?: WorkflowFileDocument | null };
export type WorkflowEdit = Exclude<WireEdit, { operation: 'replace' }> | { operation: 'replace'; document: WorkflowFileDocument };
export type WorkflowRequest = <T extends MethodResult['type']>(operation: Exclude<Request1, { method: 'delivery/cancel' }>, expected: T) => Promise<Extract<MethodResult, { type: T }>>;
const LIMIT = 32 * 1024 * 1024; // Wire encoding allowance; native normalized documents remain 16 MiB.
const CHUNK = 65_536;
const encoder = new TextEncoder();
// One native connection owns all scratch capacity, even when separate ports or
// HTTP transactions wrap its request function. Hold admission through release,
// and encode only after admission so waiting analyses retain no wire copies.
const operations = new WeakMap<object, Promise<void>>();
export async function withWorkflowOperation<T>(connection: object, operation: () => Promise<T>, owns?: () => boolean): Promise<T> {
  const previous = operations.get(connection) ?? Promise.resolve();
  let release!: () => void;
  const next = new Promise<void>(resolve => { release = resolve; });
  operations.set(connection, next);
  await previous;
  try {
    if (owns && !owns()) throw new Error('Workflow analysis request superseded');
    return await operation();
  }
  finally {
    release();
    if (operations.get(connection) === next) operations.delete(connection);
  }
}
function encode(bytes: Uint8Array): string {
  let binary = '';
  for (const byte of bytes) binary += String.fromCharCode(byte);
  return btoa(binary);
}
function decode(data: string): Uint8Array {
  if (data.length > Math.ceil(CHUNK / 3) * 4) throw new Error('Workflow chunk exceeds 64 KiB');
  const binary = atob(data);
  if (binary.length > CHUNK) throw new Error('Workflow chunk exceeds 64 KiB');
  return Uint8Array.from(binary, character => character.charCodeAt(0));
}
async function digest(bytes: Uint8Array<ArrayBuffer>): Promise<string> {
  return [...new Uint8Array(await crypto.subtle.digest('SHA-256', bytes))].map(byte => byte.toString(16).padStart(2, '0')).join('');
}

async function readContent<T>(reference: { byte_length: number; sha256: string }, read: (offset: number) => Promise<{ offset: number; data: string; next_offset?: number | null }>): Promise<T> {
  if (!Number.isSafeInteger(reference.byte_length) || reference.byte_length <= 0) throw new Error('Invalid Workflow content length');
  const bytes = new Uint8Array(reference.byte_length);
  let offset = 0;
  do {
    const chunk = await read(offset);
    const data = decode(chunk.data), end = offset + data.length;
    if (chunk.offset !== offset || !data.length || end > bytes.length || (chunk.next_offset ?? null) !== (end < bytes.length ? end : null)) throw new Error('Invalid Workflow content continuation');
    bytes.set(data, offset); offset = end;
  } while (offset < bytes.length);
  if (await digest(bytes) !== reference.sha256) throw new Error('Workflow content integrity check failed');
  return JSON.parse(new TextDecoder('utf-8', { fatal: true }).decode(bytes)) as T;
}

/** Report hydration stays inside the candidate operation, before either native
 * reference is released. Derived reports do not inherit the source-byte cap. */
export async function checkWorkflowDocument(connection: object, request: WorkflowRequest, target: SourceTarget, id: string, document: WorkflowFileDocument, explain = false): Promise<WorkflowCheckReport> {
  return withWorkflowDocument(connection, request, target, document, async candidate => {
    const { report } = await request({ method: 'workflow/check', params: { target, id, document: candidate, explain } }, 'workflow_check');
    try {
      return await readContent<WorkflowCheckReport>(report, async offset => (await request({ method: 'workflow/checkReportRead', params: { target, reference: report, offset, max_bytes: CHUNK } }, 'workflow_check_report_chunk')).chunk);
    } finally {
      await request({ method: 'workflow/checkReportRelease', params: { target, reference: report } }, 'workflow_check_report_released').catch(() => {});
    }
  });
}
export async function withWorkflowDocument<T>(connection: object, request: WorkflowRequest, target: SourceTarget, document: WorkflowFileDocument, operation: (reference: WorkflowDocumentRef) => Promise<T>, owns?: () => boolean): Promise<T> {
  return withWorkflowOperation(connection, async () => {
    const bytes = encoder.encode(JSON.stringify(document));
    if (bytes.length > LIMIT) throw new Error('Workflow document transfer exceeds 32 MiB; normalized documents must fit 16 MiB');
    const sha256 = await digest(bytes);
    if (owns && !owns()) throw new Error('Workflow analysis request superseded');
    const { reference } = await request({ method: 'workflow/documentBegin', params: { target, byte_length: bytes.length, sha256 } }, 'workflow_document');
    try {
      for (let offset = 0; offset < bytes.length; offset += CHUNK) {
        await request({ method: 'workflow/documentWrite', params: { target, reference, offset, data: encode(bytes.subarray(offset, offset + CHUNK)) } }, 'workflow_document_accepted');
      }
      await request({ method: 'workflow/documentFinish', params: { target, reference } }, 'workflow_document_accepted');
      if (owns && !owns()) throw new Error('Workflow analysis request superseded');
      return await operation(reference);
    } finally {
      // Release is connection-local scratch cleanup. Failure cannot turn an
      // acknowledged source commit into an uncertain write; disconnect owns it.
      await request({ method: 'workflow/documentRelease', params: { target, reference } }, 'workflow_document_accepted').catch(() => {});
    }
  }, owns);
}
export async function readWorkflowSource(connection: object, request: WorkflowRequest, target: SourceTarget, id: string): Promise<WorkflowSource> {
  return withWorkflowOperation(connection, async () => {
    const { source } = await request({ method: 'workflow/read', params: { target, id } }, 'workflow_source');
    const reference = source.document;
    if (!reference) return { ...source, document: null };
    try {
      if (reference.byte_length > LIMIT) throw new Error('Workflow document transfer exceeds 32 MiB; normalized documents must fit 16 MiB');
      const document = await readContent<WorkflowFileDocument>(reference, async offset => (await request({ method: 'workflow/documentRead', params: { target, reference, offset, max_bytes: CHUNK } }, 'workflow_document_chunk')).chunk);
      return { ...source, document };
    } finally {
      await request({ method: 'workflow/documentRelease', params: { target, reference } }, 'workflow_document_accepted').catch(() => {});
    }
  });
}
