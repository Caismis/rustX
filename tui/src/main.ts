#!/usr/bin/env node
/**
 * `rustx-tui` — the rustX reference terminal client.
 *
 * Startup is the composition root and nothing more:
 *
 * ```text
 * parse arguments
 *   -> bind a transport        (own a stdio child, or connect to a WebSocket)
 *   -> initialize              (AppServerClient: one protocol, both modes)
 *   -> open the first Session  (attach an existing one, or create one)
 *   -> run the terminal        (RustxTuiApp)
 * ```
 *
 * A startup failure is reported on stderr with a bounded diagnostic and a
 * non-zero exit. The client resolves no credential and reads no runtime
 * configuration file of its own; the one file it does read is the dedicated
 * WebSocket transport token, which is a transport secret and nothing else.
 *
 * Opening the first Session is an ordinary client operation. Nothing here
 * publishes a global active Session, because the App Server has none.
 */

import { readFile } from "node:fs/promises";

import { ArgumentError, USAGE, parseArguments, type TuiArguments } from "./cli.ts";
import { AppServerHost } from "./app-server/host.ts";
import type { AppServerSession } from "./app-server/session.ts";
import { RustxTuiApp } from "./ui/app.ts";
import {
  configurationCommand,
  forwardConfigurationCommand,
} from "./configuration-command.ts";

async function connect(parsed: TuiArguments): Promise<AppServerHost> {
  if (parsed.mode.kind === "local") {
    return AppServerHost.spawnLocal({
      binary: parsed.mode.binary,
      launch: parsed.mode.launch,
    });
  }
  // At most one trailing newline, exactly as the server's own token file
  // contract allows. Nothing else about the value is interpreted here.
  const token = (await readFile(parsed.mode.tokenFile, "utf8")).replace(/\n$/, "");
  return AppServerHost.connectRemote({
    endpoint: parsed.mode.endpoint,
    token,
  });
}

/**
 * Opens the Session the terminal starts on.
 *
 * `--session` attaches to that exact durable Session. `--resume` reuses the
 * most recently updated one so the picker opens over real content instead of
 * stranding a fresh empty Session beside it. Otherwise this launch creates one.
 */
async function openInitialSession(
  host: AppServerHost,
  parsed: TuiArguments,
): Promise<AppServerSession> {
  const routing = parsed.routing;
  if (routing.session !== undefined) {
    return host.attach(routing.session, routing.node);
  }
  if (routing.openSessionSelector) {
    const page = await host.listSessions(undefined, 0, 1);
    const existing = page.sessions[0];
    if (existing !== undefined) {
      return host.attach(existing.id, existing.active_node);
    }
  }
  const created = await host.createSession(parsed.sessionSettings);
  if (parsed.sessionName !== undefined) {
    await host.renameSession(created.session.id, parsed.sessionName);
  }
  return host.attach(created.session.id, created.session.active_node);
}

async function main(argv: readonly string[]): Promise<number> {
  const configuration = configurationCommand(argv);
  if (configuration !== undefined) {
    return forwardConfigurationCommand(configuration);
  }

  let parsed: TuiArguments;
  try {
    parsed = parseArguments(argv);
  } catch (error) {
    if (error instanceof ArgumentError) {
      process.stderr.write(`rustx-tui: ${error.message}\n${USAGE}\n`);
      return 2;
    }
    throw error;
  }

  let host: AppServerHost;
  try {
    host = await connect(parsed);
  } catch (error) {
    process.stderr.write(`rustx-tui: ${(error as Error).message}\n`);
    return 1;
  }

  let session: AppServerSession;
  try {
    session = await openInitialSession(host, parsed);
    if (parsed.routing.session !== undefined && parsed.sessionName !== undefined) {
      await host.renameSession(parsed.routing.session, parsed.sessionName);
    }
  } catch (error) {
    const stderr = host.stderrTail().text.trim();
    // A failed start still releases whatever this launch owns: an owned child
    // is shut down, and an external server is only disconnected from.
    await host.shutdown();
    const detail = stderr.length > 0 ? `\n${stderr}` : "";
    process.stderr.write(
      `rustx-tui: could not open a Session: ${(error as Error).message}${detail}\n`,
    );
    return 1;
  }

  const app = new RustxTuiApp({
    host,
    reconnect: parsed.mode.kind === "remote" ? () => connect(parsed) : undefined,
    session,
    sessionSettings: parsed.sessionSettings,
    openSessionSelector: parsed.routing.openSessionSelector,
    cwd: parsed.sessionSettings.cwd,
  });
  return app.run();
}

const code = await main(process.argv.slice(2));
process.exitCode = code;
