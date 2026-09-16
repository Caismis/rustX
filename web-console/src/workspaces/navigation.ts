import type { AppServerClient } from '../client/app-server';
import { createSession } from '../app/commands/native';
import type { ProductHostWorkspaces } from './host';
/** Resolve only a registered Host handle; never fall back to a browser path. */
export async function createWorkspaceSession(host: ProductHostWorkspaces, id: string, client: AppServerClient, current: () => boolean) {
  const generation = client.getSnapshot().generation;
  const valid = () => current() && generation === client.getSnapshot().generation && client.getSnapshot().connection === 'connected';
  const endpoint = client.getSnapshot().endpoint;
  if (!endpoint || !valid()) return;
  const { cwd } = await host.resolveWorkspace(id, endpoint);
  if (valid()) return createSession(client, cwd, valid);
}

/** The Web admission/navigation owner. No trust decisions or durable membership. */
export class WorkspaceSessionNavigation {
  constructor(private readonly host: ProductHostWorkspaces, private readonly client: AppServerClient, private readonly navigation: import('../app/commands/native').NavigationEpoch) {}
  private fence(current: () => boolean) {
    const generation = this.client.getSnapshot().generation;
    const navigationCurrent = this.navigation.capture();
    return () => current() && navigationCurrent() && generation === this.client.getSnapshot().generation;
  }
  async classifySession(id: string, current: () => boolean) {
    const valid = this.fence(current);
    const settings = await this.client.request({ method: 'settings/read', params: { session_id: id } }, 'settings');
    if (!valid()) return;
    const endpoint = this.client.getSnapshot().endpoint;
    if (!endpoint) throw new Error('No connected rustX endpoint.');
    const [location] = await this.host.classifyLocations([settings.settings.cwd], endpoint);
    if (valid()) return location;
  }
  admit = async (id: string, current: () => boolean) => {
    const location = await this.classifySession(id, current);
    if (!location) return false;
    if (!location.authorized) throw new Error('Session cwd is not authorized by this Product Host. The durable Session is unchanged.');
    return true;
  };
}
