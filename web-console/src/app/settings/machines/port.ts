import type { SourceMutation, SourceSettings } from '../../../../../protocol/app-server/v18';
import type { AppServerClient } from '../../../client/app-server';
import type { ProductHostWorkspaces, WorkspaceConfigurationReread } from '../../../workspaces/host';

/** One confirmed native mutation and, where the authority owns one, the
 * authoritative reread it performed after the commit. The two stay separate
 * facts: a failed reread never turns a committed write into a failed one. */
export interface WriteOutcome {
  acknowledgement: SourceSettings;
  reread?: WorkspaceConfigurationReread;
}

/** The native configuration authority of exactly one Settings target.
 *
 * This is the whole boundary between the orchestration machines and the App
 * Server / Product Host. Machines never touch a client, a socket or a Host:
 * they invoke this port, which is what lets a machine test drive a real
 * transition graph with deferred promises instead of timers.
 *
 * `ownsReread` is a native fact about the authority, not a preference: the
 * Product Host performs its own authoritative reread inside a Workspace write,
 * so that write reserves the read order at its initiation. A User write owns no
 * reread and never reserves one. */
export interface ConfigurationPort {
  readonly ownsReread: boolean;
  /** One authoritative read of this exact target. */
  read(): Promise<SourceSettings>;
  /** One exact native semantic-unit mutation against one exact CAS revision. */
  write(expectedRevision: string, mutation: SourceMutation): Promise<WriteOutcome>;
  /** Re-derive native state. Not a projection: the caller still reads. */
  reconcile(): Promise<void>;
}

/** Build the port of one Settings target over the live client and Product Host.
 *
 * `host` is read through a function because the Product Host object identity is
 * a presentation detail that may change on any render, while the port — and the
 * actor that invokes it — is bound to the authority, endpoint and target. */
export function createConfigurationPort({ client, endpoint, workspaceId, host }: {
  client: AppServerClient;
  endpoint: string;
  workspaceId?: string;
  host: () => ProductHostWorkspaces | undefined;
}): ConfigurationPort {
  const workspace = () => {
    const configure = host()?.configureWorkspace;
    if (!configure) throw new Error('Workspace Settings requires an authorized Product Host connection.');
    return configure;
  };
  if (workspaceId === undefined) {
    const target = { kind: 'user' as const };
    return {
      ownsReread: false,
      read: async () => (await client.request({ method: 'configuration/sourcesRead', params: { target } }, 'source_settings')).projection,
      write: async (expected_revision, mutation) => ({
        acknowledgement: (await client.request({ method: 'configuration/sourceWrite', params: { target, expected_revision, mutation } }, 'source_settings')).projection,
      }),
      reconcile: async () => { await client.request({ method: 'configuration/reconcile', params: { target } }, 'configuration_application'); },
    };
  }
  return {
    ownsReread: true,
    read: async () => {
      const result = await workspace()(workspaceId, endpoint, { kind: 'read' });
      if (result.kind !== 'read') throw new Error('Workspace Host returned a non-read result for a read');
      return result.projection;
    },
    write: async (expected_revision, mutation) => {
      const result = await workspace()(workspaceId, endpoint, { kind: 'write', expected_revision, mutation });
      if (result.kind !== 'write') throw new Error('Workspace Host returned a non-write result for a write');
      return { acknowledgement: result.commit.acknowledgement, reread: result.commit.reread };
    },
    reconcile: async () => {
      const result = await workspace()(workspaceId, endpoint, { kind: 'reconcile' });
      if (result.kind !== 'reconcile') throw new Error('Workspace Host returned a non-reconcile result for a reconcile');
    },
  };
}
