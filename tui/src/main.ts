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
 *   -> choose startup focus   (explicit attachment, new Session, or catalog picker)
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
import { prepareStartup, type StartupFocus } from "./startup.ts";
import { RustxTuiApp } from "./ui/app.ts";
import {
  configurationCommand,
  forwardConfigurationCommand,
} from "./configuration-command.ts";

async function connect(parsed: TuiArguments, signal?: AbortSignal): Promise<AppServerHost> {
  if (parsed.mode.kind === "local") {
    return AppServerHost.spawnLocal({
      binary: parsed.mode.binary,
      signal,
      launch: parsed.mode.launch,
    });
  }
  // At most one trailing newline, exactly as the server's own token file
  // contract allows. Nothing else about the value is interpreted here.
  const token = (await readFile(parsed.mode.tokenFile, "utf8")).replace(/\n$/, "");
  signal?.throwIfAborted();
  return AppServerHost.connectRemote({
    endpoint: parsed.mode.endpoint,
    token,
  });
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

  const controller = new AbortController();
  let host: AppServerHost | undefined;
  let app: RustxTuiApp | undefined;
  const stop = () => {
    controller.abort();
    if (app) void app.quit();
    else if (host) void host.shutdown();
  };
  const message = (value: unknown) => {
    if (typeof value === "object" && value !== null && "stop" in value && value.stop === true) stop();
  };
  process.on("SIGINT", stop);
  process.on("SIGTERM", stop);
  process.on("message", message);
  process.on("disconnect", stop);
  try {
    host = await connect(parsed, controller.signal);
    controller.signal.throwIfAborted();
    const focus: StartupFocus = await prepareStartup(host, parsed);
    controller.signal.throwIfAborted();
    app = new RustxTuiApp({
      host,
      reconnect: parsed.mode.kind === "remote" ? () => connect(parsed, controller.signal) : undefined,
      ...focus,
      sessionSettings: parsed.sessionSettings,
      cwd: parsed.sessionSettings.cwd,
    });
    return await app.run();
  } catch (error) {
    if (controller.signal.aborted) return 0;
    const stderr = host?.stderrTail().text.trim();
    process.stderr.write(`rustx-tui: ${(error as Error).message}${stderr ? `\n${stderr}` : ""}\n`);
    return 1;
  } finally {
    await host?.shutdown();
    process.off("SIGINT", stop);
    process.off("SIGTERM", stop);
    process.off("message", message);
    process.off("disconnect", stop);
  }
}

const code = await main(process.argv.slice(2));
if (process.connected) process.disconnect();
process.exitCode = code;
