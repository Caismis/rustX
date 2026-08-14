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
 *   -> initialize + snapshot + subscribe (RuntimeClientSession)
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
import { RuntimeClientSession } from "./runtime/session.ts";
import { RustxTuiApp } from "./ui/app.ts";

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

  // The child inherits this process's environment so rustX performs its own
  // credential resolution.
  const child = ChildRuntimeProcess.spawn({
    binary: parsed.binary,
    paths: parsed.paths,
  });

  const connection = new RuntimeClientConnection({
    input: child.stdout,
    output: child.stdin,
  });
  // A process that dies mid-request must fail that request with the real
  // cause rather than a bare EOF.
  void child.wait().then((exit) => {
    connection.reportProcessExit(exit.code, exit.signal, exit.spawnError);
  });

  const session = new RuntimeClientSession({ connection });
  try {
    await session.attach();
  } catch (error) {
    process.stderr.write(
      `rustx-tui: could not attach to the runtime: ${(error as Error).message}\n`,
    );
    // The runtime's own bounded startup diagnostic is usually the real cause,
    // and it already carries its own `rustx: ` prefix.
    const stderr = child.stderrTail().text.trim();
    if (stderr.length > 0) {
      process.stderr.write(`${stderr}\n`);
    }
    child.closeStdin();
    await child.waitOrTerminate();
    return 1;
  }

  const app = new RustxTuiApp({ session, connection, child });
  return app.run();
}

const code = await main(process.argv.slice(2));
process.exitCode = code;
