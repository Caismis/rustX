import type {
  AttachmentTarget,
  CheckpointReadCut,
  WorkflowFileDocument,
  WorkflowRunId,
} from '../../../../../protocol/app-server/v78';
import { workflowKey } from '../../../bindings/workflow-children';
import { RequestNotDispatched, RpcFailure } from '../../../client/errors';
import { organizeDocument } from '../ui/editor/layout';
import {
  MAX_WORKFLOW_SNAPSHOT_BYTES,
  WORKFLOW_SNAPSHOT_CHUNK_BYTES,
  type ConversationWorkflowRunSnapshotChunkParams,
  type ConversationWorkflowRunSnapshotHeaderParams,
  type SessionWorkflowRunSnapshotChunkParams,
  type SessionWorkflowRunSnapshotHeaderParams,
  type WorkflowRunSnapshotChunk,
  type WorkflowRunSnapshotHeader,
  type WorkflowRunSnapshotLoadResult,
  type WorkflowRunSnapshotTransport,
} from './contract';
import { sameWorkflowRunId } from './identity';

export type SnapshotRequestInvoker = <TResponse>(
  method:
    | 'conversation/workflowRunSnapshotHeader'
    | 'conversation/workflowRunSnapshotChunk'
    | 'session/workflowRunSnapshotHeader'
    | 'session/workflowRunSnapshotChunk',
  params:
    | ConversationWorkflowRunSnapshotHeaderParams
    | ConversationWorkflowRunSnapshotChunkParams
    | SessionWorkflowRunSnapshotHeaderParams
    | SessionWorkflowRunSnapshotChunkParams,
  expectedType: string,
  current: () => boolean,
) => Promise<TResponse>;

export function createLiveSnapshotTransport(
  request: SnapshotRequestInvoker,
  params: { target: AttachmentTarget; agentId?: string | null; cut?: CheckpointReadCut | null; runId: WorkflowRunId },
): WorkflowRunSnapshotTransport {
  const base: ConversationWorkflowRunSnapshotHeaderParams = {
    target: params.target,
    agent_id: params.agentId ?? null,
    cut: params.cut ?? null,
    run_id: params.runId,
  };
  return {
    async readHeader(current) {
      const response = await request<{ header?: WorkflowRunSnapshotHeader | null }>(
        'conversation/workflowRunSnapshotHeader',
        base,
        'workflow_run_snapshot_header',
        current,
      );
      return response.header ?? null;
    },
    async readChunk(offset, limit, current) {
      const response = await request<{ chunk?: WorkflowRunSnapshotChunk | null }>(
        'conversation/workflowRunSnapshotChunk',
        { ...base, offset, limit },
        'workflow_run_snapshot_chunk',
        current,
      );
      return response.chunk ?? null;
    },
  };
}

export function createHistorySnapshotTransport(
  request: SnapshotRequestInvoker,
  params: { sessionId: string; nodeId?: string | null; cut: CheckpointReadCut; runId: WorkflowRunId },
): WorkflowRunSnapshotTransport {
  const base: SessionWorkflowRunSnapshotHeaderParams = {
    session_id: params.sessionId,
    node_id: params.nodeId ?? null,
    cut: params.cut,
    run_id: params.runId,
  };
  return {
    async readHeader(current) {
      const response = await request<{ header?: WorkflowRunSnapshotHeader | null }>(
        'session/workflowRunSnapshotHeader',
        base,
        'workflow_run_snapshot_header',
        current,
      );
      return response.header ?? null;
    },
    async readChunk(offset, limit, current) {
      const response = await request<{ chunk?: WorkflowRunSnapshotChunk | null }>(
        'session/workflowRunSnapshotChunk',
        { ...base, offset, limit },
        'workflow_run_snapshot_chunk',
        current,
      );
      return response.chunk ?? null;
    },
  };
}

function parseSafeInt(value: number): number | null {
  return typeof value === 'number' && Number.isSafeInteger(value) && value >= 0 ? value : null;
}

function decodeBase64Chunk(data: string): Uint8Array {
  if (typeof data !== 'string' || data.length > Math.ceil(WORKFLOW_SNAPSHOT_CHUNK_BYTES / 3) * 4) {
    throw new SnapshotCorruptionError('Snapshot chunk exceeds 64 KiB boundary.');
  }
  let binary: string;
  try {
    binary = atob(data);
  } catch {
    throw new SnapshotCorruptionError('Snapshot chunk is not valid base64.');
  }
  if (binary.length === 0 || binary.length > WORKFLOW_SNAPSHOT_CHUNK_BYTES) {
    throw new SnapshotCorruptionError('Snapshot chunk payload size is out of bounds.');
  }
  return Uint8Array.from(binary, character => character.charCodeAt(0));
}

async function sha256Hex(bytes: Uint8Array<ArrayBuffer>): Promise<string> {
  const digest = await crypto.subtle.digest('SHA-256', bytes);
  return [...new Uint8Array(digest)].map(byte => byte.toString(16).padStart(2, '0')).join('');
}

class SnapshotCorruptionError extends Error {
  constructor(message: string) {
    super(message);
    this.name = 'SnapshotCorruptionError';
  }
}

export function classifySnapshotError(error: unknown): Exclude<WorkflowRunSnapshotLoadResult, { status: 'ready' } | { status: 'cancelled' }> {
  if (error instanceof SnapshotCorruptionError) {
    return { status: 'corrupt', message: error.message };
  }
  if (error instanceof RequestNotDispatched) {
    return { status: 'unavailable', reason: 'unsupported', message: error.message };
  }
  if (error instanceof RpcFailure) {
    const code = error.error.code;
    const raw = `${error.error.message} ${error.message}`.toLowerCase();
    const kind = error.error.data?.kind;
    if (code === -32601 || raw.includes('method not found') || raw.includes('not implemented')) {
      return { status: 'unavailable', reason: 'unsupported', message: error.error.message };
    }
    if (kind === 'stale_attachment') {
      return { status: 'denied', message: error.error.message };
    }
    if (
      raw.includes('expired')
      || raw.includes('stale cut')
      || raw.includes('retired')
      || raw.includes('read cut')
      || raw.includes('read_cut')
      || kind === 'read_cut_expired'
    ) {
      return { status: 'expired', message: error.error.message };
    }
    if (
      raw.includes('denied')
      || raw.includes('forbidden')
      || raw.includes('unauthorized')
      || raw.includes('cross-conversation')
      || raw.includes('wrong owner')
      || raw.includes('access')
    ) {
      return { status: 'denied', message: error.error.message };
    }
    if (
      raw.includes('corrupt')
      || raw.includes('digest')
      || raw.includes('sha256')
      || raw.includes('invalid json')
      || raw.includes('malformed')
    ) {
      return { status: 'corrupt', message: error.error.message };
    }
    return { status: 'error', message: error.error.message };
  }
  const message = error instanceof Error ? error.message : String(error);
  const lower = message.toLowerCase();
  if (lower.includes('expired') || lower.includes('retired cut')) {
    return { status: 'expired', message };
  }
  if (lower.includes('denied') || lower.includes('unauthorized') || lower.includes('forbidden')) {
    return { status: 'denied', message };
  }
  if (lower.includes('corrupt') || lower.includes('sha256') || lower.includes('integrity')) {
    return { status: 'corrupt', message };
  }
  return { status: 'error', message };
}

function isWorkflowDocumentShape(value: unknown): value is WorkflowFileDocument {
  if (!value || typeof value !== 'object') return false;
  const doc = value as Record<string, unknown>;
  const block = doc.block as Record<string, unknown> | undefined;
  return !!block && typeof block === 'object' && !!block.nodes && typeof block.nodes === 'object';
}

export async function readWorkflowRunSnapshot(
  transport: WorkflowRunSnapshotTransport,
  expectedRunId: WorkflowRunId,
  current: () => boolean,
): Promise<WorkflowRunSnapshotLoadResult> {
  if (!current()) return { status: 'cancelled' };
  let header: WorkflowRunSnapshotHeader | null;
  try {
    header = await transport.readHeader(current);
  } catch (error) {
    if (!current()) return { status: 'cancelled' };
    return classifySnapshotError(error);
  }
  if (!current()) return { status: 'cancelled' };
  if (header == null) {
    return { status: 'unavailable', reason: 'missing_snapshot' };
  }
  if (!sameWorkflowRunId(header.run_id, expectedRunId)) {
    return { status: 'corrupt', message: 'Run snapshot identity does not match requested workflow run.' };
  }

  const byteLength = parseSafeInt(header.byte_length);
  if (byteLength === null || byteLength <= 0) {
    return { status: 'corrupt', message: 'Invalid snapshot byte length.' };
  }
  if (byteLength > MAX_WORKFLOW_SNAPSHOT_BYTES) {
    return { status: 'corrupt', message: 'Run snapshot exceeds maximum normalized byte limit.' };
  }
  if (!/^[a-f0-9]{64}$/i.test(header.sha256)) {
    return { status: 'corrupt', message: 'Invalid snapshot SHA-256 header digest.' };
  }

  const buffer = new ArrayBuffer(byteLength);
  const bytes = new Uint8Array(buffer);
  let offset = 0;

  try {
    while (offset < byteLength) {
      if (!current()) return { status: 'cancelled' };
      const limit = Math.min(WORKFLOW_SNAPSHOT_CHUNK_BYTES, byteLength - offset);
      const chunk = await transport.readChunk(offset, limit, current);
      if (!current()) return { status: 'cancelled' };
      if (chunk == null) {
        return { status: 'unavailable', reason: 'empty', message: 'Snapshot payload unavailable at chunk offset.' };
      }
      const chunkOffset = parseSafeInt(chunk.offset);
      if (chunkOffset !== offset) {
        throw new SnapshotCorruptionError('Snapshot chunk offset discontinuity.');
      }
      const decoded = decodeBase64Chunk(chunk.data);
      const end = offset + decoded.length;
      if (end > byteLength) {
        throw new SnapshotCorruptionError('Snapshot chunk overflows declared byte length.');
      }
      const nextOffset = chunk.next_offset === null || chunk.next_offset === undefined ? null : parseSafeInt(chunk.next_offset);
      const expectedNext = end < byteLength ? end : null;
      if (nextOffset !== expectedNext) {
        throw new SnapshotCorruptionError('Snapshot chunk continuation offset mismatch.');
      }
      bytes.set(decoded, offset);
      offset = end;
    }

    const digest = await sha256Hex(bytes);
    if (!current()) return { status: 'cancelled' };
    if (digest.toLowerCase() !== header.sha256.toLowerCase()) {
      throw new SnapshotCorruptionError('Snapshot content SHA-256 verification failed.');
    }

    const jsonText = new TextDecoder('utf-8', { fatal: true }).decode(bytes);
    const parsed = JSON.parse(jsonText) as unknown;
    if (!isWorkflowDocumentShape(parsed)) {
      throw new SnapshotCorruptionError('Snapshot document schema does not match admitted Workflow definition.');
    }
    return { status: 'ready', header, document: parsed };
  } catch (error) {
    if (!current()) return { status: 'cancelled' };
    return classifySnapshotError(error);
  }
}

const MAX_LAYOUT_CACHE_ENTRIES = 16;

export class RunPresentationCache {
  private readonly entries = new Map<string, WorkflowFileDocument>();

  clear() {
    this.entries.clear();
  }

  size() {
    return this.entries.size;
  }

  private cacheKey(header: Pick<WorkflowRunSnapshotHeader, 'run_id' | 'snapshot_digest' | 'sha256'>): string {
    return JSON.stringify([workflowKey(header.run_id), header.snapshot_digest, header.sha256]);
  }

  /** Uses persisted source coordinates when `has_source_presentation` is true;
   * otherwise lazily computes layout once per immutable snapshot identity without
   * mutating authoring drafts or persisting edits. */
  async resolve(
    header: WorkflowRunSnapshotHeader,
    document: WorkflowFileDocument,
    current: () => boolean,
    organizeFn: (doc: WorkflowFileDocument) => Promise<WorkflowFileDocument> = organizeDocument,
  ): Promise<{ document: WorkflowFileDocument; layoutComputed: boolean } | null> {
    if (!current()) return null;
    if (header.has_source_presentation) {
      return { document, layoutComputed: false };
    }
    const key = this.cacheKey(header);
    const hit = this.entries.get(key);
    if (hit) {
      // Refresh insertion order for bounded LRU eviction.
      this.entries.delete(key);
      this.entries.set(key, hit);
      return { document: hit, layoutComputed: false };
    }
    const clone = structuredClone(document);
    const organized = await organizeFn(clone);
    if (!current()) return null;
    if (this.entries.size >= MAX_LAYOUT_CACHE_ENTRIES) {
      const oldest = this.entries.keys().next().value;
      if (oldest !== undefined) this.entries.delete(oldest);
    }
    this.entries.set(key, organized);
    return { document: organized, layoutComputed: true };
  }
}
