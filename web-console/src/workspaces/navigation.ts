import type { AppServerClient } from '../client/app-server';
import { createSession } from '../app/commands/native';
import type { ProductHostWorkspaces } from './host';
/** Resolve only a registered Host handle; never fall back to a browser path. */
export async function createWorkspaceSession(host: ProductHostWorkspaces, id: string, endpoint: string, client: AppServerClient, current: () => boolean) {
  const generation = client.getSnapshot().generation;
  const valid = () => current() && generation === client.getSnapshot().generation && client.getSnapshot().connection === 'connected';
  const { cwd } = await host.resolveWorkspace(id, endpoint);
  if (valid()) return createSession(client, cwd, valid);
}
