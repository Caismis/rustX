import type { AppServerClient, OperationAdmission } from '../client/app-server';
import { sameEndpoint } from './endpoint';
import { WorkspaceAuthority } from './authority';
/** Fresh operation admission. Display evidence is neither input nor prerequisite. */
export class WorkspaceSessionNavigation {
  constructor(private readonly authority: WorkspaceAuthority, private readonly client: AppServerClient, private readonly navigation: import('../client/navigation').NavigationEpoch) {}
  admit = async (id: string, current: () => boolean): Promise<false | OperationAdmission> => {
    const generation = this.client.getSnapshot().generation;
    const navigationCurrent = this.navigation.capture();
    const valid = () => current() && navigationCurrent() && generation === this.client.getSnapshot().generation;
    const observation = await this.authority.observe();
    if (!valid()) return false;
    const settings = await this.client.request({ method: 'session/settings', params: { session_id: id } }, 'settings');
    if (!valid() || !observation.current()) return false;
    const endpoint = this.client.getSnapshot().endpoint;
    if (!endpoint || !sameEndpoint(observation.catalog.endpoint, endpoint)) throw new Error('No matching rustX endpoint.');
    const [location] = await this.authority.classify([settings.settings.cwd], endpoint, observation);
    if (!valid() || !observation.current()) return false;
    const admitted = () => valid() && observation.current();
    if (!admitted()) return false;
    if (!location.authorized) throw new Error('Session cwd is not authorized by this Product Host. The durable Session is unchanged.');
    return { current: admitted, validate: async () => {
      // Called only after a native dispatch slot is reserved. A delayed success
      // from a retired Host cannot authorize even when display never runs.
      if (!admitted()) return false;
      await this.authority.observe();
      return admitted();
    } };
  };
}
