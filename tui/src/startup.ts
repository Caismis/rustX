/** Startup routing: catalog browsing never acquires an attachment. */
import type { TuiArguments } from "./cli.ts";
import type { AppServerHost } from "./app-server/host.ts";
import type { AppServerSession } from "./app-server/session.ts";

export type SessionCatalogPage = Awaited<ReturnType<AppServerHost["listSessions"]>>;
export type StartupFocus =
  | { session: AppServerSession; resumePage?: never }
  | { session?: undefined; resumePage: SessionCatalogPage };

export async function prepareStartup(
  host: AppServerHost,
  parsed: TuiArguments,
): Promise<StartupFocus> {
  if (parsed.routing.session !== undefined) {
    const session = await host.attach(parsed.routing.session, parsed.routing.node);
    if (parsed.sessionName !== undefined) {
      await host.renameSession(parsed.routing.session, parsed.sessionName);
    }
    return { session };
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
  return { session: await host.attach(created.session.id, created.session.active_node) };
}
