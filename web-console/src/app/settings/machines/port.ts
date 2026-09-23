import type { ConfigurationApplication, SourceMutation, SourceSettings } from '../../../../../protocol/app-server/v18';
import type { AppServerClient, ClientView } from '../../../client/app-server';
import type { ProductHostWorkspaces, WorkspaceConfigurationReread } from '../../../workspaces/host';
import { applicationScope } from '../projection';

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
 * reread and never reserves one.
 *
 * `publication` is the target-local publication boundary. Native publishes
 * application versions per application scope — each Session, each source — and
 * the client mirrors all of them in one map. The port alone knows which exact
 * native source scope this target owns, so it alone projects that map to the
 * one publication that concerns this target. Nothing else of the map reaches
 * the target's actor. */
export interface ConfigurationPort {
  readonly ownsReread: boolean;
  /** The native publication of exactly this target's source application
   * scope, or `undefined` while native published none or while this port
   * cannot yet name its source scope. */
  publication(configuration: ClientView['configuration']): ConfigurationApplication | undefined;
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
 * actor that invokes it — is bound to the authority, endpoint and target.
 *
 * The User source scope is the native constant `source:user`. A Workspace
 * source scope names the exact canonical configuration directory, which only
 * the Product Host knows: its registration resolution is what names the
 * `SourceTarget` of every Workspace configuration operation. The port therefore
 * asks exactly that resolution — once, alongside its first read, and again
 * alongside later reads until it answers — and never derives the directory from
 * a display name, a Session or a browser path. `identified` is called once when
 * the scope becomes known, so the owner of the transport can deliver this
 * target's publication level at that moment. A resolution that fails leaves the
 * scope unnamed; it is not a read failure, and no publication is attributed to
 * this target until it is named. */
export function createConfigurationPort({ client, endpoint, workspaceId, host, identified }: {
  client: AppServerClient;
  endpoint: string;
  workspaceId?: string;
  host: () => ProductHostWorkspaces | undefined;
  identified: () => void;
}): ConfigurationPort {
  const workspace = () => {
    const configure = host()?.configureWorkspace;
    if (!configure) throw new Error('Workspace Settings requires an authorized Product Host connection.');
    return configure;
  };
  if (workspaceId === undefined) {
    const target = { kind: 'user' as const };
    const scope = applicationScope(target);
    return {
      ownsReread: false,
      publication: configuration => configuration?.[scope],
      read: async () => (await client.request({ method: 'configuration/sourcesRead', params: { target } }, 'source_settings')).projection,
      write: async (expected_revision, mutation) => ({
        acknowledgement: (await client.request({ method: 'configuration/sourceWrite', params: { target, expected_revision, mutation } }, 'source_settings')).projection,
      }),
      reconcile: async () => { await client.request({ method: 'configuration/reconcile', params: { target } }, 'configuration_application'); },
    };
  }
  let scope: string | undefined;
  const identify = async () => {
    const current = host();
    if (scope !== undefined || !current) return;
    try {
      const { cwd } = await current.resolveWorkspace(workspaceId, endpoint);
      if (scope !== undefined) return;
      scope = applicationScope({ kind: 'workspace', directory: cwd });
      identified();
    } catch {
      // Unnamed until a later read resolves it: see above.
    }
  };
  return {
    ownsReread: true,
    publication: configuration => scope === undefined ? undefined : configuration?.[scope],
    read: async () => {
      // The scope is named before the read settles, so a failed read already
      // knows which publication may retry it.
      const naming = identify();
      try {
        const result = await workspace()(workspaceId, endpoint, { kind: 'read' });
        if (result.kind !== 'read') throw new Error('Workspace Host returned a non-read result for a read');
        return result.projection;
      } finally { await naming; }
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
