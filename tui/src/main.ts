#!/usr/bin/env node
/**
 * `rustx-tui` — the rustX reference terminal client.
 *
 * Startup is the composition root and nothing more:
 *
 * ```text
 * parse arguments
 *   -> spawn the rustx binary        (ChildRuntimeProcess)
 *   -> open the JSONL transport      (RuntimeClientConnection)
 *   -> initialize + snapshot + subscribe (RuntimeClientAttachment)
 *   -> run the terminal projection   (RustxTuiApp)
 * ```
 *
 * A startup failure is reported on stderr with a bounded diagnostic and a
 * non-zero exit. The client resolves no credential and reads no runtime
 * configuration file of its own.
 */

import { ArgumentError, USAGE, parseArguments } from "./cli.ts";
import { ChildRuntimeProcess } from "./runtime/child-process.ts";
import { RuntimeClientConnection } from "./runtime/connection.ts";
import { RuntimeClientAttachment } from "./runtime/attachment.ts";
import { RustxTuiApp, type RuntimeAttachmentHandle } from "./ui/app.ts";
import type { TuiArguments } from "./cli.ts";

async function startRuntime(parsed: TuiArguments): Promise<RuntimeAttachmentHandle> {
  const child = ChildRuntimeProcess.spawn({
    binary: parsed.binary,
    paths: parsed.paths,
  });

  const connection = new RuntimeClientConnection({
    input: child.stdout,
    output: child.stdin,
  });
  void child.wait().then((exit) => {
    connection.reportProcessExit(exit.code, exit.signal, exit.spawnError);
  });

  const session = new RuntimeClientAttachment({ connection });
  try {
    await session.attach();
  } catch (error) {
    const stderr = child.stderrTail().text.trim();
    child.closeStdin();
    await child.waitOrTerminate();
    const detail = stderr.length > 0 ? `\n${stderr}` : "";
    throw new Error(
      `could not attach to the runtime: ${(error as Error).message}${detail}`,
    );
  }
  return { session, connection, child };
}

async function main(argv: readonly string[]): Promise<number> {
  let parsed;
  try {
    parsed = parseArguments(argv);
  } catch (error) {
    if (error instanceof ArgumentError) {
      process.stderr.write(`rustx-tui: ${error.message}\n${USAGE}\n`);
      return 2;
    }
    throw error;
  }

  let runtime: RuntimeAttachmentHandle;
  try {
    runtime = await startRuntime(parsed);
  } catch (error) {
    process.stderr.write(`rustx-tui: ${(error as Error).message}\n`);
    return 1;
  }

  const app = new RustxTuiApp({
    ...runtime,
    restartRuntime: () => startRuntime(parsed),
  });
  return app.run();
}

const code = await main(process.argv.slice(2));
process.exitCode = code;
