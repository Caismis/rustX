/** Startup routing: catalog browsing never acquires an attachment. */
import type { TuiArguments } from "./cli.ts";
import type { AppServerHost } from "./app-server/host.ts";
import type { AppServerSession } from "./app-server/session.ts";

export type SessionCatalogPage = Awaited<ReturnType<AppServerHost["listSessions"]>>;
export interface ConnectingSession { sessionId: string; nodeId?: string }
export type StartupFocus =
  | { session: AppServerSession; resumePage?: never }
  | { session?: undefined; resumePage: SessionCatalogPage; connecting?: never }
  | { session?: undefined; resumePage?: never; connecting: ConnectingSession };

export async function prepareStartup(
  host: AppServerHost,
  parsed: TuiArguments,
): Promise<StartupFocus> {
  if (parsed.routing.session !== undefined) {
    if (parsed.sessionName !== undefined) {
      await host.renameSession(parsed.routing.session, parsed.sessionName);
    }
    return { connecting: { sessionId: parsed.routing.session, nodeId: parsed.routing.node } };
  }
  if (parsed.routing.openSessionSelector) {
    const resumePage = await host.listSessions();
    if (resumePage.sessions.length > 0) return { resumePage };
    // An empty durable catalog is the one explicit resume creation path.
  }
  const created = await host.createSession(parsed.sessionSettings);
  if (parsed.sessionName !== undefined) {
    await host.renameSession(created.session.id, parsed.sessionName);
  }
  return { connecting: { sessionId: created.session.id, nodeId: created.session.active_node } };
}
