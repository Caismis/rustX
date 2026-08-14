/**
 * The OS process owner.
 *
 * These drive a stand-in binary rather than `rustx`: the subject is lifecycle
 * mechanics — the argument contract, streams, a bounded stderr tail, stdin
 * close, wait, and the fallback termination — not protocol behaviour. The real
 * binary is exercised by the integration suite.
 */

import assert from "node:assert/strict";
import { chmodSync } from "node:fs";
import { fileURLToPath } from "node:url";
import { describe, it } from "node:test";

import {
  ChildRuntimeProcess,
  type RuntimePaths,
} from "../src/runtime/child-process.ts";

const FAKE_RUNTIME = fileURLToPath(
  new URL("./support/fake-runtime.mjs", import.meta.url),
);
chmodSync(FAKE_RUNTIME, 0o755);

const PATHS: RuntimePaths = {
  models: "/models.json",
  session: "/session.json",
  workspace: "/ws",
  runtimeRoot: "/private",
};

function spawn(env: NodeJS.ProcessEnv = {}): ChildRuntimeProcess {
  return ChildRuntimeProcess.spawn({
    binary: FAKE_RUNTIME,
    paths: PATHS,
    env: { ...process.env, ...env },
  });
}

/** Reads the child's stdout to end. */
function readAll(child: ChildRuntimeProcess): Promise<string> {
  return new Promise((resolve) => {
    const chunks: Buffer[] = [];
    child.stdout.on("data", (chunk: Buffer) => chunks.push(chunk));
    child.stdout.on("end", () => resolve(Buffer.concat(chunks).toString("utf8")));
  });
}

describe("ChildRuntimeProcess", () => {
  it("passes the explicit startup paths through verbatim", async () => {
    const child = spawn();
    const output = readAll(child);
    child.closeStdin();
    await child.wait();

    // The paths reach the binary exactly as given. Nothing here opened,
    // parsed, validated, or defaulted any of them.
    assert.deepEqual(JSON.parse((await output).trim()), [
      "--models",
      "/models.json",
      "--session",
      "/session.json",
      "--workspace",
      "/ws",
      "--runtime-root",
      "/private",
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
    const child = ChildRuntimeProcess.spawn({
      binary: FAKE_RUNTIME,
      paths: PATHS,
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

  it("reports a spawn failure as an exit rather than hanging", async () => {
    const child = ChildRuntimeProcess.spawn({
      binary: "/definitely/not/a/binary",
      paths: PATHS,
      env: process.env,
    });
    assert.equal((await child.wait()).code, null);
  });

  it("closing stdin is idempotent", async () => {
    const child = spawn();
    assert.doesNotThrow(() => child.closeStdin());
    assert.doesNotThrow(() => child.closeStdin());
    await child.wait();
  });
});
