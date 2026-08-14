#!/usr/bin/env node
/**
 * A stand-in for the `rustx` binary, for process-lifecycle tests only.
 *
 * It speaks no protocol. It echoes the startup arguments it received on
 * stdout, writes a configurable amount of stderr, and exits on stdin EOF —
 * exactly the mechanics `ChildRuntimeProcess` owns.
 *
 * Behaviour is selected through the environment so the argument vector stays
 * the real rustX contract:
 *
 *   FAKE_STDERR_BYTES  bytes of stderr noise to emit (default 0)
 *   FAKE_IGNORE_EOF    stay alive after stdin EOF, to exercise termination
 *   FAKE_EXIT_CODE     exit code to use on a clean EOF (default 0)
 */

const argv = process.argv.slice(2);
process.stdout.write(`${JSON.stringify(argv)}\n`);

const noise = Number(process.env.FAKE_STDERR_BYTES ?? "0");
if (noise > 0) {
  process.stderr.write("x".repeat(noise));
}

if (process.env.FAKE_IGNORE_EOF === "1") {
  // Hold the process open so the caller must escalate.
  setInterval(() => {}, 1_000);
} else {
  process.stdin.resume();
  process.stdin.on("end", () => {
    process.exit(Number(process.env.FAKE_EXIT_CODE ?? "0"));
  });
}
