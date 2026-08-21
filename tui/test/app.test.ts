/**
 * The controlling client's shutdown sequencing.
 *
 * The runtime/process pieces are deliberately controlled at their boundaries
 * here: this test proves that the UI waits for the authoritative settlement
 * fact before it sends stdin EOF, while the real-child integration exercises
 * the final process lifecycle.
 */

import assert from "node:assert/strict";
import { describe, it } from "node:test";

import { RustxTuiApp } from "../src/ui/app.ts";
import { ConnectionClosedError } from "../src/runtime/connection.ts";
import { emptyPresentationState } from "../src/presentation/projection.ts";
import type { ChildRuntimeProcess } from "../src/runtime/child-process.ts";
import type { RuntimeClientConnection } from "../src/runtime/connection.ts";
import type { RuntimeClientAttachment } from "../src/runtime/attachment.ts";
import { sessionModel, sessionView } from "./support/fixtures.ts";

function fakeConnection(
  onClose?: (listener: (error: ConnectionClosedError) => void) => void,
): RuntimeClientConnection {
  return {
    pendingCount: 0,
    closed: undefined,
    close: () => {},
    onEvent: () => () => {},
    onClose: (listener: (error: ConnectionClosedError) => void) => {
      onClose?.(listener);
      return () => {};
    },
  } as unknown as RuntimeClientConnection;
}

function fakeSession(
  waitForSettlement: (attemptId: string) => Promise<unknown>,
  state: unknown = {
    attempt: {
      attemptId: "attempt-1",
      phase: { type: "running" as const },
    },
  },
): RuntimeClientAttachment {
  return {
    state,
    onState: () => () => {},
    updateState: () => {},
    shutdown: async () => {},
    waitForAttemptSettlement: waitForSettlement,
  } as unknown as RuntimeClientAttachment;
}

function fakeChild(log: string[]): ChildRuntimeProcess {
  return {
    closeStdin: () => log.push("close_stdin"),
    waitOrTerminate: async () => {
      log.push("wait_exit");
      return { code: 0, signal: null };
    },
    stderrTail: () => ({ text: "", truncatedBytes: 0 }),
    exited: undefined,
    pid: 1,
  } as unknown as ChildRuntimeProcess;
}

describe("RustxTuiApp lifecycle", () => {
  it("keeps stdin open until the exact attempt settlement is observed", async () => {
    const log: string[] = [];
    let beginSettlement!: () => void;
    let settle!: () => void;
    const settlementStarted = new Promise<void>((resolve) => {
      beginSettlement = resolve;
    });
    const settlement = new Promise<void>((resolve) => {
      settle = resolve;
    });
    const session = fakeSession(async (attemptId) => {
      assert.equal(attemptId, "attempt-1");
      log.push("wait_settlement");
      beginSettlement();
      await settlement;
    });
    const app = new RustxTuiApp({
      session,
      connection: fakeConnection(),
      child: fakeChild(log),
    });

    const quitting = app.quit();
    await settlementStarted;
    assert.deepEqual(log, ["wait_settlement"]);

    settle();
    await quitting;
    assert.deepEqual(log, [
      "wait_settlement",
      "close_stdin",
      "wait_exit",
    ]);
  });

  it("settles run immediately when the connection was already terminal", async () => {
    const app = new RustxTuiApp({
      session: fakeSession(async () => {}),
      connection: fakeConnection((listener) =>
        listener(
          new ConnectionClosedError(
            "process_exit",
            "the process was already gone",
          ),
        ),
      ),
      child: fakeChild([]),
    });

    assert.equal(await app.run(), 1);
  });

  it("finishes with failure when a session restart cannot be launched", async () => {
    const log: string[] = [];
    const session = fakeSession(
      async () => {},
      emptyPresentationState(sessionModel("alpha/model-a")),
    );
    (session as unknown as {
      newSession: () => Promise<unknown>;
    }).newSession = async () => ({
      session: sessionView({ id: "session-2" }),
      restartRequired: true,
    });
    const app = new RustxTuiApp({
      session,
      connection: fakeConnection(),
      child: fakeChild(log),
      restartRuntime: async () => {
        log.push("restart");
        throw new Error("spawn failed");
      },
    });

    const running = app.run();
    process.stdin.emit("data", "/new\r");

    assert.equal(await running, 1);
    assert.deepEqual(log, ["close_stdin", "wait_exit", "restart"]);
  });
});
