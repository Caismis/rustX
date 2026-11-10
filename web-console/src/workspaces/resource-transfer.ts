import type { AppServerClient } from '../client/app-server';
import type { WorkspaceAuthorityObservation } from './authority';
import { sameEndpoint } from './endpoint';
import { WorkspaceHostError, type ProductHostWorkspaces, type ResourceTransferMutation } from './host';

export type TransferResource = (directory: string, revision: string, mutation: ResourceTransferMutation) => Promise<void>;

export function bindResourceTransfer(client: AppServerClient, host: () => ProductHostWorkspaces | undefined,
  authority: WorkspaceAuthorityObservation | undefined, workspaceId: string): TransferResource {
  const { endpoint, authorityRevision, authorityId } = client.getSnapshot();
  return async (directory, revision, mutation) => {
    const current = client.getSnapshot();
    const owner = host();
    if (!authority?.current() || !endpoint || !authorityId || !sameEndpoint(authority.scope.endpoint, endpoint)
      || current.endpoint !== endpoint || current.authorityRevision !== authorityRevision || current.connection !== 'connected'
      || !owner?.configureWorkspace) throw new WorkspaceHostError('Workspace or App Server authority changed', 'workspace_unavailable');
    const result = await owner.configureWorkspace(workspaceId, endpoint, {
      kind: 'transfer', scope: authority.scope, native: authorityId,
      expected_directory: directory, expected_revision: revision, mutation,
    });
    if (result.kind !== 'transfer') throw new WorkspaceHostError('Unexpected resource transfer outcome', undefined, true);
  };
}
