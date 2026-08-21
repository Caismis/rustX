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
import { ConnectionClosedError, RuntimeRequestError } from "../src/runtime/connection.ts";
import { emptyPresentationState } from "../src/presentation/projection.ts";
import type { ChildRuntimeProcess } from "../src/runtime/child-process.ts";
import type { RuntimeClientConnection } from "../src/runtime/connection.ts";
import type { RuntimeClientAttachment } from "../src/runtime/attachment.ts";
import type { SessionSummaryView } from "../src/protocol/types.ts";
import {
  attemptView,
  catalogModel,
  sessionModel,
  sessionView,
} from "./support/fixtures.ts";

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

function tick(): Promise<void> {
  // Pi's ProcessTerminal deliberately coalesces raw stdin for 10ms so an
  // escape byte can be distinguished from the prefix of a longer sequence.
  // Wait for that parser boundary rather than racing it with the next test
  // input.
  return new Promise((resolve) => setTimeout(resolve, 20));
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

  it("keeps Esc precedence at the app input-routing boundary", async () => {
    let cancelled = 0;
    const runningState = {
      ...emptyPresentationState(sessionModel("alpha/model-a")),
      attempt: {
        ...attemptView(),
        phase: { type: "running" as const },
      },
    };
    const session = fakeSession(async () => {}, runningState);
    const sessionApi = session as unknown as {
      cancelCurrentAttempt: () => Promise<string>;
      listSessions: () => Promise<{ sessions: SessionSummaryView[]; nextOffset?: number }>;
      refreshSession: () => Promise<ReturnType<typeof sessionView>>;
    };
    sessionApi.cancelCurrentAttempt = async () => {
      cancelled += 1;
      return "a1";
    };
    sessionApi.listSessions = async () => ({
      sessions: [{
        id: "session-1",
        name: "current",
        updated_at: "2026-08-21T00:00:00Z",
        active_node: "node-1",
        active: true,
      }],
    });
    sessionApi.refreshSession = async () => sessionView();

    const app = new RustxTuiApp({
      session,
      connection: fakeConnection(),
      child: fakeChild([]),
    });
    const running = app.run();

    // Overlay open: Esc closes it and must not reach /cancel, even though
    // the authoritative presentation says an attempt is unsettled.
    process.stdin.emit("data", "/resume\r");
    await tick();
    process.stdin.emit("data", "\u001b");
    await tick();
    assert.equal(cancelled, 0);

    // No overlay: the same Esc input reaches the existing /cancel route once.
    process.stdin.emit("data", "\u001b");
    await tick();
    assert.equal(cancelled, 1);

    await app.quit();
    await running;
  });

  it("replaces a terminal Session attachment from the authoritative restart", async () => {
    const log: string[] = [];
    const oldSession = fakeSession(
      async () => {},
      emptyPresentationState(sessionModel("alpha/model-a")),
    );
    const oldApi = oldSession as unknown as {
      newSession: () => Promise<never>;
      detach: () => Promise<void>;
      refreshSession: () => Promise<ReturnType<typeof sessionView>>;
    };
    oldApi.newSession = async () => {
      throw new RuntimeRequestError({
        type: "session_restart_required",
        message: "catalog visibility committed; durability uncertain",
      });
    };
    oldApi.detach = async () => {
      log.push("detach");
    };
    oldApi.refreshSession = async () => sessionView();

    const nextSession = fakeSession(
      async () => {},
      emptyPresentationState(sessionModel("alpha/model-a")),
    );
    const nextApi = nextSession as unknown as {
      refreshSession: () => Promise<ReturnType<typeof sessionView>>;
    };
    nextApi.refreshSession = async () => {
      log.push("refresh_authoritative");
      return sessionView({ id: "session-2", name: "authoritative destination" });
    };

    const app = new RustxTuiApp({
      session: oldSession,
      connection: fakeConnection(),
      child: fakeChild(log),
      restartRuntime: async () => {
        log.push("restart");
        return {
          session: nextSession,
          connection: fakeConnection(),
          child: fakeChild(log),
        };
      },
    });
    const running = app.run();
    process.stdin.emit("data", "/new\r");
    for (let index = 0; index < 10 && !log.includes("refresh_authoritative"); index += 1) {
      await tick();
    }

    assert.deepEqual(log.slice(0, 4), [
      "detach",
      "close_stdin",
      "wait_exit",
      "restart",
    ]);
    assert.ok(log.includes("refresh_authoritative"));

    await app.quit();
    await running;
  });

  it("restores a committed fork draft only after authoritative restart metadata", async () => {
    const prompt = "fork-draft-exact-7f3b";
    const log: string[] = [];
    let refreshed!: () => void;
    const refreshedObserved = new Promise<void>((resolve) => {
      refreshed = resolve;
    });
    let submitted!: (content: string) => void;
    const submittedObserved = new Promise<string>((resolve) => {
      submitted = resolve;
    });

    const oldSession = fakeSession(
      async () => {},
      emptyPresentationState(sessionModel("alpha/model-a")),
    );
    const oldApi = oldSession as unknown as {
      newSession: () => Promise<unknown>;
      detach: () => Promise<void>;
    };
    oldApi.newSession = async () => ({
      session: sessionView({ id: "session-2", name: "committed fork" }),
      editorContent: [{ type: "text", text: prompt }],
      restartRequired: true,
      restartDiagnostic: "catalog visibility committed; durability uncertain",
    });
    oldApi.detach = async () => {
      log.push("detach");
    };

    const nextSession = fakeSession(
      async () => {},
      emptyPresentationState(sessionModel("alpha/model-a")),
    );
    const nextApi = nextSession as unknown as {
      refreshSession: () => Promise<ReturnType<typeof sessionView>>;
      submitInbound: (content: Array<{ type: "text"; text: string }>) => Promise<{
        messageId: string;
        sequence: number;
      }>;
    };
    nextApi.refreshSession = async () => {
      refreshed();
      return sessionView({ id: "session-2", name: "authoritative committed fork" });
    };
    nextApi.submitInbound = async (content) => {
      submitted(content.map((block) => block.text).join("\n"));
      return { messageId: "destination-user-1", sequence: 1 };
    };

    const app = new RustxTuiApp({
      session: oldSession,
      connection: fakeConnection(),
      child: fakeChild(log),
      restartRuntime: async () => ({
        session: nextSession,
        connection: fakeConnection(),
        child: fakeChild(log),
      }),
    });
    const running = app.run();
    process.stdin.emit("data", "/new\r");
    await refreshedObserved;
    // The refresh promise resolves at the native metadata boundary; allow
    // the app's awaited replacement continuation to install the draft before
    // the next parser event is delivered.
    await tick();

    process.stdin.emit("data", "\r");
    await tick();
    assert.equal(await submittedObserved, prompt);
    assert.deepEqual(log.slice(0, 3), ["detach", "close_stdin", "wait_exit"]);

    await app.quit();
    await running;
  });

  it("routes interactive model replacement through the terminal Session flow", async () => {
    const log: string[] = [];
    let refreshStarted!: () => void;
    const refreshObserved = new Promise<void>((resolve) => {
      refreshStarted = resolve;
    });
    const oldSession = fakeSession(
      async () => {},
      emptyPresentationState(sessionModel("alpha/model-a")),
    );
    const oldApi = oldSession as unknown as {
      modelCatalog: () => Promise<{ models: ReturnType<typeof catalogModel>[] }>;
      modelSet: () => Promise<never>;
      refreshSession: () => Promise<ReturnType<typeof sessionView>>;
      detach: () => Promise<void>;
    };
    oldApi.modelCatalog = async () => ({
      models: [
        catalogModel("alpha/model-a"),
        catalogModel("beta/model-b"),
      ],
    });
    oldApi.modelSet = async () => {
      throw new RuntimeRequestError({
        type: "session_restart_required",
        message: "model catalog visibility committed; durability uncertain",
      });
    };
    oldApi.refreshSession = async () => sessionView();
    oldApi.detach = async () => {
      log.push("detach");
    };

    const nextSession = fakeSession(
      async () => {},
      emptyPresentationState(sessionModel("beta/model-b")),
    );
    const nextApi = nextSession as unknown as {
      refreshSession: () => Promise<ReturnType<typeof sessionView>>;
    };
    nextApi.refreshSession = async () => {
      log.push("refresh_authoritative");
      refreshStarted();
      return sessionView({ id: "session-2", name: "authoritative model session" });
    };

    const app = new RustxTuiApp({
      session: oldSession,
      connection: fakeConnection(),
      child: fakeChild(log),
      restartRuntime: async () => {
        log.push("restart");
        return {
          session: nextSession,
          connection: fakeConnection(),
          child: fakeChild(log),
        };
      },
    });
    const running = app.run();

    process.stdin.emit("data", "/model\r");
    await tick();
    process.stdin.emit("data", "\u001b[B");
    await tick();
    process.stdin.emit("data", "\r");
    await refreshObserved;

    assert.deepEqual(log.slice(0, 5), [
      "detach",
      "close_stdin",
      "wait_exit",
      "restart",
      "refresh_authoritative",
    ]);

    await app.quit();
    await running;
  });
});
