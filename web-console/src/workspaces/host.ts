import type { SourceMutation, SourceSettings } from '../../../protocol/app-server/v37.ts';
import type { AttachmentTarget, SessionFileReference } from '../../../protocol/app-server/v37.ts';
import type { DocumentRequest, DocumentResult } from '../../shared/documents.ts';
export interface DeliveryRead { target: AttachmentTarget; message_id: string; delivery_index: number }
export interface DeliveryBytes { file: SessionFileReference; data: string }
export type WorkspaceConfigurationOperation = { kind: 'read' | 'reconcile' } | { kind: 'write'; expected_revision: string; mutation: SourceMutation };
/** The separate authoritative read attempted after a confirmed write. It may
 * succeed or fail without changing the fact that the write committed. */
export type WorkspaceConfigurationReread =
  | { status: 'observed'; projection: SourceSettings }
  | { status: 'failed'; error: unknown };
/** One confirmed native configuration mutation. The acknowledgement is exactly
 * the fact that this mutation committed and the revision it committed at; it is
 * never a projection, an application observation, or a read outcome. */
export interface WorkspaceConfigurationCommit {
  acknowledgement: SourceSettings;
  reread: WorkspaceConfigurationReread;
}
/** A Workspace configuration operation outcome: acknowledgement and
 * authoritative reread stay distinct facts for a write, so a failed reread can
 * never be mistaken for an uncommitted write. */
export type WorkspaceConfigurationResult =
  | { kind: 'read' | 'reconcile'; projection: SourceSettings }
  | { kind: 'write'; commit: WorkspaceConfigurationCommit };
/** Product Host contract. No rustX trust, configuration, or Session ownership. */
export interface ProductHostWorkspace { id: string; displayName: string; location: string; displayPath: string }
export interface WorkspaceAuthorityScope {
  /** Product Host process identity, independent of registration metadata. */
  readonly authorityId: string;
  /** With authorityId, the authority scope; compared only by normalized identity. */
  readonly endpoint: string;
}
export interface WorkspaceCatalog extends WorkspaceAuthorityScope {
  workspaces: ProductHostWorkspace[];
  picker: { kind: 'configured'; locations: { id: string; displayName: string }[] } | { kind: 'unavailable'; reason: string };
}
export type SessionLocation = { authorized: false; reason: 'denied' | 'unavailable' } | { authorized: true; workspaceId?: string };
/** Validate the current positional contract before any display or admission consumer. */
export function validateLocations(value: unknown, count: number): asserts value is SessionLocation[] {
  if (!Array.isArray(value) || value.length !== count || value.some(row => !row ||
    (row.authorized !== true && row.authorized !== false) ||
    (row.authorized === false && !['denied', 'unavailable'].includes(row.reason)) ||
    (row.authorized === true && row.workspaceId !== undefined && typeof row.workspaceId !== 'string'))) {
    throw new Error('Invalid Workspace classification');
  }
}
/** Typed Product Host failures stay independent of the browser client. */
export class WorkspaceHostError extends Error {
  readonly kind?: string;
  readonly uncertain: boolean;
  constructor(message: string, kind?: string, uncertain = false) {
    super(message);
    this.name = 'WorkspaceHostError';
    this.kind = kind;
    this.uncertain = uncertain;
  }
}
/** Private physical facts, independent of cancellation/publication intent.
 * A lost document carrier can conceal either its native reread or converter. */
export type SettlementFailureKind = 'file_settlement_unknown' | 'converter_settlement_unknown' | 'document_settlement_unknown';
export function settlementFailureKind(cause: unknown): SettlementFailureKind | undefined {
  if (!(cause instanceof WorkspaceHostError)) return;
  switch (cause.kind) {
    case 'file_settlement_unknown':
    case 'converter_settlement_unknown':
    case 'document_settlement_unknown': return cause.kind;
  }
}

export interface ProductHostWorkspaces {
  workbench?(scope: WorkspaceAuthorityScope, call: import('./workbench.ts').WorkbenchCall, signal?: AbortSignal): Promise<import('./workbench.ts').WorkbenchResult>;
  previewDocument?(scope: WorkspaceAuthorityScope, request: DocumentRequest, signal?: AbortSignal): Promise<DocumentResult>;
  desktopCatalog?(scope: WorkspaceAuthorityScope, refresh?: boolean): Promise<import('./desktop.ts').DesktopCatalog>;
  openWorkspace?(scope: WorkspaceAuthorityScope, target: import('./desktop.ts').DesktopTarget, application: import('./desktop.ts').DesktopAppId): Promise<import('./desktop.ts').DesktopLaunch>;
  readDelivery?(scope: WorkspaceAuthorityScope, read: DeliveryRead, signal?: AbortSignal): Promise<DeliveryBytes>;
  configureWorkspace?(id: string, endpoint: string, operation: WorkspaceConfigurationOperation): Promise<WorkspaceConfigurationResult>;
  listWorkspaces(signal?: AbortSignal): Promise<WorkspaceCatalog>;
  /** Each metadata write must validate this expected scope at execution, before changing registrations. */
  adoptWorkspace(scope: WorkspaceAuthorityScope, location: string): Promise<void>;
  renameWorkspace(scope: WorkspaceAuthorityScope, id: string, displayName: string): Promise<void>;
  reorderWorkspace(scope: WorkspaceAuthorityScope, id: string, before?: string): Promise<void>;
  removeWorkspace(scope: WorkspaceAuthorityScope, id: string): Promise<void>;
  resolveWorkspace(id: string, endpoint: string): Promise<{ cwd: string }>;
  /** Authorization is independent of registration. Exact Host-owned classification, bounded to a page. */
  classifyLocations(cwds: readonly string[], endpoint: string, authorityId?: string, signal?: AbortSignal): Promise<SessionLocation[]>;
}
