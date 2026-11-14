import type {
  Request1,
  WorkflowFileDocument,
  WorkflowRunSnapshotChunk,
  WorkflowRunSnapshotHeader,
} from '../../../../../protocol/app-server/v78';

export type { WorkflowRunSnapshotChunk, WorkflowRunSnapshotHeader };

/** Maximum bytes per run-snapshot chunk transfer, matching native WORKFLOW_SNAPSHOT_CHUNK_BYTES. */
export const WORKFLOW_SNAPSHOT_CHUNK_BYTES = 65_536;

/** Upper byte limit on normalized Workflow run snapshot documents (matches 32 MiB wire cap). */
export const MAX_WORKFLOW_SNAPSHOT_BYTES = 32 * 1024 * 1024;

export type ConversationWorkflowRunSnapshotHeaderParams = Extract<
  Request1,
  { method: 'conversation/workflowRunSnapshotHeader' }
>['params'];

export type ConversationWorkflowRunSnapshotChunkParams = Extract<
  Request1,
  { method: 'conversation/workflowRunSnapshotChunk' }
>['params'];

export type SessionWorkflowRunSnapshotHeaderParams = Extract<
  Request1,
  { method: 'session/workflowRunSnapshotHeader' }
>['params'];

export type SessionWorkflowRunSnapshotChunkParams = Extract<
  Request1,
  { method: 'session/workflowRunSnapshotChunk' }
>['params'];

export interface WorkflowRunSnapshotTransport {
  readHeader(current: () => boolean): Promise<WorkflowRunSnapshotHeader | null>;
  readChunk(offset: number, limit: number, current: () => boolean): Promise<WorkflowRunSnapshotChunk | null>;
}

export type WorkflowRunSnapshotLoadResult =
  | {
    readonly status: 'ready';
    readonly header: WorkflowRunSnapshotHeader;
    readonly document: WorkflowFileDocument;
  }
  | {
    readonly status: 'unavailable';
    readonly reason: 'missing_snapshot' | 'unsupported' | 'empty';
    readonly message?: string;
  }
  | { readonly status: 'corrupt'; readonly message: string }
  | { readonly status: 'expired'; readonly message: string }
  | { readonly status: 'denied'; readonly message: string }
  | { readonly status: 'error'; readonly message: string }
  | { readonly status: 'cancelled' };
