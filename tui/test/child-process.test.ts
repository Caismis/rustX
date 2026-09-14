/**
 * The OS process owner of a TUI-owned App Server child.
 *
 * These drive a stand-in binary rather than `rustx`: the subject is lifecycle
 * mechanics — the argument contract, streams, a bounded stderr tail, stdin
 * close, wait, and the fallback termination — not protocol behaviour. The real
 * binary is exercised by the integration suite.
 *
 * Everything here is a *process* fact. A child that exits has cancelled
 * nothing, settled nothing, and completed no background work; it has only
 * stopped running.
 */

import assert from "node:assert/strict";
import { chmodSync } from "node:fs";
import { fileURLToPath } from "node:url";
import { describe, it } from "node:test";

import {
  AppServerChild,
  appServerArguments,
  type AppServerLaunchOptions,
} from "../src/app-server/child-process.ts";

const FAKE_RUNTIME = fileURLToPath(
  new URL("./support/fake-runtime.mjs", import.meta.url),
);
chmodSync(FAKE_RUNTIME, 0o755);

const LAUNCH: AppServerLaunchOptions = {
  userSettings: "/private/user/settings.toml",
  models: "/models.toml",
  runtimeRoot: "/private/state",
};

function spawn(env: NodeJS.ProcessEnv = {}): AppServerChild {
  return AppServerChild.spawn({
    binary: FAKE_RUNTIME,
    launch: LAUNCH,
    env: { ...process.env, ...env },
  });
}

/** Reads the child's stdout to end. */
function readAll(child: AppServerChild): Promise<string> {
  return new Promise((resolve) => {
    const chunks: Buffer[] = [];
    child.stdout.on("data", (chunk: Buffer) => chunks.push(chunk));
    child.stdout.on("end", () => resolve(Buffer.concat(chunks).toString("utf8")));
  });
}

describe("AppServerChild", () => {
  it("always selects the stdio App Server, and nothing else", () => {
    // The subcommand and the transport selection are not options: this owner
    // exists precisely to run one `rustx app-server --listen stdio` child.
    assert.deepEqual(appServerArguments({}), ["app-server", "--listen", "stdio"]);
  });

  it("forwards only process-level source bindings, in a deterministic order", async () => {
    const child = spawn();
    const output = readAll(child);
    child.closeStdin();
    await child.wait();

    // Every flag here binds a source for the whole App Server *process*. None
    // of them is a Session selection: a Session's cwd and project
    // configuration travel in `session/create`, never in this argv.
    assert.deepEqual(JSON.parse((await output).trim()), [
      "app-server",
      "--user-settings",
      "/private/user/settings.toml",
      "--models",
      "/models.toml",
      "--runtime-root",
      "/private/state",
      "--listen",
      "stdio",
    ]);
  });

  it("omits every binding the caller did not supply", async () => {
    const child = AppServerChild.spawn({
      binary: FAKE_RUNTIME,
      launch: {},
      env: process.env,
    });
    const output = readAll(child);
    child.closeStdin();
    await child.wait();

    // An omitted binding keeps the App Server's own canonical default. The
    // client never invents a path, and never reads one.
    assert.deepEqual(JSON.parse((await output).trim()), [
      "app-server",
      "--listen",
      "stdio",
    ]);
  });

  it("exits cleanly on stdin EOF", async () => {
    const child = spawn({ FAKE_EXIT_CODE: "0" });
    child.closeStdin();
    const exit = await child.wait();

    assert.equal(exit.code, 0);
    assert.deepEqual(child.exited, exit);
  });

  it("surfaces a non-zero exit code", async () => {
    const child = spawn({ FAKE_EXIT_CODE: "7" });
    child.closeStdin();
    assert.equal((await child.wait()).code, 7);
  });

  it("keeps only a bounded stderr tail and counts what it dropped", async () => {
    const child = AppServerChild.spawn({
      binary: FAKE_RUNTIME,
      launch: LAUNCH,
      env: { ...process.env, FAKE_STDERR_BYTES: "4096" },
      stderrTailBytes: 256,
    });
    child.closeStdin();
    await child.wait();

    const tail = child.stderrTail();
    assert.equal(tail.text.length, 256, "the tail never grows past its bound");
    assert.equal(
      tail.truncatedBytes,
      4096 - 256,
      "dropped bytes are reported rather than silently hidden",
    );
  });

  it("escalates only after the grace period, and only as a fallback", async () => {
    const child = spawn({ FAKE_IGNORE_EOF: "1" });
    child.closeStdin();

    // The child deliberately ignores EOF, so the bounded process-level
    // fallback is what ends it. This says nothing semantic: no attempt was
    // cancelled and no background work was settled by it.
    const exit = await child.waitOrTerminate(200);
    assert.ok(
      exit.signal !== null || exit.code !== null,
      "the fallback terminated the process",
    );
  });

  it("returns the normal exit when the child leaves within its grace", async () => {
    const child = spawn({ FAKE_EXIT_CODE: "3" });
    child.closeStdin();

    const exit = await child.waitOrTerminate(10_000);
    assert.equal(exit.code, 3);
    assert.equal(exit.signal, null, "no escalation was needed");
  });

  it("reports a spawn failure as an exit, with the reason", async () => {
    const child = AppServerChild.spawn({
      binary: "/definitely/not/a/binary",
      launch: LAUNCH,
      env: process.env,
    });
    const exit = await child.wait();

    assert.equal(exit.code, null, "a failed spawn has no exit code");
    assert.equal(exit.signal, null);
    // Without the reason a spawn failure would be indistinguishable from an
    // unexplained disappearance, and the startup diagnostic would be useless.
    assert.match(exit.spawnError ?? "", /ENOENT/);
  });

  it("closing stdin is idempotent", async () => {
    const child = spawn();
    assert.doesNotThrow(() => child.closeStdin());
    assert.doesNotThrow(() => child.closeStdin());
    await child.wait();
  });
});
