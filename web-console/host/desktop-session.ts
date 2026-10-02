import { AppServerClient } from '../../tui/src/app-server/client.ts';
import { WebSocketTransport } from '../../tui/src/app-server/websocket-transport.ts';
import type { DesktopTarget } from '../src/workspaces/desktop.ts';
/** Exact native metadata read; never attaches a Session or composes an Agent. */
export async function readDesktopSession(endpoint: string, token: string, target: DesktopTarget) {
  const transport = await WebSocketTransport.connect({ endpoint, token });
  const deadline = setTimeout(() => { void transport.close(); }, 10_000);
  try {
    const client = await AppServerClient.initialize({ transport, identity: { name: 'rustx-product-host', version: '0.1.0' } });
    const { summary } = await client.call('session/summary', { session_id: target.session_id }, 'session_summary');
    if (summary.id !== target.session_id || summary.active_node !== target.active_node) throw new Error('Session target changed; select the current Session and try again');
    return summary.cwd;
  } finally { clearTimeout(deadline); void transport.close(); }
}
