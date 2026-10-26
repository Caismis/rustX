import type { AppServerClient } from '../../client/app-server';
import type { McpDraftTarget } from '../../../../protocol/app-server/v59';
import type { ProductHostWorkspaces } from '../../workspaces/host';
import type { WorkspaceAuthority } from '../../workspaces/authority';
import { sameEndpoint } from '../../workspaces/endpoint';
import { firstSubmissions } from './first-submit';

interface Selection {
  binding: string; workspace: string; target: McpDraftTarget;
  generation: number; endpoint: string; sealed: boolean;
}
/** Application-lifetime draft intent follows XState navigation, independently
 * of React mounts. Native owns resources, coalescing, capacity and settlement.
 * No outcome here is tool readiness or canonical input admission. */
export class DraftPreparations {
  private desired?: { binding: string; workspace?: string };
  private selected?: Selection;
  // A Send remains sealed across carrier replacement even when its create
  // outcome is unknown. Only a different logical draft may start new work.
  private submittedBinding?: string;
  private sequence = 0;
  private readonly identity = crypto.randomUUID();
  private disposed = false;
  private authoritySubscription?: () => void;
  private clientSubscription: () => void;
  private submissionSubscription: () => void;
  constructor(private readonly client: AppServerClient, private host: ProductHostWorkspaces, private authority: WorkspaceAuthority) {
    this.clientSubscription = client.subscribe(() => this.reconcile());
    const submissions = firstSubmissions(client);
    this.submissionSubscription = submissions.subscribe(() => {
      const binding = this.submittedBinding;
      const submission = binding === undefined ? undefined : submissions.draft(binding);
      const phase = submission?.phase;
      if (phase !== 'rejected' && phase !== 'discarded') return;
      // Only a draft without a known Session returns to preparation ownership.
      // Discarding retained input for a real Session cannot prewarm a new draft.
      this.submittedBinding = submission?.session || submission?.existingSessionId ? binding : undefined;
      if (this.selected?.binding === binding) this.retire();
      this.reconcile();
    });
    client.onDispose(() => this.dispose());
    this.authoritySubscription = authority.subscribe(() => this.authorityChanged());
  }
  configure(host: ProductHostWorkspaces, authority: WorkspaceAuthority) {
    if (host === this.host && authority === this.authority) return;
    if (host !== this.host) this.retire();
    this.authoritySubscription?.();
    this.host = host; this.authority = authority;
    this.authoritySubscription = authority.subscribe(() => this.authorityChanged());
    this.reconcile();
  }
  private authorityChanged() {
    const catalog = this.authority.getCatalog(), selected = this.selected;
    if (selected?.target.host_authority && (!catalog || catalog.authorityId !== selected.target.host_authority || !sameEndpoint(catalog.endpoint, selected.endpoint))) this.retire();
    this.reconcile();
  }
  follow(binding?: string, workspace?: string) {
    this.desired = binding === undefined ? undefined : { binding, workspace };
    this.reconcile();
  }
  private reconcile() {
    if (this.disposed) return;
    const state = this.client.getSnapshot(), desired = this.desired;
    const held = this.selected;
    if (held && state.connection === 'connected' && state.generation === held.generation && state.endpoint === held.endpoint
      && (held.sealed && desired?.binding === held.binding || desired?.binding === held.binding && desired.workspace === held.workspace)) return;
    this.retire();
    if (desired?.binding === this.submittedBinding) return;
    if (!desired?.workspace || state.connection !== 'connected' || !state.endpoint) return;
    const selection: Selection = { binding: desired.binding, workspace: desired.workspace,
      target: { draft_id: this.identity, generation: ++this.sequence, host_authority: '' },
      generation: state.generation, endpoint: state.endpoint, sealed: false };
    this.selected = selection;
    void this.prepare(selection);
  }
  private current(selection: Selection) {
    const state = this.client.getSnapshot();
    return !this.disposed && this.selected === selection && state.connection === 'connected'
      && state.generation === selection.generation && state.endpoint === selection.endpoint;
  }
  private async prepare(selection: Selection) {
    try {
      const observation = await this.authority.observe();
      if (!this.current(selection) || selection.sealed || !observation.current()
        || !sameEndpoint(observation.catalog.endpoint, selection.endpoint)
        || !observation.catalog.workspaces.some(workspace => workspace.id === selection.workspace)) return;
      selection.target.host_authority = observation.catalog.authorityId;
      const { cwd } = await this.host.resolveWorkspace(selection.workspace, selection.endpoint);
      const current = () => this.current(selection) && !selection.sealed && observation.current();
      if (!current()) return;
      await this.client.request({ method: 'mcp/draftPrepare', params: { target: selection.target, cwd } }, 'mcp_draft_accepted', undefined, {
        current,
        validate: async signal => {
          const fresh = await this.authority.observe(signal);
          if (!current() || !fresh.current() || fresh.catalog.authorityId !== selection.target.host_authority) return false;
          const resolved = await this.host.resolveWorkspace(selection.workspace, selection.endpoint);
          const [location] = await this.authority.classify([cwd], selection.endpoint, fresh, signal);
          return current() && resolved.cwd === cwd && location.authorized;
        },
      });
    } catch { /* Silent preparation never creates a notice or retries a failure. Send revalidates natively. */ }
  }
  /** The actual Send captures one exact target. Even absent/lost prewarming
   * uses the same native submission path and original receipt deadline. */
  seal(binding: string, workspace: string, hostAuthority: string): McpDraftTarget {
    this.submittedBinding = binding;
    const selected = this.selected;
    if (selected && this.current(selected) && selected.binding === binding && selected.workspace === workspace
      && (!selected.target.host_authority || selected.target.host_authority === hostAuthority)) {
      selected.sealed = true;
      selected.target.host_authority = hostAuthority;
      return { ...selected.target };
    }
    this.retire();
    return { draft_id: this.identity, generation: ++this.sequence, host_authority: hostAuthority };
  }
  private retire() {
    const selected = this.selected;
    this.selected = undefined;
    const state = this.client.getSnapshot();
    if (!selected?.target.host_authority || state.connection !== 'connected'
      || state.generation !== selected.generation || state.endpoint !== selected.endpoint) return;
    void this.client.request({ method: 'mcp/draftCancel', params: { target: selected.target } }, 'mcp_draft_accepted', undefined,
      () => { const state = this.client.getSnapshot(); return state.connection === 'connected' && state.generation === selected.generation && state.endpoint === selected.endpoint; }).catch(() => {});
  }
  dispose() { this.retire(); this.disposed = true; this.desired = undefined; this.clientSubscription(); this.submissionSubscription(); this.authoritySubscription?.(); }
}
const owners = new WeakMap<AppServerClient, DraftPreparations>();
export function draftPreparations(client: AppServerClient, host?: ProductHostWorkspaces, authority?: WorkspaceAuthority) {
  let owner = owners.get(client);
  if (host && authority) {
    if (owner) owner.configure(host, authority);
    else { owner = new DraftPreparations(client, host, authority); owners.set(client, owner); }
  }
  return owner;
}
