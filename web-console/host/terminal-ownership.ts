/** Native ownership notifications are independent of runtime attachments. */
import { AppServerClient } from '../../tui/src/app-server/client.ts';
import { WebSocketTransport } from '../../tui/src/app-server/websocket-transport.ts';
export async function observeTerminalOwnership(endpoint: string, token: string, retired: (session: string) => void, lost: () => void) {
  const transport = await WebSocketTransport.connect({ endpoint, token });
  const client = await AppServerClient.initialize({ transport, identity: { name: 'rustx-terminal-owner', version: '0.1.0' } }).catch(error => { void transport.close(); throw error; });
  client.onNotification(notification => { if (notification.method === 'session/ownershipRetired') retired(notification.params.session_id); });
  const unlisten = client.onClose(lost);
  return async () => { unlisten(); await client.close(); };
}
