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
import { TUI } from "@earendil-works/pi-tui";

import { RustxTuiApp } from "../src/ui/app.ts";
import { ConnectionClosedError, RuntimeRequestError } from "../src/runtime/connection.ts";
import { emptyPresentationState } from "../src/presentation/projection.ts";
import { TransientFeedbackSurface } from "../src/ui/components/transient-feedback.ts";
import type { ChildRuntimeProcess } from "../src/runtime/child-process.ts";
import type { RuntimeClientConnection } from "../src/runtime/connection.ts";
import type { RuntimeClientAttachment } from "../src/runtime/attachment.ts";
import type { SessionSummaryView } from "../src/protocol/types.ts";
import {
  attemptView,
  approvalInteraction,
  childApprovalInteraction,
  childQuestionnaireInteraction,
  questionnaireInteraction,
  catalogModel,
  sessionModel,
  sessionView,
  subagent,
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
  const stateListeners = new Set<(nextState: unknown) => void>();
  const snapshotListeners = new Set<() => void>();
  const session = {
    state,
    onState: (listener: (nextState: unknown) => void) => {
      stateListeners.add(listener);
      return () => stateListeners.delete(listener);
    },
    onSnapshot: (listener: () => void) => {
      snapshotListeners.add(listener);
      return () => snapshotListeners.delete(listener);
    },
    updateState: () => {},
    shutdown: async () => {},
    waitForAttemptSettlement: waitForSettlement,
    publishState(nextState: unknown): void {
      session.state = nextState;
      for (const listener of stateListeners) listener(nextState);
    },
    publishSnapshot(): void {
      for (const listener of snapshotListeners) listener();
    },
  };
  return session as unknown as RuntimeClientAttachment;
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

function waitForPiEscapeDisambiguation(): Promise<void> {
  // This is the one wall-clock wait in these app tests. Pi's ProcessTerminal
  // deliberately holds a bare ESC for its disambiguation window so it can
  // distinguish ESC from the prefix of a longer sequence. This helper waits
  // for that third-party parser boundary; it does not synchronize rustX
  // runtime or Session semantics.
  return new Promise((resolve) => setTimeout(resolve, 20));
}

function waitForApplicationContinuation(): Promise<void> {
  // Let the promise chain that handles one observed Runtime Client response
  // finish before the next synthetic input event. This is an event-loop
  // continuation, not an elapsed-time synchronization primitive.
  return new Promise((resolve) => setImmediate(resolve));
}

function deferred<T>(): {
  promise: Promise<T>;
  resolve: (value: T) => void;
  reject: (error: unknown) => void;
} {
  let resolve!: (value: T) => void;
  let reject!: (error: unknown) => void;
  const promise = new Promise<T>((promiseResolve, promiseReject) => {
    resolve = promiseResolve;
    reject = promiseReject;
  });
  return { promise, resolve, reject };
}

function countTuiRenderRequests(): {
  readonly count: () => number;
  readonly start: () => void;
  readonly restore: () => void;
} {
  const prototype = TUI.prototype as unknown as {
    requestRender: (force?: boolean) => void;
  };
  const original = prototype.requestRender;
  let enabled = false;
  let count = 0;
  prototype.requestRender = function(force?: boolean): void {
    if (enabled) count += 1;
    original.call(this, force);
  };
  return {
    count: () => count,
    start: () => {
      enabled = true;
      count = 0;
    },
    restore: () => {
      prototype.requestRender = original;
    },
  };
}

describe("RustxTuiApp lifecycle", () => {
  it("opens a selected child by child_conversation_id and returns with Esc", async () => {
    const parentState = {
      ...emptyPresentationState(sessionModel("alpha/model-a")),
      subagents: [subagent("explore", "sha256:child")],
    };
    const parent = fakeSession(async () => {}, parentState);
    const parentApi = parent as unknown as {
      identity: { conversationId: string };
    };
    parentApi.identity = { conversationId: "conversation-parent" };

    const child = fakeSession(async () => {}, emptyPresentationState(sessionModel("alpha/model-a")));
    const childApi = child as unknown as {
      identity: { conversationId: string };
    };
    childApi.identity = { conversationId: "conversation-child" };
    const childLog: string[] = [];
    const opened: string[] = [];
    const app = new RustxTuiApp({
      session: parent,
      connection: fakeConnection(),
      child: fakeChild([]),
      openConversation: async (conversationId) => {
        opened.push(conversationId);
        return { session: child, connection: fakeConnection(), child: fakeChild(childLog) };
      },
    });
    const running = app.run();

    // Ctrl+Down enters the explicit subagent-list focus; plain Enter remains
    // reserved for the editor unless that focus is active.
    process.stdin.emit("data", "\x1b[1;5B");
    await waitForApplicationContinuation();
    process.stdin.emit("data", "\r");
    await waitForApplicationContinuation();
    assert.deepEqual(opened, ["conv-1-subagent-1"]);

    process.stdin.emit("data", "\u001b");
    await waitForPiEscapeDisambiguation();
    assert.deepEqual(childLog, ["close_stdin", "wait_exit"]);
    assert.equal(parent.state, parentState, "navigation does not replace parent state");

    await app.quit();
    await running;
  });

  it("detaches a direct read-only inspection without requesting shutdown", async () => {
    const log: string[] = [];
    let shutdownRequested = false;
    const session = fakeSession(async () => {}, {
      ...emptyPresentationState(sessionModel("alpha/model-a")),
    });
    session.shutdown = async () => {
      shutdownRequested = true;
    };
    const sessionApi = session as unknown as {
      identity: { conversationId: string };
    };
    sessionApi.identity = { conversationId: "conversation-child" };
    const app = new RustxTuiApp({
      session,
      connection: fakeConnection(),
      child: fakeChild(log),
      readOnly: true,
    });
    const running = app.run();

    process.stdin.emit("data", "\x03");
    await running;

    assert.equal(shutdownRequested, false);
    assert.deepEqual(log, ["close_stdin", "wait_exit"]);
  });

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

  it("commits a fatal diagnostic before stopping the TUI", async () => {
    const events: string[] = [];
    let close!: (error: ConnectionClosedError) => void;
    const prototype = TUI.prototype as unknown as {
      doRender: () => void;
      stop: () => void;
    };
    const transientPrototype = TransientFeedbackSurface.prototype as unknown as {
      replace: (feedback: { level: "info" | "error"; text: string }) => void;
    };
    const originalRender = prototype.doRender;
    const originalStop = prototype.stop;
    const originalReplace = transientPrototype.replace;
    prototype.doRender = function(): void {
      events.push("render");
      originalRender.call(this);
    };
    prototype.stop = function(): void {
      events.push("stop");
      originalStop.call(this);
    };
    transientPrototype.replace = function(feedback): void {
      if (feedback.text.includes("fatal transport diagnostic")) {
        events.push("fatal_feedback");
      }
      originalReplace.call(this, feedback);
    };

    try {
      const app = new RustxTuiApp({
        session: fakeSession(async () => {}, emptyPresentationState(sessionModel("alpha/model-a"))),
        connection: fakeConnection((listener) => {
          close = listener;
        }),
        child: fakeChild([]),
      });
      const running = app.run();
      await waitForApplicationContinuation();
      events.length = 0;

      close(new ConnectionClosedError("process_exit", "fatal transport diagnostic"));
      assert.equal(await running, 1);
      const renderIndex = events.indexOf("render");
      const stopIndex = events.indexOf("stop");
      assert.deepEqual(events.slice(0, 3), [
        "fatal_feedback",
        "render",
        "stop",
      ]);
      assert.ok(renderIndex >= 0, "fatal path must commit a final frame");
      assert.ok(stopIndex > renderIndex, "TUI stop must follow the final frame");
    } finally {
      prototype.doRender = originalRender;
      prototype.stop = originalStop;
      transientPrototype.replace = originalReplace;
    }
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
    let sessionsListed!: () => void;
    const sessionsListedObserved = new Promise<void>((resolve) => {
      sessionsListed = resolve;
    });
    sessionApi.cancelCurrentAttempt = async () => {
      cancelled += 1;
      return "a1";
    };
    sessionApi.listSessions = async () => {
      sessionsListed();
      return {
        sessions: [{
          id: "session-1",
          name: "current",
          updated_at: "2026-08-21T00:00:00Z",
          active_node: "node-1",
          active: true,
        }],
      };
    };
    sessionApi.refreshSession = async () => sessionView();

    const app = new RustxTuiApp({
      session,
      connection: fakeConnection(),
      child: fakeChild([]),
    });
    const running = app.run();

    // Inspection overlay: Esc closes it and must not reach /cancel, even
    // though the authoritative presentation says an attempt is unsettled.
    process.stdin.emit("data", "/help\r");
    await waitForApplicationContinuation();
    process.stdin.emit("data", "\u001b");
    await waitForPiEscapeDisambiguation();
    assert.equal(cancelled, 0);

    // Overlay open: Esc closes it and must not reach /cancel, even though
    // the authoritative presentation says an attempt is unsettled.
    process.stdin.emit("data", "/resume\r");
    await sessionsListedObserved;
    // The list response is observed above; this microtask lets the awaiting
    // dispatcher install the overlay before the next input event.
    await Promise.resolve();
    process.stdin.emit("data", "\u001b");
    await waitForPiEscapeDisambiguation();
    assert.equal(cancelled, 0);

    // No overlay: the same Esc input reaches the existing /cancel route once.
    process.stdin.emit("data", "\u001b");
    await waitForPiEscapeDisambiguation();
    assert.equal(cancelled, 1);

    await app.quit();
    await running;
  });

  it("opens a questionnaire overlay and submits one typed response", async () => {
    const questionnaire = questionnaireInteraction();
    const state = {
      ...emptyPresentationState(sessionModel("alpha/model-a")),
      attempt: {
        ...attemptView(),
        phase: { type: "running" as const },
      },
      pendingInteractions: [questionnaire],
    };
    const session = fakeSession(async () => {}, state);
    const api = session as unknown as {
      respondInteraction: (
        interaction: { conversation_id: string; interaction_id: string },
        response: unknown,
      ) => Promise<void>;
    };
    let response!: (value: { id: string; response: unknown }) => void;
    const responseObserved = new Promise<{ id: string; response: unknown }>((resolve) => {
      response = resolve;
    });
    api.respondInteraction = async (interaction, typedResponse) => {
      response({
        id: `${interaction.conversation_id}::${interaction.interaction_id}`,
        response: typedResponse,
      });
    };

    const app = new RustxTuiApp({
      session,
      connection: fakeConnection(),
      child: fakeChild([]),
    });
    const running = app.run();
    await waitForApplicationContinuation();
    process.stdin.emit("data", "\r");
    await waitForApplicationContinuation();
    process.stdin.emit("data", "\t");
    await waitForApplicationContinuation();
    process.stdin.emit("data", "\r");

    assert.deepEqual(await responseObserved, {
      id: `${questionnaire.interaction.conversation_id}::${questionnaire.interaction.interaction_id}`,
      response: {
        type: "questionnaire",
        response: {
          type: "submitted",
          value: {
            answers: [{
              question_index: 0,
              answer: { type: "single_option", value: { label: "staging" } },
            }],
          },
        },
      },
    });

    await app.quit();
    await running;
  });

  it("Esc declines only the focused questionnaire", async () => {
    const state = {
      ...emptyPresentationState(sessionModel("alpha/model-a")),
      pendingInteractions: [questionnaireInteraction()],
    };
    const session = fakeSession(async () => {}, state);
    const api = session as unknown as {
      respondInteraction: (
        interaction: { conversation_id: string; interaction_id: string },
        response: unknown,
      ) => Promise<void>;
    };
    let decline!: (value: { id: string; response: unknown }) => void;
    const declineObserved = new Promise<{ id: string; response: unknown }>((resolve) => {
      decline = resolve;
    });
    api.respondInteraction = async (interaction, response) =>
      decline({
        id: `${interaction.conversation_id}::${interaction.interaction_id}`,
        response,
      });

    const app = new RustxTuiApp({
      session,
      connection: fakeConnection(),
      child: fakeChild([]),
    });
    const running = app.run();

    process.stdin.emit("data", "\u001b");
    assert.deepEqual(await declineObserved, {
      id: "conv-test::attempt-1-interaction-question-1",
      response: {
        type: "questionnaire",
        response: { type: "declined" },
      },
    });

    await app.quit();
    await running;
  });

  it("closes a questionnaire overlay when an authoritative remote settlement arrives", async () => {
    const questionnaire = questionnaireInteraction();
    const state = {
      ...emptyPresentationState(sessionModel("alpha/model-a")),
      pendingInteractions: [questionnaire],
    };
    const session = fakeSession(async () => {}, state) as RuntimeClientAttachment & {
      publishState(nextState: unknown): void;
    };
    let responses = 0;
    (session as unknown as {
      respondInteraction: () => Promise<void>;
    }).respondInteraction = async () => {
      responses += 1;
    };

    const app = new RustxTuiApp({
      session,
      connection: fakeConnection(),
      child: fakeChild([]),
    });
    const running = app.run();
    await waitForApplicationContinuation();

    session.publishState({
      ...state,
      pendingInteractions: [],
    });
    await waitForApplicationContinuation();
    process.stdin.emit("data", "\r");
    await waitForApplicationContinuation();

    assert.equal(responses, 0, "remote settlement must close the local overlay");
    await app.quit();
    await running;
  });

  it("Ctrl+C cancels the owning attempt while the questionnaire is focused", async () => {
    const state = {
      ...emptyPresentationState(sessionModel("alpha/model-a")),
      attempt: {
        ...attemptView(),
        phase: { type: "running" as const },
      },
      pendingInteractions: [questionnaireInteraction()],
    };
    const session = fakeSession(async () => {}, state);
    const api = session as unknown as {
      cancelCurrentAttempt: () => Promise<string>;
    };
    let resolveCancellation!: () => void;
    const cancellationObserved = new Promise<void>((resolve) => {
      resolveCancellation = resolve;
    });
    let cancelled = 0;
    api.cancelCurrentAttempt = async () => {
      cancelled += 1;
      resolveCancellation();
      return "attempt-1";
    };

    const app = new RustxTuiApp({
      session,
      connection: fakeConnection(),
      child: fakeChild([]),
    });
    const running = app.run();

    process.stdin.emit("data", "\u0003");
    await cancellationObserved;
    assert.equal(cancelled, 1);

    await app.quit();
    await running;
  });

  it("opens the unified surface for a primary approval, preselected on Deny", async () => {
    const approval = approvalInteraction();
    const state = {
      ...emptyPresentationState(sessionModel("alpha/model-a")),
      attempt: { ...attemptView(), phase: { type: "running" as const } },
      pendingInteractions: [approval],
    };
    const session = fakeSession(async () => {}, state);
    const responses: Array<{ id: string; response: unknown }> = [];
    (session as unknown as {
      respondInteraction: (
        interaction: { conversation_id: string; interaction_id: string },
        response: unknown,
      ) => Promise<void>;
    }).respondInteraction = async (interaction, response) => {
      responses.push({
        id: `${interaction.conversation_id}::${interaction.interaction_id}`,
        response,
      });
    };

    const app = new RustxTuiApp({
      session,
      connection: fakeConnection(),
      child: fakeChild([]),
    });
    const running = app.run();
    await waitForApplicationContinuation();

    // A bare Enter on the freshly opened surface answers Deny, never Allow.
    process.stdin.emit("data", "\r");
    await waitForApplicationContinuation();
    assert.deepEqual(responses, [
      {
        id: "conv-test::attempt-1-interaction-1",
        response: {
          type: "approval",
          decision: { type: "deny", reason: "denied by the user" },
        },
      },
    ]);

    await app.quit();
    await running;
  });

  it("allows once only after explicit navigation to the affirmative choice", async () => {
    const approval = approvalInteraction();
    const state = {
      ...emptyPresentationState(sessionModel("alpha/model-a")),
      attempt: { ...attemptView(), phase: { type: "running" as const } },
      pendingInteractions: [approval],
    };
    const session = fakeSession(async () => {}, state);
    const responses: unknown[] = [];
    (session as unknown as {
      respondInteraction: (interaction: unknown, response: unknown) => Promise<void>;
    }).respondInteraction = async (_interaction, response) => {
      responses.push(response);
    };

    const app = new RustxTuiApp({
      session,
      connection: fakeConnection(),
      child: fakeChild([]),
    });
    const running = app.run();
    await waitForApplicationContinuation();

    process.stdin.emit("data", "\u001b[B");
    await waitForApplicationContinuation();
    assert.equal(responses.length, 0, "navigation settles nothing");
    process.stdin.emit("data", "\r");
    await waitForApplicationContinuation();
    assert.deepEqual(responses, [
      { type: "approval", decision: { type: "allow" } },
    ]);

    await app.quit();
    await running;
  });

  it("serves child and primary interactions through one surface by exact ref", async () => {
    const childQuestion = childQuestionnaireInteraction("child-b-interaction-1");
    const approval = approvalInteraction("attempt-1-interaction-approval-a");
    const state = {
      ...emptyPresentationState(sessionModel("alpha/model-a")),
      attempt: { ...attemptView(), phase: { type: "running" as const } },
      pendingInteractions: [approval, childQuestion],
    };
    const session = fakeSession(async () => {}, state) as RuntimeClientAttachment & {
      publishState(nextState: unknown): void;
    };
    const responses: Array<{ id: string; response: unknown }> = [];
    (session as unknown as {
      respondInteraction: (
        interaction: { conversation_id: string; interaction_id: string },
        response: unknown,
      ) => Promise<void>;
    }).respondInteraction = async (interaction, response) => {
      responses.push({
        id: `${interaction.conversation_id}::${interaction.interaction_id}`,
        response,
      });
    };

    const app = new RustxTuiApp({
      session,
      connection: fakeConnection(),
      child: fakeChild([]),
    });
    const running = app.run();
    await waitForApplicationContinuation();

    // Deterministic focus: conv-child-1 sorts before conv-test, so the child
    // questionnaire is focused first. Esc is its explicit typed decline.
    process.stdin.emit("data", "\u001b");
    await waitForPiEscapeDisambiguation();
    await waitForApplicationContinuation();
    assert.deepEqual(responses, [
      {
        id: "conv-child-1::child-b-interaction-1",
        response: { type: "questionnaire", response: { type: "declined" } },
      },
    ]);

    // The runtime settles the declined questionnaire; focus advances to the
    // surviving primary approval, which a bare Enter denies.
    session.publishState({ ...state, pendingInteractions: [approval] });
    await waitForApplicationContinuation();
    process.stdin.emit("data", "\r");
    await waitForApplicationContinuation();
    assert.deepEqual(responses[1], {
      id: "conv-test::attempt-1-interaction-approval-a",
      response: {
        type: "approval",
        decision: { type: "deny", reason: "denied by the user" },
      },
    });

    await app.quit();
    await running;
  });

  it("dismisses an approval with Esc without answering, and Ctrl+G reopens", async () => {
    const approval = approvalInteraction();
    const state = {
      ...emptyPresentationState(sessionModel("alpha/model-a")),
      attempt: { ...attemptView(), phase: { type: "running" as const } },
      pendingInteractions: [approval],
    };
    const session = fakeSession(async () => {}, state);
    const responses: unknown[] = [];
    (session as unknown as {
      respondInteraction: (interaction: unknown, response: unknown) => Promise<void>;
    }).respondInteraction = async (_interaction, response) => {
      responses.push(response);
    };

    const app = new RustxTuiApp({
      session,
      connection: fakeConnection(),
      child: fakeChild([]),
    });
    const running = app.run();
    await waitForApplicationContinuation();

    // Esc dismisses the surface; nothing is answered, and ordinary editor
    // submission stays ordinary (an empty editor submits nothing at all).
    process.stdin.emit("data", "\u001b");
    await waitForPiEscapeDisambiguation();
    await waitForApplicationContinuation();
    assert.equal(responses.length, 0);
    process.stdin.emit("data", "\r");
    await waitForApplicationContinuation();
    assert.equal(responses.length, 0, "editor Enter never answers an approval");

    // Ctrl+G reopens the surface; the same Enter now answers Deny.
    process.stdin.emit("data", "\u0007");
    await waitForApplicationContinuation();
    process.stdin.emit("data", "\r");
    await waitForApplicationContinuation();
    assert.deepEqual(responses, [
      { type: "approval", decision: { type: "deny", reason: "denied by the user" } },
    ]);

    await app.quit();
    await running;
  });

  it("a distinct new arrival supersedes an approval dismissal", async () => {
    const approval = approvalInteraction("attempt-1-interaction-approval-a");
    const question = questionnaireInteraction("attempt-1-interaction-question-b");
    const state = {
      ...emptyPresentationState(sessionModel("alpha/model-a")),
      attempt: { ...attemptView(), phase: { type: "running" as const } },
      pendingInteractions: [approval],
    };
    const session = fakeSession(async () => {}, state) as RuntimeClientAttachment & {
      publishState(nextState: unknown): void;
    };
    const responses: Array<{ id: string; response: unknown }> = [];
    (session as unknown as {
      respondInteraction: (
        interaction: { conversation_id: string; interaction_id: string },
        response: unknown,
      ) => Promise<void>;
    }).respondInteraction = async (interaction, response) => {
      responses.push({
        id: `${interaction.conversation_id}::${interaction.interaction_id}`,
        response,
      });
    };

    const app = new RustxTuiApp({
      session,
      connection: fakeConnection(),
      child: fakeChild([]),
    });
    const running = app.run();
    await waitForApplicationContinuation();

    // Esc dismisses the approval surface: A stays pending and unanswered.
    process.stdin.emit("data", "\u001b");
    await waitForPiEscapeDisambiguation();
    await waitForApplicationContinuation();
    assert.equal(responses.length, 0);
    // The dismissal holds while the queue is unchanged: Enter answers nothing.
    process.stdin.emit("data", "\r");
    await waitForApplicationContinuation();
    assert.equal(responses.length, 0);

    // A distinct questionnaire arrives. The dismissal of A is scoped to A and
    // cannot hide B: the surface reopens focused on the new arrival, and the
    // focus move itself emits no response.
    session.publishState({ ...state, pendingInteractions: [approval, question] });
    await waitForApplicationContinuation();
    assert.equal(responses.length, 0, "superseding a dismissal settles nothing");

    // The focused panel is the newly arrived questionnaire: Esc is its typed
    // decline, proving B became presented rather than suppressed behind A.
    process.stdin.emit("data", "\u001b");
    await waitForPiEscapeDisambiguation();
    await waitForApplicationContinuation();
    assert.deepEqual(responses, [
      {
        id: "conv-test::attempt-1-interaction-question-b",
        response: { type: "questionnaire", response: { type: "declined" } },
      },
    ]);

    // A was never answered and is still pending: once the runtime settles B,
    // focus falls back to A, which remains answerable.
    session.publishState({ ...state, pendingInteractions: [approval] });
    await waitForApplicationContinuation();
    process.stdin.emit("data", "\r");
    await waitForApplicationContinuation();
    assert.deepEqual(responses[1], {
      id: "conv-test::attempt-1-interaction-approval-a",
      response: {
        type: "approval",
        decision: { type: "deny", reason: "denied by the user" },
      },
    });

    await app.quit();
    await running;
  });

  it("a resync replaces the stale surface and never answers the old interaction", async () => {
    const questionnaire = questionnaireInteraction("attempt-1-interaction-question-stale");
    const approval = approvalInteraction("attempt-1-interaction-approval-9");
    const state = {
      ...emptyPresentationState(sessionModel("alpha/model-a")),
      attempt: { ...attemptView(), phase: { type: "running" as const } },
      pendingInteractions: [questionnaire],
    };
    const session = fakeSession(async () => {}, state) as RuntimeClientAttachment & {
      publishState(nextState: unknown): void;
      publishSnapshot(): void;
    };
    const responses: Array<{ id: string; response: unknown }> = [];
    (session as unknown as {
      respondInteraction: (
        interaction: { conversation_id: string; interaction_id: string },
        response: unknown,
      ) => Promise<void>;
    }).respondInteraction = async (interaction, response) => {
      responses.push({
        id: `${interaction.conversation_id}::${interaction.interaction_id}`,
        response,
      });
    };

    const app = new RustxTuiApp({
      session,
      connection: fakeConnection(),
      child: fakeChild([]),
    });
    const running = app.run();
    await waitForApplicationContinuation();

    // The authoritative resync replaces the projection: the questionnaire is
    // gone, an approval is pending instead. The stale surface is closed, so
    // the following Enter can only target the new authoritative interaction.
    session.publishSnapshot();
    session.publishState({ ...state, pendingInteractions: [approval] });
    await waitForApplicationContinuation();
    process.stdin.emit("data", "\r");
    await waitForApplicationContinuation();
    assert.deepEqual(responses, [
      {
        id: "conv-test::attempt-1-interaction-approval-9",
        response: {
          type: "approval",
          decision: { type: "deny", reason: "denied by the user" },
        },
      },
    ]);

    await app.quit();
    await running;
  });

  it("child death removes only its own interaction from the surface", async () => {
    const childApproval = childApprovalInteraction("child-a-interaction-1", "implement");
    const approval = approvalInteraction("attempt-1-interaction-approval-a");
    const state = {
      ...emptyPresentationState(sessionModel("alpha/model-a")),
      attempt: { ...attemptView(), phase: { type: "running" as const } },
      pendingInteractions: [childApproval, approval],
    };
    const session = fakeSession(async () => {}, state) as RuntimeClientAttachment & {
      publishState(nextState: unknown): void;
    };
    const responses: Array<{ id: string; response: unknown }> = [];
    (session as unknown as {
      respondInteraction: (
        interaction: { conversation_id: string; interaction_id: string },
        response: unknown,
      ) => Promise<void>;
    }).respondInteraction = async (interaction, response) => {
      responses.push({
        id: `${interaction.conversation_id}::${interaction.interaction_id}`,
        response,
      });
    };

    const app = new RustxTuiApp({
      session,
      connection: fakeConnection(),
      child: fakeChild([]),
    });
    const running = app.run();
    await waitForApplicationContinuation();

    // The child died: the runtime removes its interaction only. The primary
    // approval survives, takes the focus, and remains answerable.
    session.publishState({ ...state, pendingInteractions: [approval] });
    await waitForApplicationContinuation();
    process.stdin.emit("data", "\r");
    await waitForApplicationContinuation();
    assert.deepEqual(responses, [
      {
        id: "conv-test::attempt-1-interaction-approval-a",
        response: {
          type: "approval",
          decision: { type: "deny", reason: "denied by the user" },
        },
      },
    ]);

    await app.quit();
    await running;
  });

  it("closes stale inspection focus when an authoritative snapshot replaces the attachment", async () => {
    let cancelled = 0;
    let snapshotListener!: () => void;
    const state = {
      ...emptyPresentationState(sessionModel("alpha/model-a")),
      attempt: {
        ...attemptView(),
        phase: { type: "running" as const },
      },
    };
    const session = fakeSession(async () => {}, state);
    const api = session as unknown as {
      onSnapshot: (listener: () => void) => () => void;
      cancelCurrentAttempt: () => Promise<string>;
    };
    api.onSnapshot = (listener) => {
      snapshotListener = listener;
      return () => {};
    };
    api.cancelCurrentAttempt = async () => {
      cancelled += 1;
      return "a1";
    };

    const app = new RustxTuiApp({
      session,
      connection: fakeConnection(),
      child: fakeChild([]),
    });
    const running = app.run();

    process.stdin.emit("data", "/help\r");
    await waitForApplicationContinuation();
    snapshotListener();

    // The snapshot callback closes the old inspection before Escape is
    // interpreted as cancellation intent.
    process.stdin.emit("data", "\u001b");
    await waitForPiEscapeDisambiguation();
    assert.equal(cancelled, 1);

    await app.quit();
    await running;
  });

  it("drops an inspection result that completes after a new attachment is bound", async () => {
    let oldInspectionStarted!: () => void;
    const oldInspectionObserved = new Promise<void>((resolve) => {
      oldInspectionStarted = resolve;
    });
    const oldInspection = deferred<ReturnType<typeof sessionView>>();
    let refreshCalls = 0;
    let nextBound!: () => void;
    const nextBoundObserved = new Promise<void>((resolve) => {
      nextBound = resolve;
    });
    let oldCancelled = 0;
    let nextCancelled = 0;
    const runningState = {
      ...emptyPresentationState(sessionModel("alpha/model-a")),
      attempt: {
        ...attemptView(),
        phase: { type: "running" as const },
      },
    };
    const oldSession = fakeSession(async () => {}, runningState);
    const oldApi = oldSession as unknown as {
      refreshSession: () => Promise<ReturnType<typeof sessionView>>;
      newSession: () => Promise<unknown>;
      detach: () => Promise<void>;
      cancelCurrentAttempt: () => Promise<string>;
    };
    oldApi.refreshSession = async () => {
      refreshCalls += 1;
      if (refreshCalls === 1) return sessionView({ id: "session-a", name: "A" });
      oldInspectionStarted();
      return oldInspection.promise;
    };
    oldApi.newSession = async () => ({
      session: sessionView({ id: "session-b", name: "B" }),
      restartRequired: true,
    });
    oldApi.detach = async () => {};
    oldApi.cancelCurrentAttempt = async () => {
      oldCancelled += 1;
      return "old-attempt";
    };

    const nextSession = fakeSession(async () => {}, runningState);
    const nextApi = nextSession as unknown as {
      refreshSession: () => Promise<ReturnType<typeof sessionView>>;
      cancelCurrentAttempt: () => Promise<string>;
    };
    nextApi.refreshSession = async () => {
      nextBound();
      return sessionView({ id: "session-b", name: "B" });
    };
    nextApi.cancelCurrentAttempt = async () => {
      nextCancelled += 1;
      return "next-attempt";
    };

    const app = new RustxTuiApp({
      session: oldSession,
      connection: fakeConnection(),
      child: fakeChild([]),
      restartRuntime: async () => ({
        session: nextSession,
        connection: fakeConnection(),
        child: fakeChild([]),
      }),
    });
    const running = app.run();
    await waitForApplicationContinuation();

    process.stdin.emit("data", "/session\r");
    await oldInspectionObserved;
    process.stdin.emit("data", "/new\r");
    await nextBoundObserved;
    await waitForApplicationContinuation();

    // The old request really completes after B is current. Its inspection
    // result must not acquire B's overlay or steal its editor focus.
    oldInspection.resolve(sessionView({ id: "session-a", name: "stale A" }));
    await waitForApplicationContinuation();
    process.stdin.emit("data", "\u001b");
    await waitForPiEscapeDisambiguation();

    assert.equal(oldCancelled, 0);
    assert.equal(nextCancelled, 1, "Escape must reach the current attachment");

    await app.quit();
    await running;
  });

  it("does not let a late old-attachment acknowledgement replace B's transient", async () => {
    let oldRenameStarted!: () => void;
    const oldRenameObserved = new Promise<void>((resolve) => {
      oldRenameStarted = resolve;
    });
    const oldRename = deferred<ReturnType<typeof sessionView>>();
    let nextBound!: () => void;
    const nextBoundObserved = new Promise<void>((resolve) => {
      nextBound = resolve;
    });
    const renders = countTuiRenderRequests();
    const runningState = {
      ...emptyPresentationState(sessionModel("alpha/model-a")),
      attempt: {
        ...attemptView(),
        phase: { type: "running" as const },
      },
    };
    const oldSession = fakeSession(async () => {}, runningState);
    const oldApi = oldSession as unknown as {
      refreshSession: () => Promise<ReturnType<typeof sessionView>>;
      newSession: () => Promise<unknown>;
      detach: () => Promise<void>;
      nameSession: () => Promise<ReturnType<typeof sessionView>>;
    };
    oldApi.refreshSession = async () => sessionView({ id: "session-a", name: "A" });
    oldApi.nameSession = async () => {
      oldRenameStarted();
      return oldRename.promise;
    };
    oldApi.newSession = async () => ({
      session: sessionView({ id: "session-b", name: "B" }),
      restartRequired: true,
    });
    oldApi.detach = async () => {};

    const nextSession = fakeSession(async () => {}, runningState);
    const nextApi = nextSession as unknown as {
      refreshSession: () => Promise<ReturnType<typeof sessionView>>;
      nameSession: () => Promise<ReturnType<typeof sessionView>>;
    };
    nextApi.refreshSession = async () => {
      nextBound();
      return sessionView({ id: "session-b", name: "B" });
    };
    nextApi.nameSession = async () => sessionView({ id: "session-b", name: "current B" });

    try {
      const app = new RustxTuiApp({
        session: oldSession,
        connection: fakeConnection(),
        child: fakeChild([]),
        restartRuntime: async () => ({
          session: nextSession,
          connection: fakeConnection(),
          child: fakeChild([]),
        }),
      });
      const running = app.run();
      await waitForApplicationContinuation();

      process.stdin.emit("data", "/name stale A\r");
      await oldRenameObserved;
      process.stdin.emit("data", "/new\r");
      await nextBoundObserved;
      await waitForApplicationContinuation();

      // Establish a current B-owned feedback item before releasing A's old
      // operation. A stale completion must not replace it or request a redraw.
      process.stdin.emit("data", "/name current B\r");
      await waitForApplicationContinuation();
      renders.start();
      oldRename.resolve(sessionView({ id: "session-a", name: "stale A" }));
      await waitForApplicationContinuation();
      assert.equal(renders.count(), 0, "late A feedback must not touch B's surface");

      await app.quit();
      await running;
    } finally {
      renders.restore();
    }
  });

  it("closes a stale picker and rejects its late page callback on resync", async () => {
    let snapshotListener!: () => void;
    let firstPage!: () => void;
    const firstPageObserved = new Promise<void>((resolve) => {
      firstPage = resolve;
    });
    const latePage = deferred<{
      sessions: SessionSummaryView[];
      nextOffset?: number;
    }>();
    let listCalls = 0;
    let cancelled = 0;
    const renders = countTuiRenderRequests();
    const session = fakeSession(async () => {}, {
      ...emptyPresentationState(sessionModel("alpha/model-a")),
      attempt: {
        ...attemptView(),
        phase: { type: "running" as const },
      },
    });
    const api = session as unknown as {
      onSnapshot: (listener: () => void) => () => void;
      refreshSession: () => Promise<ReturnType<typeof sessionView>>;
      listSessions: () => Promise<{
        sessions: SessionSummaryView[];
        nextOffset?: number;
      }>;
      cancelCurrentAttempt: () => Promise<string>;
    };
    api.onSnapshot = (listener) => {
      snapshotListener = listener;
      return () => {};
    };
    api.refreshSession = async () => sessionView();
    api.listSessions = async () => {
      listCalls += 1;
      if (listCalls === 1) {
        firstPage();
        return {
          sessions: [{
            id: "session-a",
            name: "A",
            updated_at: "2026-08-21T00:00:00Z",
            active_node: "node-a",
            active: true,
          }],
          nextOffset: 1,
        };
      }
      return latePage.promise;
    };
    api.cancelCurrentAttempt = async () => {
      cancelled += 1;
      return "attempt-a";
    };

    try {
      const app = new RustxTuiApp({
        session,
        connection: fakeConnection(),
        child: fakeChild([]),
      });
      const running = app.run();
      process.stdin.emit("data", "/resume\r");
      await firstPageObserved;
      await waitForApplicationContinuation();

      // The only row is selected, so Down starts the deferred continuation.
      process.stdin.emit("data", "\u001b[B");
      await waitForApplicationContinuation();
      snapshotListener();
      await waitForApplicationContinuation();

      // Snapshot replacement closes every overlay, including non-inspection
      // pickers. The late page error must not repaint the new presentation.
      renders.start();
      latePage.reject(new Error("stale page failure"));
      await waitForApplicationContinuation();
      assert.equal(renders.count(), 0);

      process.stdin.emit("data", "\u001b");
      await waitForPiEscapeDisambiguation();
      assert.equal(cancelled, 1, "the closed picker must restore cancellation precedence");

      await app.quit();
      await running;
    } finally {
      renders.restore();
    }
  });

  it("replaces a terminal Session attachment from the authoritative restart", async () => {
    const log: string[] = [];
    let refreshed!: () => void;
    const refreshedObserved = new Promise<void>((resolve) => {
      refreshed = resolve;
    });
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
      refreshed();
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
    await refreshedObserved;

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
    // the next input event is delivered.
    await Promise.resolve();

    process.stdin.emit("data", "\r");
    assert.equal(await submittedObserved, prompt);
    assert.deepEqual(log.slice(0, 3), ["detach", "close_stdin", "wait_exit"]);

    await app.quit();
    await running;
  });

  it("routes interactive model replacement through the terminal Session flow", async () => {
    const log: string[] = [];
    let catalogRead!: () => void;
    const catalogReadObserved = new Promise<void>((resolve) => {
      catalogRead = resolve;
    });
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
    oldApi.modelCatalog = async () => {
      catalogRead();
      return {
        models: [
          catalogModel("alpha/model-a"),
          catalogModel("beta/model-b"),
        ],
      };
    };
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
    await catalogReadObserved;
    // The catalog response is observed above; let the dispatcher finish its
    // promise chain and install the selector before its next input event.
    await waitForApplicationContinuation();
    process.stdin.emit("data", "\u001b[B");
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

  it("opens the same dispatcher-backed model selector from Ctrl+L", async () => {
    let catalogReads = 0;
    let selectedModel: string | undefined;
    let catalogRead!: () => void;
    const catalogReadObserved = new Promise<void>((resolve) => {
      catalogRead = resolve;
    });
    const session = fakeSession(
      async () => {},
      emptyPresentationState(sessionModel("alpha/model-a")),
    );
    const api = session as unknown as {
      modelCatalog: () => Promise<{ models: ReturnType<typeof catalogModel>[] }>;
      modelSet: (config: { model: string }) => Promise<ReturnType<typeof sessionModel>>;
    };
    api.modelCatalog = async () => {
      catalogReads += 1;
      catalogRead();
      return {
        models: [catalogModel("alpha/model-a"), catalogModel("beta/model-b")],
      };
    };
    api.modelSet = async (config) => {
      selectedModel = config.model;
      return sessionModel(config.model);
    };

    const app = new RustxTuiApp({
      session,
      connection: fakeConnection(),
      child: fakeChild([]),
    });
    const running = app.run();

    process.stdin.emit("data", "\f");
    await catalogReadObserved;
    await waitForApplicationContinuation();
    // The focused overlay owns Ctrl+L; it must not open a second selector.
    process.stdin.emit("data", "\f");
    process.stdin.emit("data", "\u001b[B");
    process.stdin.emit("data", "\r");
    await waitForApplicationContinuation();

    assert.equal(catalogReads, 1);
    assert.equal(selectedModel, "beta/model-b");

    await app.quit();
    await running;
  });
});
