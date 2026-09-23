import type { ConfigurationApplication, SourceMutation, SourceSettings } from '../../../../../protocol/app-server/v19';
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
   * cannot yet name its source scope. A port that has returned a successful
   * read can always name it. */
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
 * `SourceTarget` of every Workspace configuration operation. The port never
 * derives the directory from a display name, a Session or a browser path.
 *
 * A successful Workspace configuration operation is self-identifying. Native
 * answers every source read and write with a `SourceSettings` whose `target` is
 * the exact `SourceTarget` it performed the I/O on — a directory native itself
 * rejects unless it is canonical — and native publishes that source under
 * exactly that target's application scope. So a read or write that succeeds
 * names this target's scope from its own result, before the result reaches the
 * actor, and a projection adopted as authoritative is never left without the
 * publication scope that owns it.
 *
 * The Host's registration resolution is asked as well, alongside each read
 * while the scope is still unnamed, for one case only: a read that fails still
 * knows which publication may retry it once the Host names the scope. A failed
 * resolution is neither a read failure nor a reason to hold back a successful
 * read. `identified` is called whenever the named scope changes, so the owner of
 * the transport can deliver this target's publication level at that moment. */
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
  const name = (next: string) => {
    if (scope === next) return;
    scope = next;
    identified();
  };
  /** Name the scope native performed this exact Workspace configuration I/O
   * on. */
  const performedOn = ({ target }: SourceSettings) => {
    if (target.kind === 'workspace') name(applicationScope(target));
  };
  const resolve = async () => {
    const current = host();
    if (scope !== undefined || !current) return;
    try {
      const { cwd } = await current.resolveWorkspace(workspaceId, endpoint);
      // A successful operation that answered meanwhile already named it.
      if (scope === undefined) name(applicationScope({ kind: 'workspace', directory: cwd }));
    } catch {
      // Unnamed until a later read or resolution names it: see above.
    }
  };
  return {
    ownsReread: true,
    publication: configuration => scope === undefined ? undefined : configuration?.[scope],
    read: async () => {
      const naming = resolve();
      let result;
      try {
        result = await workspace()(workspaceId, endpoint, { kind: 'read' });
      } catch (error) {
        // A failed read settles only once the Host has answered whether it can
        // name the scope, so it already knows which publication may retry it.
        await naming;
        throw error;
      }
      if (result.kind !== 'read') throw new Error('Workspace Host returned a non-read result for a read');
      if (result.projection.target.kind !== 'workspace') throw new Error('Workspace Host returned a non-Workspace source for a read');
      performedOn(result.projection);
      return result.projection;
    },
    write: async (expected_revision, mutation) => {
      const result = await workspace()(workspaceId, endpoint, { kind: 'write', expected_revision, mutation });
      if (result.kind !== 'write') throw new Error('Workspace Host returned a non-write result for a write');
      // The commit is definitive whatever it names: naming never fails it.
      performedOn(result.commit.acknowledgement);
      return { acknowledgement: result.commit.acknowledgement, reread: result.commit.reread };
    },
    reconcile: async () => {
      const result = await workspace()(workspaceId, endpoint, { kind: 'reconcile' });
      if (result.kind !== 'reconcile') throw new Error('Workspace Host returned a non-reconcile result for a reconcile');
    },
  };
}
