import type { AppServerClient, OperationAdmission } from '../client/app-server';
import { sameEndpoint } from './endpoint';
import { WorkspaceAuthority } from './authority';
/** Fresh operation admission. Display evidence is neither input nor prerequisite.
 * Native Session cwd is fixed at creation on every App Server path, so the exact
 * cwd read here is the one reauthorized at the dispatch boundary. */
export class WorkspaceSessionNavigation {
  constructor(private readonly authority: WorkspaceAuthority, private readonly client: AppServerClient, private readonly navigation: import('../client/navigation').NavigationEpoch) {}
  admit = async (id: string, current: () => boolean): Promise<false | OperationAdmission> => {
    const { generation, endpoint } = this.client.getSnapshot();
    const navigationCurrent = this.navigation.capture();
    const valid = () => current() && navigationCurrent() && generation === this.client.getSnapshot().generation && endpoint === this.client.getSnapshot().endpoint;
    const observation = await this.authority.observe();
    if (!valid()) return false;
    const settings = await this.client.request({ method: 'session/settings', params: { session_id: id } }, 'settings');
    if (!valid() || !observation.current()) return false;
    if (!endpoint || !sameEndpoint(observation.catalog.endpoint, endpoint)) throw new Error('No matching rustX endpoint.');
    const cwd = settings.settings.cwd;
    const [location] = await this.authority.classify([cwd], endpoint, observation);
    const admitted = () => valid() && observation.current();
    if (!admitted()) return false;
    if (!location.authorized) throw new Error('Session cwd is not authorized by this Product Host. The durable Session is unchanged.');
    return { current: admitted, validate: async signal => {
      // Called only after a native dispatch slot is reserved. This fresh
      // classification of the exact cwd, not the one above, is the admission
      // linearization point: Host process identity alone does not prove that a
      // configured root still exists or still resolves to the same directory.
      // Host replacement retires this proof at once, so its reads are aborted too.
      if (!admitted()) return false;
      const retired = new AbortController();
      const release = this.authority.subscribe(() => retired.abort());
      const scoped = AbortSignal.any([signal, retired.signal]);
      try {
        const fresh = await this.authority.observe(scoped);
        if (!admitted()) return false;
        const [final] = await this.authority.classify([cwd], endpoint, fresh, scoped);
        if (!admitted()) return false;
        if (!final.authorized) throw new Error(`Session cwd is no longer authorized by this Product Host (${final.reason}). No operation was sent.`);
        return true;
      } catch (cause) { if (!admitted()) return false; throw cause; }
      finally { release(); }
    } };
  };
}
