import type {
  ErrorData, WorkflowCheckReport, WorkflowDefinition, WorkflowDiagnostic, WorkflowEdit, WorkflowLibrary,
  WorkflowSource, WorkflowModelChoices, WorkflowModelChoicesQuery,
} from '../../../../protocol/app-server/v57';
import type { AppServerClient } from '../../client/app-server';
import { isOutcomeUncertain, RequestNotDispatched, RpcFailure } from '../../client/errors';
import type { WorkspaceAuthorityObservation } from '../../workspaces/authority';
import { sameEndpoint } from '../../workspaces/endpoint';
import { WorkspaceHostError, type ProductHostWorkspaces, type WorkflowCommit, type WorkflowHostOperation, type WorkflowHostResult } from '../../workspaces/host';

/** The source scope a Workflow file is physically owned by. The User scope
 * is presented as Global. A Workspace is addressed only by its Product Host
 * registration; the browser never names a directory. */
export type WorkflowScope = { readonly kind: 'user' } | { readonly kind: 'workspace'; readonly workspaceId: string };

export const USER_SCOPE: WorkflowScope = Object.freeze({ kind: 'user' });

export function scopeKey(scope: WorkflowScope): string {
  return scope.kind === 'user' ? 'user' : `workspace:${scope.workspaceId}`;
}

export function sameScope(left: WorkflowScope, right: WorkflowScope): boolean {
  return scopeKey(left) === scopeKey(right);
}

/** The exact file an authoring transaction is bound to: one server, one
 * scope, one native identity. It never follows the focused conversation. */
export interface WorkflowTarget {
  readonly endpoint: string;
  readonly scope: WorkflowScope;
  readonly id: string;
}

export function targetKey(target: WorkflowTarget): string {
  return JSON.stringify([target.endpoint, scopeKey(target.scope), target.id]);
}

/** A typed authoring failure. Each kind is a distinct product fact. */
export type WorkflowFailure =
  /** Native compared the expected revision and found another. */
  | { kind: 'conflict'; expected: string; actual: string }
  /** The native parser or compiler rejected the candidate; nothing changed. */
  | { kind: 'invalid'; diagnostic: WorkflowDiagnostic }
  /** The outcome of a write is unknown; it is reconciled, never replayed. */
  | { kind: 'uncertain'; message: string }
  /** The Workspace registration, Product Host or server no longer authorizes this target. */
  | { kind: 'unavailable'; message: string }
  /** The request was rejected or never sent. */
  | { kind: 'failed'; message: string };

export class WorkflowOperationError extends Error {
  constructor(readonly failure: WorkflowFailure) { super('message' in failure ? failure.message : failure.kind); this.name = 'WorkflowOperationError'; }
}

function native(data: ErrorData | null | undefined): WorkflowFailure | undefined {
  switch (data?.kind) {
    case 'source_conflict': return { kind: 'conflict', expected: data.expected, actual: data.actual };
    case 'workflow_invalid': return { kind: 'invalid', diagnostic: data.diagnostic };
    case 'committed_durability_uncertain':
    case 'outcome_unknown': return { kind: 'uncertain', message: data.kind };
    default: return undefined;
  }
}

/** Classify any failure of one operation. A write whose acknowledgement may
 * have been lost is `uncertain`, never `failed`. */
export function classifyFailure(cause: unknown, write: boolean): WorkflowFailure {
  if (cause instanceof WorkflowOperationError) return cause.failure;
  const message = cause instanceof Error ? cause.message : String(cause);
  if (cause instanceof RpcFailure) return native(cause.error.data) ?? { kind: 'failed', message };
  if (cause instanceof WorkspaceHostError) {
    const relayed = native(cause.native);
    if (relayed) return relayed;
    if (cause.uncertain) return { kind: 'uncertain', message };
    if (cause.kind === 'workspace_unavailable' || cause.kind === 'authority_replaced' || cause.kind === 'native_replaced') return { kind: 'unavailable', message };
    return { kind: 'failed', message };
  }
  if (cause instanceof RequestNotDispatched) return { kind: 'failed', message };
  if (isOutcomeUncertain(cause)) return { kind: write ? 'uncertain' : 'failed', message };
  return write ? { kind: 'uncertain', message } : { kind: 'failed', message };
}

/** The native authority of exactly one Workflow scope on one server. State
 * machines invoke this port and nothing else, so tests drive real
 * transition graphs with controlled promises. */
export interface WorkflowScopePort {
  readonly endpoint: string;
  readonly scope: WorkflowScope;
  list(): Promise<WorkflowLibrary>;
  read(id: string): Promise<WorkflowSource>;
  /** Bounded registered Model/Profile choices from this exact source scope.
   * A continuation is bound to the first page's native source revision. */
  modelChoices(query: WorkflowModelChoicesQuery, expectedRevision?: string): Promise<WorkflowModelChoices>;
  /** Check the exact candidate. A Global file's dependencies are checked
   * only in an explicitly chosen registered Workspace. */
  check(id: string, document: WorkflowDefinition, explain: boolean, context?: string): Promise<WorkflowCheckReport>;
  /** One whole-file write. Its acknowledgement and its reread are separate. */
  write(id: string, expectedRevision: string, edit: WorkflowEdit): Promise<WorkflowCommit>;
}

const replaced = (write: boolean) => new WorkflowOperationError(write ? { kind: 'uncertain', message: 'server_replaced' } : { kind: 'unavailable', message: 'server_replaced' });

/** Build the port of one scope over the live client and Product Host.
 *
 * A port is bound to two authorities, both observed when it is created:
 * the App Server (endpoint and authority revision) and the Product Host
 * that maps Workspace registrations (`hostAuthority`). A Workspace
 * operation, and a Global file's dependency check routed through a
 * Workspace, carries that Product Host scope, which the Host validates in
 * the registration lane immediately before native dispatch. A replaced
 * authority therefore refuses the transaction even when it reuses the
 * registration ID; it is never retargeted.
 *
 * A write acknowledgement is a fact about the original target once it is
 * received: a later authority change marks its reread unobserved but never
 * erases the commit. Results that would come from a replacement authority
 * are refused. */
export function createWorkflowPort({ client, host, hostAuthority, scope }: {
  client: AppServerClient;
  host: () => ProductHostWorkspaces | undefined;
  /** The Product Host authority current at creation, if one is observed. */
  hostAuthority: WorkspaceAuthorityObservation | undefined;
  scope: WorkflowScope;
}): WorkflowScopePort {
  // The native authority is the `initialize` identity already observed;
  // it is never filled in later from another connection.
  const { endpoint = '', authorityRevision, authorityId: native } = client.getSnapshot();
  const current = () => client.getSnapshot().authorityRevision === authorityRevision && client.getSnapshot().endpoint === endpoint;
  const attempt = async <T>(write: boolean, operation: () => Promise<T>): Promise<T> => {
    if (!current()) throw new WorkflowOperationError({ kind: 'unavailable', message: 'server_replaced' });
    try { return await operation(); } catch (cause) { throw new WorkflowOperationError(classifyFailure(cause, write)); }
  };
  /** A read or check: its result is refused once its server is replaced. */
  const read = async <T>(operation: () => Promise<T>): Promise<T> => {
    const result = await attempt(false, operation);
    if (!current()) throw replaced(false);
    return result;
  };
  /** A write: its acknowledgement stands; only the reread depends on the
   * server still being the one that acknowledged it. */
  const write = async (operation: () => Promise<WorkflowCommit>): Promise<WorkflowCommit> => {
    const commit = await attempt(true, operation);
    return current() || commit.reread.status === 'failed' ? commit : { ...commit, reread: { status: 'failed', error: 'server_replaced' } };
  };
  const viaHost = async <K extends WorkflowHostResult['kind']>(workspaceId: string, operation: WorkflowHostOperation & { kind: K }) => {
    const owner = host();
    if (!owner?.workflowWorkspace) throw new WorkflowOperationError({ kind: 'unavailable', message: 'product_host_unavailable' });
    if (!hostAuthority?.current() || !sameEndpoint(hostAuthority.scope.endpoint, endpoint)) {
      throw new WorkflowOperationError({ kind: 'unavailable', message: 'product_host_replaced' });
    }
    if (!native) throw new WorkflowOperationError({ kind: 'unavailable', message: 'server_unavailable' });
    const result = await owner.workflowWorkspace(hostAuthority.scope, native, workspaceId, operation);
    if (result.kind !== operation.kind) throw new Error('Unexpected Product Host result');
    return result as Extract<WorkflowHostResult, { kind: K }>;
  };
  if (scope.kind === 'workspace') {
    const { workspaceId } = scope;
    return {
      endpoint, scope,
      list: () => read(async () => (await viaHost(workspaceId, { kind: 'list' })).library),
      read: id => read(async () => (await viaHost(workspaceId, { kind: 'read', id })).source),
      modelChoices: (query, expected_revision) => read(async () => (await viaHost(workspaceId, { kind: 'model_choices', query, expected_revision })).choices),
      check: (id, document, explain) => read(async () => (await viaHost(workspaceId, { kind: 'check', scope: 'workspace', id, document, explain })).report),
      write: (id, expected_revision, edit) => write(async () => (await viaHost(workspaceId, { kind: 'write', id, expected_revision, edit })).commit),
    };
  }
  const target = { kind: 'user' as const };
  return {
    endpoint, scope,
    list: () => read(async () => (await client.request({ method: 'workflow/list', params: { target } }, 'workflow_library')).library),
    read: id => read(async () => (await client.request({ method: 'workflow/read', params: { target, id } }, 'workflow_source')).source),
    modelChoices: (query, expected_revision) => read(async () => (await client.request({ method: 'workflow/modelChoices', params: { target, query, expected_revision } }, 'workflow_model_choices')).choices),
    check: (id, document, explain, context) => read(async () => context === undefined
      ? (await client.request({ method: 'workflow/check', params: { target, id, document, explain } }, 'workflow_check')).report
      : (await viaHost(context, { kind: 'check', scope: 'user', id, document, explain })).report),
    write: (id, expected_revision, edit) => write(async () => {
      const acknowledgement = await client.request({ method: 'configuration/sourceWrite', params: { target, expected_revision, mutation: { kind: 'workflow', id, edit } } }, 'sources_written');
      // The commit is a fact from here on; its reread may fail independently.
      let reread: WorkflowCommit['reread'];
      try { reread = { status: 'observed', source: (await client.request({ method: 'workflow/read', params: { target, id } }, 'workflow_source')).source }; }
      catch (error) { reread = { status: 'failed', error: error instanceof Error ? error.message : String(error) }; }
      return { acknowledgement: { revision: acknowledgement.revision, configuration_revision: acknowledgement.configuration_revision ?? null }, reread };
    }),
  };
}
