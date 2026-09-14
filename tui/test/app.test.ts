/**
 * The terminal application's lifecycle and input routing.
 *
 * The App Server pieces are controlled at their own boundaries here: this
 * suite proves what the UI does — which overlay owns a key, which projection a
 * late continuation may touch, what exiting means — while the real-child
 * integration suite exercises the process and transport boundary itself.
 *
 * The two process-ownership modes are represented honestly: an owned host
 * reports a child and ends it on exit; an external host has no child and only
 * ever disconnects.
 */

import assert from "node:assert/strict";
import { describe, it, type TestContext } from "node:test";
import { TUI, Editor } from "@earendil-works/pi-tui";

import { plainText, plainWidth } from "../src/ui/theme.ts";
import { stateOf } from "./support/render.ts";
import { RustxTuiApp } from "../src/ui/app.ts";
import {
  AppServerRequestError,
  UncertainOutcomeError,
} from "../src/app-server/client.ts";
import { TransportClosedError } from "../src/app-server/transport.ts";
import { emptyPresentationState } from "../src/presentation/projection.ts";
import { TransientFeedbackSurface } from "../src/ui/components/transient-feedback.ts";
import type { AppServerHost } from "../src/app-server/host.ts";
import type { AppServerSession } from "../src/app-server/session.ts";
import type {
  AttachmentTarget,
  SessionDeleteResult,
  SessionSnapshot,
  SessionSummaryView,
} from "../src/protocol/app-server.ts";
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

const SESSION_SETTINGS = { cwd: "/work/project" };

function targetFor(sessionId: string): AttachmentTarget {
  return {
    session_id: sessionId,
    conversation_id: `conv-${sessionId}`,
    runtime_incarnation: "1",
    attachment_id: `att-${sessionId}`,
  };
}

/**
 * One attached Session, controlled by the test.
 *
 * Only the surface the app actually uses is present: a projection, the two
 * publication signals, and the operations a command can reach.
 */
function fakeSession(
  state: unknown = {
    attempt: { attemptId: "attempt-1", phase: { type: "running" as const } },
  },
  sessionId = "session-1",
): AppServerSession {
  // Even lifecycle-focused tests expose a complete native presentation
  // snapshot: the footer reads it at render time, including terminal resizes.
  if (state && typeof state === "object" && !("sessionModel" in state)) {
    state = { ...emptyPresentationState(sessionModel("alpha/model-a")), ...state };
  }
  const stateListeners = new Set<(nextState: unknown) => void>();
  const snapshotListeners = new Set<() => void>();
  const closedListeners = new Set<() => void>();
  const session = {
    state,
    sessionId,
    target: targetFor(sessionId),
    released: false,
    serverClosed: false,
    resyncCount: 0,
    onState: (listener: (nextState: unknown) => void) => {
      stateListeners.add(listener);
      return () => stateListeners.delete(listener);
    },
    onSnapshot: (listener: () => void) => {
      snapshotListeners.add(listener);
      return () => snapshotListeners.delete(listener);
    },
    onClosed: (listener: () => void) => {
      closedListeners.add(listener);
      return () => closedListeners.delete(listener);
    },
    updateState: () => {},
    resync: async () => {},
    detach: async () => {},
    publishState(nextState: unknown): void {
      session.state = nextState;
      for (const listener of stateListeners) listener(nextState);
    },
    publishSnapshot(): void {
      for (const listener of snapshotListeners) listener();
    },
    publishClosed(): void {
      for (const listener of closedListeners) listener();
    },
  };
  return session as unknown as AppServerSession;
}

interface FakeHostOptions {
  ownership?: "owned_child" | "external";
  /** Durable Session catalog overrides: list, preview, delete, recover. */
  catalog?: Record<string, unknown>;
  /** Records the owned child's shutdown steps, in order. */
  log?: string[];
  /** Installed so a test can end the connection on demand. */
  onClose?: (listener: (error: TransportClosedError) => void) => void;
  /** Answers `host.attach` for a focus change. */
  attach?: (sessionId: string, nodeId?: string) => Promise<AppServerSession>;
  /** Answers `host.readSession` for the footer. */
  readSession?: (sessionId: string) => Promise<SessionSnapshot>;
  exitCode?: number;
}

function fakeHost(options: FakeHostOptions = {}): AppServerHost {
  const ownership = options.ownership ?? "owned_child";
  const log = options.log ?? [];
  const host = {
    ownership,
    client: {
      closed: undefined,
      pendingCount: 0,
      close: () => {},
      onClose: (listener: (error: TransportClosedError) => void) => {
        options.onClose?.(listener);
        return () => {};
      },
      onNotification: () => () => {},
      describeTransport: () => "fake",
    },
    attached: [],
    describe: () => (ownership === "owned_child" ? "owned child" : "external"),
    stderrTail: () => ({ text: "", truncatedBytes: 0 }),
    childExit: undefined,
    attach:
      options.attach ??
      (async (sessionId: string) => fakeSession(undefined, sessionId)),
    attachment: () => undefined,
    detach: async () => {},
    readSession:
      options.readSession ?? (async () => sessionView()),
    listSessions: async () => ({ sessions: [] as SessionSummaryView[] }),
    createSession: async () => ({ session: sessionView() }),
    renameSession: async () => sessionView(),
    sessionTree: async () => ({ nodes: [] }),
    forkSession: async () => ({ session: sessionView() }),
    branchSession: async () => ({ session: sessionView() }),
    previewSessionDeletion: async () => ({}) as SessionDeleteResult,
    deleteSession: async () => ({}) as SessionDeleteResult,
    recoverSessionDeletion: async () => ({}) as SessionDeleteResult,
    ...(options.catalog ?? {}),
    shutdown: async () => {
      if (ownership === "owned_child") {
        // The owned child's stdin is closed, it sees EOF, and the owner waits.
        log.push("close_stdin");
        log.push("wait_exit");
        return { code: options.exitCode ?? 0, signal: null };
      }
      // An external server keeps running. This is a disconnect and no more.
      log.push("disconnect");
      return undefined;
    },
  };
  return host as unknown as AppServerHost;
}

/** Builds an app over a controlled host and Session. */
function appOver(
  session: AppServerSession,
  host: AppServerHost = fakeHost(),
): RustxTuiApp {
  return new RustxTuiApp({
    host,
    session,
    sessionSettings: SESSION_SETTINGS,
    cwd: "/work/project",
  });
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
  // Let the promise chain that handles one observed App Server response finish
  // before the next synthetic input event. This is an event-loop continuation,
  // not an elapsed-time synchronization primitive.
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
  it("inspects a selected subagent from authoritative status, without a second client", async () => {
    const state = {
      ...emptyPresentationState(sessionModel("alpha/model-a")),
      subagents: [subagent("explore", "sha256:child")],
    };
    const session = fakeSession(state);
    let statusReads = 0;
    (session as unknown as {
      subagentStatus: (id: string) => Promise<unknown>;
    }).subagentStatus = async (id) => {
      statusReads += 1;
      assert.equal(id, "conv-1-subagent-1");
      return subagent("explore", "sha256:child");
    };
    const app = appOver(session as unknown as AppServerSession);
    const running = app.run();

    // Ctrl+Down enters the explicit subagent-list focus; plain Enter remains
    // reserved for the editor unless that focus is active.
    process.stdin.emit("data", "\x1b[1;5B");
    await waitForApplicationContinuation();
    process.stdin.emit("data", "\r");
    await waitForApplicationContinuation();

    // The child is read through `subagent/status`. No second conversation
    // client is composed, and no process is spawned to look at a child.
    assert.equal(statusReads, 1);
    assert.equal(session.state, state, "inspection does not replace state");

    process.stdin.emit("data", "\u001b");
    await waitForPiEscapeDisambiguation();
    await app.quit();
    await running;
  });

  it("ends an owned App Server child on normal exit", async () => {
    const log: string[] = [];
    const app = appOver(
      fakeSession(),
      fakeHost({ ownership: "owned_child", log }),
    );

    await app.quit();

    // The child this TUI started is ended through the process-lifecycle seam.
    // Work in flight may be lost because that process is deliberately ending,
    // not because a transport detach is execution authority.
    assert.deepEqual(log, ["close_stdin", "wait_exit"]);
  });

  it("never waits for a runtime settlement it cannot observe", async () => {
    const log: string[] = [];
    const session = fakeSession({
      ...emptyPresentationState(sessionModel("alpha/model-a")),
      attempt: { attemptId: "attempt-1", phase: { type: "running" as const } },
    });
    const app = appOver(session as unknown as AppServerSession, fakeHost({ ownership: "owned_child", log }));

    await app.quit();

    // Exiting does not ask the runtime to settle, and does not report that it
    // did. An attempt that was running is simply no longer observable.
    assert.deepEqual(log, ["close_stdin", "wait_exit"]);
    assert.equal(
      (session.state as { attempt?: { phase: { type: string } } }).attempt?.phase
        .type,
      "running",
      "nothing fabricated a settlement",
    );
  });

  it("only disconnects from an external App Server on exit", async () => {
    const log: string[] = [];
    const app = appOver(
      fakeSession(),
      fakeHost({ ownership: "external", log }),
    );

    assert.equal(await app.quit(), undefined);

    // No child, no stdin close, no termination: the server someone else runs
    // keeps running, and so does every Session on it.
    assert.deepEqual(log, ["disconnect"]);
  });

  it("settles run immediately when the connection was already terminal", async () => {
    const app = appOver(
      fakeSession(),
      fakeHost({
        onClose: (listener) =>
          listener(
            new TransportClosedError(
              "process_exit",
              "the process was already gone",
            ),
          ),
      }),
    );

    assert.equal(await app.run(), 1);
  });

  it("commits a fatal diagnostic before stopping the TUI", async () => {
    const events: string[] = [];
    let close!: (error: TransportClosedError) => void;
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
      const app = appOver(
        fakeSession(emptyPresentationState(sessionModel("alpha/model-a"))),
        fakeHost({
          onClose: (listener) => {
            close = listener;
          },
        }),
      );
      const running = app.run();
      await waitForApplicationContinuation();
      events.length = 0;

      close(new TransportClosedError("process_exit", "fatal transport diagnostic"));
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

  it("reports a failed focus change without replacing anything", async () => {
    const log: string[] = [];
    const session = fakeSession(emptyPresentationState(sessionModel("alpha/model-a")));
    const app = appOver(
      session,
      fakeHost({
        log,
        catalog: {
          createSession: async () => ({ session: sessionView({ id: "session-2" }) }),
        },
        attach: async () => {
          throw new AppServerRequestError("session/attach", {
            code: -32000,
            message: "another client already controls this Session",
            data: { kind: "controller_in_use" },
          });
        },
      }),
    );

    const running = app.run();
    process.stdin.emit("data", "/new\r");
    await waitForApplicationContinuation();
    await waitForApplicationContinuation();

    // A Session that cannot be opened is reported. It is not a reason to end
    // the process, replace the child, or stop the Session already on screen.
    assert.deepEqual(log, [], "no process lifecycle action was taken");
    assert.equal(session.state, session.state);

    await app.quit();
    assert.equal(await running, 0);
    assert.deepEqual(log, ["close_stdin", "wait_exit"]);
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
    const session = fakeSession(runningState);
    (session as unknown as { cancelCurrentAttempt: () => Promise<string> })
      .cancelCurrentAttempt = async () => {
        cancelled += 1;
        return "a1";
      };
    let sessionsListed!: () => void;
    const sessionsListedObserved = new Promise<void>((resolve) => {
      sessionsListed = resolve;
    });

    const app = appOver(
      session as unknown as AppServerSession,
      fakeHost({
        catalog: {
          listSessions: async () => {
            sessionsListed();
            return {
              sessions: [{
                id: "session-1",
                name: "current",
                updated_at: "2026-08-21T00:00:00Z",
                active_node: "node-1",
              }],
            };
          },
        },
      }),
    );
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

  it("Session deletion overlay preserves editor, attachment and active projection and owns Esc", async () => {
    const state = emptyPresentationState(sessionModel("alpha/model-a"));
    const session = fakeSession(state);
    const log: string[] = [];
    let previews = 0, executes = 0, cancelled = 0;
    let rows = [{ id: "old", name: "history", updated_at: "today", active_node: "node-2" }];
    (session as unknown as { cancelCurrentAttempt: () => Promise<string> }).cancelCurrentAttempt =
      async () => { cancelled++; return "attempt"; };
    const app = appOver(session as unknown as AppServerSession, fakeHost({ log, catalog: {
      listSessions: async () => ({ sessions: rows }),
      previewSessionDeletion: async () => { previews++; return { status: "preview", preview: { session_id: "old", name: "history", target_revision: "revision", owned_node_count: 1, owned_conversation_count: 1, owned_child_count: 0 } }; },
      deleteSession: async (id: string, revision: string) => { assert.equal(id, "old"); assert.equal(revision, "revision"); executes++; rows = []; return { status: "deleted", session_id: id }; },
    } }));
    const originalSetText = Editor.prototype.setText;
    let editorWrites = 0;
    Editor.prototype.setText = function(text: string): void { editorWrites++; originalSetText.call(this, text); };
    const running = app.run();
    try {
      process.stdin.emit("data", "/resume\r"); await waitForApplicationContinuation();
      const writesBeforeDeletion = editorWrites;
      process.stdin.emit("data", "\x04"); await waitForApplicationContinuation();
      process.stdin.emit("data", "\x1b"); await waitForPiEscapeDisambiguation();
      assert.equal(cancelled, 0); assert.equal(executes, 0);
      process.stdin.emit("data", "\x04"); await waitForApplicationContinuation();
      process.stdin.emit("data", "\t\r"); await waitForApplicationContinuation();
      assert.equal(previews, 2); assert.equal(executes, 1);
      // Deleting some other Session neither replaces the visible projection
      // nor touches the owned process.
      assert.equal(session.state, state); assert.deepEqual(log, []);
      assert.equal(editorWrites, writesBeforeDeletion, "deletion never resets the editor");
    } finally { Editor.prototype.setText = originalSetText; await app.quit(); await running; }
  });

  it("pending Session execute and recovery own Esc without cancelling the active attempt", async () => {
    const state = {
      ...emptyPresentationState(sessionModel("alpha/model-a")),
      attempt: { ...attemptView(), phase: { type: "running" as const } },
    };
    const session = fakeSession(state);
    const execution = deferred<SessionDeleteResult>();
    const recovery = deferred<SessionDeleteResult>();
    let executes = 0, recovers = 0, cancelled = 0, lists = 0;
    (session as unknown as { cancelCurrentAttempt: () => Promise<string> }).cancelCurrentAttempt =
      async () => { cancelled++; return "attempt"; };
    const app = appOver(session as unknown as AppServerSession, fakeHost({ catalog: {
      listSessions: async () => {
        lists++;
        return { sessions: lists === 1 ? [{ id: "old", name: "history", updated_at: "today", active_node: "node-2" }] : [] };
      },
      previewSessionDeletion: async () => ({ status: "preview", preview: { session_id: "old", name: "history", target_revision: "revision", owned_node_count: 1, owned_conversation_count: 1, owned_child_count: 0 } }),
      deleteSession: () => { executes++; return execution.promise; },
      recoverSessionDeletion: () => { recovers++; return recovery.promise; },
    } }));
    const running = app.run();
    try {
      process.stdin.emit("data", "/resume\r"); await waitForApplicationContinuation();
      process.stdin.emit("data", "\x04"); await waitForApplicationContinuation();
      process.stdin.emit("data", "\t\r"); await waitForApplicationContinuation();
      assert.equal(executes, 1);
      // Kitty's complete Escape sequence avoids the bare-ESC disambiguation timer.
      process.stdin.emit("data", "\x1b[27u\x1b[27u"); await waitForApplicationContinuation();
      assert.equal(cancelled, 0);
      execution.resolve({ status: "committed_cleanup_pending", session_id: "old" });
      await waitForApplicationContinuation();
      process.stdin.emit("data", "r"); await waitForApplicationContinuation();
      assert.equal(recovers, 1);
      recovery.resolve({ status: "deleted", session_id: "old" });
      await waitForApplicationContinuation();
      assert.equal(cancelled, 0, "a deletion overlay never cancels the attempt");
    } finally { await app.quit(); await running; }
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
    const session = fakeSession(state);
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

    const app = appOver(session as unknown as AppServerSession);
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
              answer: { type: "option", value: { option_index: 0 } },
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
    const session = fakeSession(state);
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

    const app = appOver(session as unknown as AppServerSession);
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
    const session = fakeSession(state) as unknown as Record<string, unknown> & {
      publishState(nextState: unknown): void;
    };
    let responses = 0;
    (session as unknown as {
      respondInteraction: () => Promise<void>;
    }).respondInteraction = async () => {
      responses += 1;
    };

    const app = appOver(session as unknown as AppServerSession);
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
    const session = fakeSession(state);
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

    const app = appOver(session as unknown as AppServerSession);
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
    const session = fakeSession(state);
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

    const app = appOver(session as unknown as AppServerSession);
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
    const session = fakeSession(state);
    const responses: unknown[] = [];
    (session as unknown as {
      respondInteraction: (interaction: unknown, response: unknown) => Promise<void>;
    }).respondInteraction = async (_interaction, response) => {
      responses.push(response);
    };

    const app = appOver(session as unknown as AppServerSession);
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
    const session = fakeSession(state) as unknown as Record<string, unknown> & {
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

    const app = appOver(session as unknown as AppServerSession);
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
    const session = fakeSession(state);
    const responses: unknown[] = [];
    (session as unknown as {
      respondInteraction: (interaction: unknown, response: unknown) => Promise<void>;
    }).respondInteraction = async (_interaction, response) => {
      responses.push(response);
    };

    const app = appOver(session as unknown as AppServerSession);
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
    const session = fakeSession(state) as unknown as Record<string, unknown> & {
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

    const app = appOver(session as unknown as AppServerSession);
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
    const session = fakeSession(state) as unknown as Record<string, unknown> & {
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

    const app = appOver(session as unknown as AppServerSession);
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
    const session = fakeSession(state) as unknown as Record<string, unknown> & {
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

    const app = appOver(session as unknown as AppServerSession);
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
    const session = fakeSession(state);
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

    const app = appOver(session as unknown as AppServerSession);
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

  it("drops an inspection result that completes after focus moved to another Session", async () => {
    let oldInspectionStarted!: () => void;
    const oldInspectionObserved = new Promise<void>((resolve) => {
      oldInspectionStarted = resolve;
    });
    const oldInspection = deferred<ReturnType<typeof sessionView>>();
    let nextBound!: () => void;
    const nextBoundObserved = new Promise<void>((resolve) => {
      nextBound = resolve;
    });
    let refreshCalls = 0;
    let oldCancelled = 0;
    let nextCancelled = 0;
    const runningState = {
      ...emptyPresentationState(sessionModel("alpha/model-a")),
      attempt: { ...attemptView(), phase: { type: "running" as const } },
    };
    const oldSession = fakeSession(runningState, "session-a");
    (oldSession as unknown as { cancelCurrentAttempt: () => Promise<string> })
      .cancelCurrentAttempt = async () => {
        oldCancelled += 1;
        return "old-attempt";
      };
    const nextSession = fakeSession(runningState, "session-b");
    (nextSession as unknown as { cancelCurrentAttempt: () => Promise<string> })
      .cancelCurrentAttempt = async () => {
        nextCancelled += 1;
        return "next-attempt";
      };

    const app = appOver(
      oldSession,
      fakeHost({
        attach: async () => {
          nextBound();
          return nextSession;
        },
        readSession: async () => {
          refreshCalls += 1;
          // 1: the footer's read when A is bound.
          // 2: the `/session` command, held open on purpose.
          // 3+: the footer's read once B is bound.
          if (refreshCalls === 1) return sessionView({ id: "session-a", name: "A" });
          if (refreshCalls === 2) {
            oldInspectionStarted();
            return oldInspection.promise;
          }
          return sessionView({ id: "session-b", name: "B" });
        },
        catalog: {
          createSession: async () => ({
            session: sessionView({ id: "session-b", name: "B" }),
          }),
        },
      }),
    );
    const running = app.run();
    await waitForApplicationContinuation();

    process.stdin.emit("data", "/session\r");
    await oldInspectionObserved;
    process.stdin.emit("data", "/new\r");
    await nextBoundObserved;
    await waitForApplicationContinuation();

    // The old request really completes after B is visible. Its inspection
    // result must not acquire B's overlay or steal its editor focus.
    oldInspection.resolve(sessionView({ id: "session-a", name: "stale A" }));
    await waitForApplicationContinuation();
    process.stdin.emit("data", "\u001b");
    await waitForPiEscapeDisambiguation();

    // A is still attached and still running; Escape reached B, not A.
    assert.equal(oldCancelled, 0);
    assert.equal(nextCancelled, 1, "Escape must reach the visible Session");

    await app.quit();
    await running;
  });

  it("does not let a late continuation from the previous focus replace B's transient", async () => {
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
      attempt: { ...attemptView(), phase: { type: "running" as const } },
    };
    const oldSession = fakeSession(runningState, "session-a");
    const nextSession = fakeSession(runningState, "session-b");
    let renames = 0;

    try {
      const app = appOver(
        oldSession,
        fakeHost({
          attach: async () => {
            nextBound();
            return nextSession;
          },
          catalog: {
            createSession: async () => ({
              session: sessionView({ id: "session-b", name: "B" }),
            }),
            renameSession: async () => {
              renames += 1;
              if (renames === 1) {
                oldRenameStarted();
                return oldRename.promise;
              }
              return sessionView({ id: "session-b", name: "current B" });
            },
          },
        }),
      );
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
    const session = fakeSession({
      ...emptyPresentationState(sessionModel("alpha/model-a")),
      attempt: {
        ...attemptView(),
        phase: { type: "running" as const },
      },
    });
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
      return "attempt-a";
    };

    try {
      const app = appOver(
        session as unknown as AppServerSession,
        fakeHost({
          catalog: {
            listSessions: async () => {
              listCalls += 1;
              if (listCalls === 1) {
                firstPage();
                return {
                  sessions: [{
                    id: "session-a",
                    name: "A",
                    updated_at: "2026-08-21T00:00:00Z",
                    active_node: "node-a",
                  }],
                  nextOffset: 1,
                };
              }
              return latePage.promise;
            },
          },
        }),
      );
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

  it("shows the created Session without replacing a process", async () => {
    const log: string[] = [];
    let attached!: () => void;
    const attachObserved = new Promise<void>((resolve) => {
      attached = resolve;
    });
    const oldSession = fakeSession(
      emptyPresentationState(sessionModel("alpha/model-a")),
      "session-1",
    );
    const nextSession = fakeSession(
      emptyPresentationState(sessionModel("alpha/model-a")),
      "session-2",
    );

    const app = appOver(
      oldSession,
      fakeHost({
        log,
        attach: async (sessionId) => {
          assert.equal(sessionId, "session-2");
          log.push("attach");
          attached();
          return nextSession;
        },
        catalog: {
          createSession: async () => {
            log.push("create");
            return { session: sessionView({ id: "session-2", name: "B" }) };
          },
        },
      }),
    );
    const running = app.run();
    process.stdin.emit("data", "/new\r");
    await attachObserved;
    await waitForApplicationContinuation();

    // Creating and showing a Session is two App Server operations and no
    // process lifecycle at all: the child is untouched, and the Session that
    // was visible before is neither detached nor unloaded.
    assert.deepEqual(log, ["create", "attach"]);
    assert.equal(oldSession.released, false);

    await app.quit();
    await running;
    assert.deepEqual(log.slice(-2), ["close_stdin", "wait_exit"]);
  });

  it("restores a committed fork draft into the Session it belongs to", async () => {
    const prompt = "fork-draft-exact-7f3b";
    const log: string[] = [];
    let attached!: () => void;
    const attachObserved = new Promise<void>((resolve) => {
      attached = resolve;
    });
    let submitted!: (content: string) => void;
    const submittedObserved = new Promise<string>((resolve) => {
      submitted = resolve;
    });

    const oldSession = fakeSession(
      emptyPresentationState(sessionModel("alpha/model-a")),
      "session-1",
    );
    const nextSession = fakeSession(
      emptyPresentationState(sessionModel("alpha/model-a")),
      "session-2",
    );
    (nextSession as unknown as {
      submitInbound: (
        content: Array<{ type: "text"; text: string }>,
      ) => Promise<{ messageId: string; sequence: string }>;
    }).submitInbound = async (content) => {
      submitted(content.map((block) => block.text).join("\n"));
      return { messageId: "destination-user-1", sequence: "1" };
    };

    const app = appOver(
      oldSession,
      fakeHost({
        log,
        attach: async () => {
          attached();
          return nextSession;
        },
        catalog: {
          createSession: async () => ({
            session: sessionView({ id: "session-2", name: "committed fork" }),
            editorContent: [{ type: "text", text: prompt }],
            durabilityDiagnostic: "catalog visibility committed; durability uncertain",
          }),
        },
      }),
    );
    const running = app.run();
    process.stdin.emit("data", "/new\r");
    await attachObserved;
    // Let the app's awaited focus continuation install the draft before the
    // next input event is delivered.
    await waitForApplicationContinuation();
    await waitForApplicationContinuation();

    process.stdin.emit("data", "\r");
    assert.equal(await submittedObserved, prompt);
    // The draft was restored by changing focus, not by replacing a process.
    assert.ok(!log.includes("close_stdin"));

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
    const session = fakeSession(emptyPresentationState(sessionModel("alpha/model-a")),
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

    const app = appOver(session as unknown as AppServerSession);
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

/** Real app routing with native operations held at explicit submission gates. */
async function deletionAppHarness(overlapInitial = false) {
  const state = { ...emptyPresentationState(sessionModel("alpha/model-a")), attempt: { ...attemptView(), phase: { type: "running" as const } } };
  const session = fakeSession(state) as unknown as Record<string, unknown> & {
    publishState(next: typeof state): void;
    publishSnapshot(): void;
  };
  let preview = deferred<SessionDeleteResult>();
  const previews: string[] = [];
  let execution = deferred<SessionDeleteResult>();
  const recovery = deferred<SessionDeleteResult>();
  const lateList = deferred<{ sessions: SessionSummaryView[] }>();
  const lists: Array<[string | undefined, number | undefined]> = [];
  const executes: string[][] = [], recovers: string[] = [], responses: unknown[] = [];
  let cancelled = 0;
  let rows: SessionSummaryView[] = [{ id: "old", name: "historical-target", updated_at: "today", active_node: "node-2", }];
  let listResponse: ((query?: string, offset?: number) => Promise<{ sessions: SessionSummaryView[]; nextOffset?: number }>) | undefined;
  // Deletion addresses the durable Session catalog on the host; the attached
  // Session owns only what an attachment owns.
  const catalog = {
    listSessions: async (query?: string, offset = 0) => {
      lists.push([query, offset]);
      if (overlapInitial && lists.length === 1) return lateList.promise;
      return listResponse ? listResponse(query, offset) : { sessions: [...rows] };
    },
    previewSessionDeletion: (id: string) => { previews.push(id); return preview.promise; },
    deleteSession: (id: string, revision: string) => { executes.push([id, revision]); return execution.promise; },
    recoverSessionDeletion: (id: string) => { recovers.push(id); return recovery.promise; },
  };
  session.cancelCurrentAttempt = async () => { cancelled++; return "attempt-1"; };
  session.respondInteraction = async (interaction: unknown, response: unknown) => { responses.push({ interaction, response }); };
  session.submitInbound = async () => { assert.fail("deletion must not submit model-visible input"); };
  let close!: (error: TransportClosedError) => void;
  const host = fakeHost({ catalog, onClose: (listener) => { close = listener; } });
  const original = TUI.prototype.showOverlay;
  const surfaces: Array<{ content: Parameters<TUI["showOverlay"]>[0]; visible: boolean }> = [];
  TUI.prototype.showOverlay = function(content, options) {
    const surface = { content, visible: true };
    surfaces.push(surface);
    const handle = original.call(this, content, options);
    const hide = handle.hide;
    handle.hide = () => { surface.visible = false; hide(); };
    return handle;
  };
  const app = appOver(session as unknown as AppServerSession, host);
  const running = app.run();
  const input = async (data: string) => { process.stdin.emit("data", data); await waitForApplicationContinuation(); };
  await input("/resume\r");
  if (overlapInitial) await input("/resume\r");
  await input("\x04");
  return {
    session, state, recovery, lists, executes, recovers, responses, previews, lateList,
    cancelled: () => cancelled,
    surface: () => surfaces.findLast((surface) => surface.visible),
    text: () => surfaces.findLast((surface) => surface.visible)?.content.render(120).map(plainText).join("\n") ?? "",
    absent: () => { rows = []; },
    setList: (response: typeof listResponse) => { listResponse = response; },
    input,
    get execution() { return execution; },
    get heldPreview() { return preview; },
    resetExecution: () => { execution = deferred<SessionDeleteResult>(); },
    resetPreview: () => { preview = deferred<SessionDeleteResult>(); },
    resolvePreview: async (revision = "revision") => {
      preview.resolve({ status: "preview", preview: { session_id: "old", name: "historical-target", target_revision: revision, owned_node_count: 1, owned_conversation_count: 1, owned_child_count: 0 } });
      await waitForApplicationContinuation();
    },
    takeover: (interaction: typeof state.pendingInteractions[number]) => {
      session.publishState({ ...state, pendingInteractions: [interaction] });
    },
    resync: () => { session.publishSnapshot(); session.publishState(state); },
    terminal: () => close(new TransportClosedError("process_exit", "transport ended")),
    finish: async () => { await app.quit(); await running; TUI.prototype.showOverlay = original; },
  };
}

it("execute settlement survives approval takeover and retains exact cleanup recovery", async () => {
  const h = await deletionAppHarness();
  try {
    await h.resolvePreview(); await h.input("\t\r");
    assert.deepEqual(h.executes, [["old", "revision"]]);
    const deletionSurface = h.surface();
    h.takeover(approvalInteraction());
    const approvalSurface = h.surface();
    assert.notEqual(approvalSurface, deletionSurface);
    assert.equal(deletionSurface?.visible, false);
    assert.match(h.text(), /Deny/);
    h.absent(); h.execution.resolve({ status: "committed_cleanup_pending", session_id: "old" });
    await waitForApplicationContinuation();
    assert.equal(h.surface(), approvalSurface, "cleanup cannot steal HITL focus");
    assert.deepEqual(h.lists, [[undefined, 0], ["", 0]]);
    assert.deepEqual(h.responses, []); assert.equal(h.cancelled(), 0);
    await h.input("\x1b[27u"); // ordinary approval dismissal, not an answer
    assert.match(h.text(), /removed and cannot be resumed/);
    assert.match(h.text(), /R retry native cleanup/);
    assert.doesNotMatch(h.text(), /historical-target/);
    await h.input("rr\r\x04\x1b[27u");
    assert.deepEqual(h.recovers, ["old"]); assert.equal(h.executes.length, 1);
    assert.equal(h.cancelled(), 0); assert.deepEqual(h.responses, []);
  } finally { await h.finish(); }
});

it("deleted settlement reconciles during questionnaire takeover without stealing focus", async () => {
  const h = await deletionAppHarness();
  try {
    await h.resolvePreview(); await h.input("\t\r");
    const question = questionnaireInteraction(); h.takeover(question);
    const surface = h.surface();
    h.absent(); h.execution.resolve({ status: "deleted", session_id: "old" });
    await waitForApplicationContinuation();
    assert.deepEqual(h.lists, [[undefined, 0], ["", 0]]);
    assert.equal(h.surface(), surface); assert.equal(h.executes.length, 1);
    assert.equal(h.cancelled(), 0); assert.deepEqual(h.responses, []);
    await h.input("\x1b[27u");
    assert.deepEqual(h.responses, [{ interaction: question.interaction, response: { type: "questionnaire", response: { type: "declined" } } }]);
    h.session.publishState(h.state);
    assert.equal(h.surface(), undefined, "success does not reopen the deleted popup");
    await h.input("/resume\r");
    assert.doesNotMatch(h.text(), /historical-target/);
  } finally { await h.finish(); }
});

for (const takeover of ["HITL", "snapshot"] as const) {
  it(`recovery settlement survives ${takeover} replacement and reconciles fresh native visibility`, async () => {
    const h = await deletionAppHarness();
    try {
      await h.resolvePreview(); await h.input("\t\r"); h.absent();
      h.execution.resolve({ status: "committed_cleanup_pending", session_id: "old" });
      await waitForApplicationContinuation(); await h.input("rr");
      assert.deepEqual(h.recovers, ["old"]);
      const old = h.surface();
      if (takeover === "HITL") h.takeover(approvalInteraction()); else h.resync();
      assert.equal(old?.visible, false, "disposable popup really was invalidated");
      const replacement = h.surface();
      h.recovery.resolve({ status: "deleted", session_id: "old" });
      await waitForApplicationContinuation();
      assert.deepEqual(h.lists, [[undefined, 0], ["", 0], ["", 0]]);
      assert.equal(h.executes.length, 1); assert.deepEqual(h.recovers, ["old"]);
      assert.equal(h.cancelled(), 0); assert.deepEqual(h.responses, []);
      if (takeover === "HITL") assert.equal(h.surface(), replacement);
      else assert.doesNotMatch(h.text(), /historical-target/);
    } finally { await h.finish(); }
  });
}

for (const status of ["committed_cleanup_pending", "committed_durability_uncertain", "unknown"] as const) {
  it(`execute ${status} remains recoverable across same-attachment snapshot invalidation`, async () => {
    const h = await deletionAppHarness();
    try {
      await h.resolvePreview(); await h.input("\t\r");
      const old = h.surface(); h.resync(); assert.equal(old?.visible, false);
      h.absent();
      if (status === "unknown") h.execution.reject(new Error("response rejected on live transport"));
      else h.execution.resolve({ status, session_id: "old" });
      await waitForApplicationContinuation();
      assert.deepEqual(h.lists, [[undefined, 0], ["", 0]]);
      assert.match(h.text(), status === "unknown" ? /outcome unknown/ : status === "committed_cleanup_pending" ? /removed and cannot be resumed/ : /durability is uncertain/);
      assert.doesNotMatch(h.text(), /delete failed|historical-target/);
      await h.input("rr"); assert.deepEqual(h.recovers, ["old"]);
      assert.equal(h.executes.length, 1); assert.equal(h.cancelled(), 0);
    } finally { await h.finish(); }
  });
}

for (const takeover of ["HITL", "snapshot"] as const) {
  it(`preview remains disposable across ${takeover} replacement`, async () => {
    const h = await deletionAppHarness();
    try {
      const old = h.surface();
      if (takeover === "HITL") h.takeover(approvalInteraction()); else h.resync();
      assert.equal(old?.visible, false);
      const replacement = h.surface();
      await h.resolvePreview();
      assert.equal(h.surface(), replacement);
      assert.doesNotMatch(h.text(), /Permanently delete/);
      assert.deepEqual(h.executes, []); assert.deepEqual(h.recovers, []);
      assert.equal(h.cancelled(), 0);
    } finally { await h.finish(); }
  });
}

it("terminal transport ends deletion observation without execute replay or fabricated failure", async () => {
  const h = await deletionAppHarness();
  try {
    await h.resolvePreview(); await h.input("\t\r");
    h.terminal(); await waitForApplicationContinuation();
    h.execution.reject(new Error("transport ended")); await waitForApplicationContinuation();
    assert.equal(h.surface(), undefined);
    assert.deepEqual(h.executes, [["old", "revision"]]);
    assert.deepEqual(h.recovers, []); assert.deepEqual(h.lists, [[undefined, 0]]);
    assert.equal(h.cancelled(), 0);
  } finally { await h.finish(); }
});

it("stale settlement waits through HITL for fresh preview and a second explicit confirmation", async () => {
  const h = await deletionAppHarness();
  try {
    await h.resolvePreview(); await h.input("\t\r"); h.resetPreview();
    h.takeover(approvalInteraction()); const approval = h.surface();
    h.execution.resolve({ status: "stale", session_id: "old" });
    await waitForApplicationContinuation();
    assert.equal(h.surface(), approval); assert.deepEqual(h.previews, ["old"]);
    await h.input("\x1b[27u");
    assert.deepEqual(h.previews, ["old", "old"]);
    assert.match(h.text(), /Waiting for native preview/);
    await h.resolvePreview("new-revision");
    assert.match(h.text(), /❯ Cancel/); assert.equal(h.executes.length, 1);
    await h.input("\t\r");
    assert.deepEqual(h.executes, [["old", "revision"], ["old", "new-revision"]]);
  } finally { await h.finish(); }
});

for (const outcome of [
  { status: "blocked", session_id: "old", reason: { kind: "in_use" } },
  { status: "committed_durability_uncertain", session_id: "old" },
  { status: "not_found", session_id: "old" },
] as const) it(`${outcome.status} settlement survives HITL and presents the native result afterward`, async () => {
  const h = await deletionAppHarness();
  try {
    await h.resolvePreview(); await h.input("\t\r");
    h.takeover(approvalInteraction()); const approval = h.surface();
    h.execution.resolve(outcome); await waitForApplicationContinuation();
    assert.equal(h.surface(), approval);
    assert.deepEqual(h.lists, [[undefined, 0], ["", 0]]);
    await h.input("\x1b[27u");
    assert.match(h.text(), outcome.status === "blocked" ? /currently in use/ : outcome.status === "not_found" ? /absent from native authority/ : /durability is uncertain/);
    assert.doesNotMatch(h.text(), /delete failed/);
    if (outcome.status === "committed_durability_uncertain") {
      await h.input("rr"); assert.deepEqual(h.recovers, ["old"]);
    }
    assert.equal(h.executes.length, 1); assert.equal(h.cancelled(), 0);
  } finally { await h.finish(); }
});


it("a delayed initial resume response cannot resurrect a row after deletion reconciliation", async () => {
  const h = await deletionAppHarness(true);
  try {
    await h.resolvePreview(); await h.input("\t\r"); h.absent();
    h.execution.resolve({ status: "deleted", session_id: "old" });
    await waitForApplicationContinuation();
    const reconciled = h.surface();
    assert.doesNotMatch(h.text(), /historical-target/);
    h.lateList.resolve({ sessions: [{ id: "old", name: "historical-target", updated_at: "today", active_node: "node-2", }] });
    await waitForApplicationContinuation();
    assert.equal(h.surface(), reconciled, "pre-mutation initial query cannot reopen a stale selector");
    assert.doesNotMatch(h.text(), /historical-target/);
    assert.deepEqual(h.lists, [[undefined, 0], [undefined, 0], ["", 0]]);
    assert.equal(h.executes.length, 1);
  } finally { await h.finish(); }
});


for (const outcome of ["committed_cleanup_pending", "committed_durability_uncertain", "unknown"] as const) {
  for (const empty of [false, true]) it(`${outcome}: reopening after failed reconciliation honors fresh ${empty ? "empty" : "matching"} authority and retains recovery`, async () => {
    const h = await deletionAppHarness();
    try {
      await h.input("\x1b[27u"); // cancel the initial disposable preview
      await h.input("histor");
      await h.input("\x04"); await h.resolvePreview(); await h.input("\t\r");
      const refresh = deferred<{ sessions: SessionSummaryView[] }>();
      h.setList(() => refresh.promise);
      if (outcome === "unknown") h.execution.reject(new Error("healthy request rejection"));
      else h.execution.resolve({ status: outcome, session_id: "old" });
      await waitForApplicationContinuation();
      refresh.reject(new Error("list rejected on live transport")); await waitForApplicationContinuation();
      assert.equal(h.executes.length, 1);
      assert.match(h.text(), outcome === "unknown" ? /outcome unknown/ : outcome === "committed_durability_uncertain" ? /durability is uncertain/ : /removed and cannot be resumed/);
      assert.doesNotMatch(h.text(), /delete failed/);
      await h.input("\x1b[27u"); assert.match(h.text(), /visibility unavailable/);
      await h.input("\x1b[27u"); assert.equal(h.surface(), undefined);
      h.setList(async () => ({ sessions: empty ? [] : ["A", "C"].map((id) => ({ id, name: `histor-${id}`, updated_at: "today", active_node: id, active: false })) }));
      await h.input("/resume\r");
      assert.deepEqual(h.lists.at(-1), ["histor", 0]);
      assert.match(h.text(), /R retry native cleanup/);
      await h.input("\x1b[27u");
      assert.doesNotMatch(h.text(), /visibility unavailable|historical-target/);
      if (empty) {
        assert.doesNotMatch(h.text(), /histor-A|histor-C/);
        assert.match(h.text(), /no persisted session matches/);
      }
      else { assert.match(h.text(), /histor-A/); assert.match(h.text(), /histor-C/); }
      // The retained action remains available on reopening even with no rows.
      await h.input("\x1b[27u"); await h.input("/resume\r"); await h.input("rr");
      assert.deepEqual(h.recovers, ["old"]); assert.equal(h.executes.length, 1);
      assert.equal(h.cancelled(), 0);
    } finally { await h.finish(); }
  });
}

it("failed reconciliation cannot revive a pre-mutation initial response after a fresh reopen", async () => {
  const h = await deletionAppHarness(true);
  try {
    await h.resolvePreview(); await h.input("\t\r");
    h.setList(async () => { throw new Error("list refresh rejected"); });
    h.execution.resolve({ status: "committed_cleanup_pending", session_id: "old" });
    await waitForApplicationContinuation();
    await h.input("\x1b[27u\x1b[27u");
    h.setList(async () => ({ sessions: [{ id: "A", name: "fresh-A", updated_at: "today", active_node: "A", active: false }] }));
    await h.input("/resume\r"); await h.input("\x1b[27u");
    const fresh = h.surface(); assert.match(h.text(), /fresh-A/);
    h.lateList.resolve({ sessions: [{ id: "old", name: "historical-target", updated_at: "today", active_node: "old", }] });
    await waitForApplicationContinuation();
    assert.equal(h.surface(), fresh); assert.match(h.text(), /fresh-A/);
    assert.doesNotMatch(h.text(), /historical-target/); assert.equal(h.executes.length, 1);
  } finally { await h.finish(); }
});

it("reopening resume retains the current query while recovery retains the original target", async () => {
  const h = await deletionAppHarness();
  const row = (id: string): SessionSummaryView => ({ id, name: id, active_node: id, updated_at: "today" });
  try {
    await h.input("\x1b[27u"); await h.input("old"); await h.input("\x04");
    await h.resolvePreview(); await h.input("\t\r");
    h.setList(async () => ({ sessions: [row("old-A")], nextOffset: 101 }));
    h.execution.resolve({ status: "committed_cleanup_pending", session_id: "old" }); await waitForApplicationContinuation();
    await h.input("\x1b[27u");
    h.setList(async () => ({ sessions: [row("new-A"), row("new-C")], nextOffset: 202 }));
    await h.input("\x7f\x7f\x7fnew");
    assert.match(h.text(), /Search: new/); assert.match(h.text(), /new-C/);
    await h.input("\x1b[27u"); assert.equal(h.surface(), undefined);
    const reopened = h.lists.length;
    await h.input("/resume\r");
    assert.deepEqual(h.lists.slice(reopened), [["new", 0]]);
    assert.match(h.text(), /R retry native cleanup/);
    await h.input("\x1b[27u");
    assert.match(h.text(), /new-A/); assert.match(h.text(), /new-C/); assert.doesNotMatch(h.text(), /old-A/);
    await h.input("\x1b[B\x04");
    const recoveryStart = h.lists.length;
    h.setList(async (query, offset) => {
      assert.equal(query, "new");
      if (offset === 0) return { sessions: [row("new-A")], nextOffset: 202 };
      assert.equal(offset, 202); return { sessions: [row("new-C")] };
    });
    await h.input("rr\r\x04\x1b[A\x1b[B\x1b[27u");
    assert.deepEqual(h.recovers, ["old"]); assert.equal(h.executes.length, 1);
    h.recovery.resolve({ status: "deleted", session_id: "old" }); await waitForApplicationContinuation();
    assert.deepEqual(h.lists.slice(recoveryStart), [["new", 0], ["new", 202]]);
    assert.match(h.text(), /Search: new/); assert.match(h.text(), /new-A/); assert.match(h.text(), /new-C/);
    assert.doesNotMatch(h.text(), /old-A|historical-target/); assert.equal(h.cancelled(), 0);
  } finally { await h.finish(); }
});

for (const replacement of ["HITL", "snapshot"] as const) {
  it(`stale obligation survives ${replacement} replacing an already-pending fresh preview`, async () => {
    const h = await deletionAppHarness();
    try {
      await h.resolvePreview(); await h.input("\t\r"); h.resetPreview();
      h.execution.resolve({ status: "stale", session_id: "old" }); await waitForApplicationContinuation();
      assert.deepEqual(h.previews, ["old", "old"]);
      assert.match(h.text(), /Waiting for native preview/);
      const discardedPreview = h.heldPreview, discardedSurface = h.surface();
      h.resetPreview(); h.resetExecution();
      if (replacement === "HITL") h.takeover(approvalInteraction()); else h.resync();
      assert.equal(discardedSurface?.visible, false);
      discardedPreview.resolve({ status: "preview", preview: { session_id: "old", name: "discarded-preview", target_revision: "discarded-revision", owned_node_count: 1, owned_conversation_count: 1, owned_child_count: 0 } });
      await waitForApplicationContinuation();
      assert.doesNotMatch(h.text(), /discarded-preview|Permanently delete Session/);
      assert.equal(h.executes.length, 1);
      if (replacement === "HITL") {
        assert.match(h.text(), /Deny/); assert.deepEqual(h.previews, ["old", "old"]);
        await h.input("\x1b[27u");
      }
      assert.deepEqual(h.previews, ["old", "old", "old"]);
      assert.match(h.text(), /Waiting for native preview/);
      await h.resolvePreview("rev2");
      assert.match(h.text(), /❯ Cancel/); assert.equal(h.executes.length, 1);
      await h.input("\t\r\r\x04");
      assert.deepEqual(h.executes, [["old", "revision"], ["old", "rev2"]]);
      assert.equal(h.cancelled(), 0); assert.deepEqual(h.responses, []);
    } finally { await h.finish(); }
  });

  it(`known precommit failure after ${replacement} stays non-recoverable`, async () => {
    const h = await deletionAppHarness();
    try {
      await h.resolvePreview(); await h.input("\t\r");
      if (replacement === "HITL") h.takeover(approvalInteraction()); else h.resync();
      h.execution.reject(new AppServerRequestError("session/delete", { code: -32000, message: "Session deletion failed before logical commit.", data: { kind: "operation_failed" } }));
      await waitForApplicationContinuation();
      assert.deepEqual(h.lists, [[undefined, 0], ["", 0]]);
      if (replacement === "HITL") { assert.match(h.text(), /Deny/); await h.input("\x1b[27u"); }
      assert.match(h.text(), /failed before logical commit/);
      assert.doesNotMatch(h.text(), /outcome unknown|durability is uncertain|R retry/);
      await h.input("rr"); assert.deepEqual(h.recovers, []);
      await h.input("\x1b[27u"); assert.match(h.text(), /historical-target/);
      h.resetPreview(); await h.input("\x04");
      assert.deepEqual(h.previews, ["old", "old"]); assert.equal(h.executes.length, 1);
      assert.equal(h.cancelled(), 0); assert.deepEqual(h.responses, []);
    } finally { await h.finish(); }
  });
}

it("approval overlay routes one typed request, consumes Esc, and discards stale snapshot surfaces", async () => {
  const state = stateOf({ attempt: attemptView({ phase: { type: "running" } }) });
  const session = fakeSession(state) as unknown as Record<string, unknown> & {
    publishState(next: typeof state): void; publishSnapshot(): void;
  };
  const response = deferred<Awaited<ReturnType<AppServerSession["approvalModeSet"]>>>();
  const requests: string[] = [];
  let cancellations = 0;
  session.approvalModeSet = (mode: string) => { requests.push(mode); return response.promise; };
  session.cancelCurrentAttempt = async () => { cancellations++; return "a1"; };
  const surfaces: Array<{ content: Parameters<TUI["showOverlay"]>[0]; visible: boolean }> = [];
  const original = TUI.prototype.showOverlay;
  TUI.prototype.showOverlay = function(content, options) {
    const surface = { content, visible: true }; surfaces.push(surface);
    const handle = original.call(this, content, options);
    const hide = handle.hide;
    handle.hide = () => { surface.visible = false; hide(); };
    return handle;
  };
  const app = appOver(session as unknown as AppServerSession);
  const running = app.run();
  const input = async (data: string) => { process.stdin.emit("data", data); await waitForApplicationContinuation(); };
  const active = () => surfaces.findLast((surface) => surface.visible);
  const text = () => active()?.content.render(100).map(plainText).join("\n") ?? "";
  try {
    await input("/approval\r");
    assert.match(text(), /✓ Policy/);
    await input("\x1b[B"); await input("\x1b[27u");
    assert.equal(active(), undefined); assert.equal(cancellations, 0); assert.deepEqual(requests, []);
    await input("/approval\r"); await input("\x1b[B"); await input("\r");
    assert.match(text(), /Enable full access/); assert.match(text(), /❯ Cancel/);
    await input("\r"); assert.deepEqual(requests, []);
    await input("/approval\r"); await input("\x1b[B"); await input("\r");
    await input("\t"); await input("\r"); await input("\r");
    assert.deepEqual(requests, ["full_access"]);
    assert.match(text(), /Current attempt: Policy/);
    await input("\x1b[27u"); await input("/approval\r");
    assert.equal(active(), undefined); assert.deepEqual(requests, ["full_access"]);
    response.resolve({ effectiveApprovalMode: "policy", pendingApprovalMode: "full_access", revision: "1" });
    await waitForApplicationContinuation();
    session.publishState({ ...state, pendingApprovalMode: "full_access", approvalModeRevision: "1" });
    await input("/approval\r");
    assert.match(text(), /Current attempt: Policy/); assert.match(text(), /Next attempt: Full access/);
    const stale = active()!;
    session.publishSnapshot();
    session.publishState({ ...state, effectiveApprovalMode: "full_access", approvalModeRevision: "2" });
    stale.content.handleInput?.("\r");
    assert.deepEqual(requests, ["full_access"]);
    await input("/approval\r");
    assert.match(text(), /✓ Full access/); assert.doesNotMatch(text(), /Next attempt/);
    assert.equal(cancellations, 0);
    await input("\r");
    assert.deepEqual(requests, ["full_access", "policy"]);
    assert.equal((session.state as { effectiveApprovalMode?: string }).effectiveApprovalMode, "full_access", "the response never mutates local effective state");
  } finally {
    await app.quit(); await running; TUI.prototype.showOverlay = original;
  }
});

/** Approval-specific gates over the existing real-app input/attachment seams. */
async function approvalOwnerHarness(t: TestContext, sameAttachment = false) {
  type Reply = Awaited<ReturnType<AppServerSession["approvalModeSet"]>>;
  function owner(model: string) {
    const state = stateOf({ model: sessionModel(model), attempt: attemptView({ phase: { type: "running" } }) });
    const session = fakeSession(state) as unknown as Record<string, unknown> & {
      publishState(next: typeof state): void; publishSnapshot(): void;
    };
    const requests: Array<{ mode: string; response: ReturnType<typeof deferred<Reply>> }> = [];
    let cancelled = 0;
    session.approvalModeSet = (mode: string) => {
      const response = deferred<Reply>(); requests.push({ mode, response }); return response.promise;
    };
    session.cancelCurrentAttempt = async () => { cancelled++; return "active-attempt"; };
    return { state, session, requests, cancelled: () => cancelled };
  }
  const a = owner("owner/a");
  const b = owner("owner/b");
  const bound = deferred<void>();
  const bView = sessionView({ id: "owner/b", name: "owner/b" });
  const surfaces: Array<{ content: Parameters<TUI["showOverlay"]>[0]; visible: boolean }> = [];
  const originalOverlay = TUI.prototype.showOverlay;
  t.mock.method(TUI.prototype, "showOverlay", function(this: TUI, content: Parameters<TUI["showOverlay"]>[0], options: Parameters<TUI["showOverlay"]>[1]) {
    const surface = { content, visible: true }; surfaces.push(surface);
    const handle = originalOverlay.call(this, content, options);
    const hide = handle.hide;
    handle.hide = () => { surface.visible = false; hide(); };
    return handle;
  });
  const feedback: Array<{ level: string; text: string }> = [];
  let transient: TransientFeedbackSurface | undefined;
  const originalReplace = TransientFeedbackSurface.prototype.replace;
  t.mock.method(TransientFeedbackSurface.prototype, "replace", function(this: TransientFeedbackSurface, value: Parameters<TransientFeedbackSurface["replace"]>[0]) {
    transient = this; feedback.push(value); originalReplace.call(this, value);
  });
  const app = appOver(
    a.session as unknown as AppServerSession,
    fakeHost({
      catalog: {
        createSession: async () => {
          if (sameAttachment) {
            // A Session transition can land on the attachment already in
            // focus. The accepted focus change still advances the app's
            // presentation owner epoch, so a late continuation is still stale.
            a.session.publishState(b.state);
            a.session.approvalModeSet = b.session.approvalModeSet;
          }
          return { session: bView };
        },
      },
      attach: async () => {
        bound.resolve();
        return (sameAttachment ? a.session : b.session) as unknown as AppServerSession;
      },
    }),
  );
  const running = app.run();
  const input = async (data: string) => { process.stdin.emit("data", data); await waitForApplicationContinuation(); };
  const active = () => surfaces.findLast((surface) => surface.visible);
  return {
    a, b, input, active, feedback,
    text: () => active()?.content.render(100).map(plainText).join("\n") ?? "",
    feedbackRows: () => transient?.render(50) ?? [],
    enable: async () => {
      await input("/approval\r"); await input("\x1b[B\r"); await input("\t\r");
    },
    switchOwner: async () => {
      await input("/new\r");
      if (!sameAttachment) await bound.promise;
      await waitForApplicationContinuation();
    },
    finish: async () => { await app.quit(); await running; },
  };
}

for (const outcome of ["success", "failure"] as const) {
  it(`${outcome} after approval submission and Esc still reports to the current owner and settles its token`, async (t) => {
    const h = await approvalOwnerHarness(t);
    try {
      await h.enable();
      assert.deepEqual(h.a.requests.map((request) => request.mode), ["full_access"]);
      assert.match(h.text(), /Current attempt: Policy/);
      await h.input("\x1b[27u");
      assert.equal(h.active(), undefined);
      assert.equal(h.a.cancelled(), 0);
      assert.equal((h.a.session.state as { effectiveApprovalMode?: string }).effectiveApprovalMode, "policy");
      await h.input("/approval\r");
      assert.equal(h.active(), undefined, "Esc did not settle the still-pending native request");
      const before = h.feedback.length;
      const request = h.a.requests[0]!;
      if (outcome === "success") request.response.resolve({ effectiveApprovalMode: "policy", pendingApprovalMode: "full_access", revision: "1" });
      else request.response.reject(new Error("native rejection\n" + "detail ".repeat(100)));
      await waitForApplicationContinuation();
      assert.equal(h.feedback.length, before + 1);
      const feedback = h.feedback.at(-1)!;
      assert.equal(feedback.level, outcome === "success" ? "info" : "error");
      assert.match(feedback.text, outcome === "success" ? /accepted: effective Policy · next attempt Full access/ : /Approval change failed: native rejection/);
      assert.ok(h.feedbackRows().length <= 3);
      assert.ok(h.feedbackRows().every((row) => plainWidth(row) <= 50));
      assert.equal((h.a.session.state as { effectiveApprovalMode?: string }).effectiveApprovalMode, "policy", "no optimistic mutation");
      assert.equal((h.a.session.state as { pendingApprovalMode?: string }).pendingApprovalMode, undefined, "control reply is not copied into projection");
      await h.input("/approval\r");
      assert.match(h.text(), /Approval mode/, "its own completion released the token");
      assert.equal(h.a.requests.length, 1);
    } finally { await h.finish(); }
  });
}

for (const sameAttachment of [false, true]) {
  for (const outcome of ["success", "failure"] as const) {
    it(`old approval ${outcome} cannot clear or repaint B's request after ${sameAttachment ? "Session ownership changes on the same attachment" : "attachment replacement"}`, async (t) => {
      const h = await approvalOwnerHarness(t, sameAttachment);
      try {
        await h.enable();
        const stale = h.active()!;
        await h.input("\x1b[27u");
        await h.switchOwner();
        // Old popup input is stale even if called directly after replacement.
        stale.content.handleInput?.("\r");
        await h.enable();
        assert.equal(h.a.requests.length, 1);
        assert.equal(h.b.requests.length, 1, "A's pending operation does not block B");
        assert.match(h.text(), /Current attempt: Policy/);
        const before = [...h.feedback];
        const aRequest = h.a.requests[0]!;
        if (outcome === "success") aRequest.response.resolve({ effectiveApprovalMode: "full_access", revision: "99" });
        else aRequest.response.reject(new Error("stale owner A failure"));
        await waitForApplicationContinuation();
        assert.deepEqual(h.feedback, before, "A's result/error cannot repaint B");
        assert.match(h.text(), /Current attempt: Policy/);
        assert.doesNotMatch(h.text(), /Current attempt: Full access|stale owner/);
        await h.input("\r\r");
        assert.equal(h.b.requests.length, 1);
        await h.input("\x1b[27u"); await h.input("/approval\r");
        assert.equal(h.active(), undefined, "A's finally cannot clear B's pending token");
        assert.equal(h.b.requests.length, 1);
        const current = sameAttachment ? h.a.session : h.b.session;
        current.publishState({ ...h.b.state, pendingApprovalMode: "full_access", approvalModeRevision: "2" });
        h.b.requests[0]!.response.resolve({ effectiveApprovalMode: "policy", pendingApprovalMode: "full_access", revision: "2" });
        await waitForApplicationContinuation();
        assert.match(h.feedback.at(-1)!.text, /accepted: effective Policy · next attempt Full access/);
        await h.input("/approval\r");
        assert.match(h.text(), /Current attempt: Policy/);
        assert.match(h.text(), /Next attempt: Full access/);
        assert.equal((current.state as { effectiveApprovalMode?: string }).effectiveApprovalMode, "policy");
        assert.equal(h.a.cancelled() + h.b.cancelled(), 0);
        // B's own completion (and no other completion) admits the next request.
        await h.input("\r");
        assert.equal(h.b.requests.length, 2);
        h.b.requests[1]!.response.resolve({ effectiveApprovalMode: "policy", revision: "3" });
        await waitForApplicationContinuation();
      } finally { await h.finish(); }
    });
  }
}

it("remote recovery installs a fresh attachment and fences old callbacks without replay", async () => {
  let close!: (error: TransportClosedError) => void;
  const old = fakeSession(emptyPresentationState(sessionModel("alpha/model-a")));
  const next = fakeSession(emptyPresentationState(sessionModel("beta/model-b")));
  const pending = deferred<{ messageId: string; sequence: string }>();
  let submissions = 0;
  old.submitInbound = () => { submissions++; return pending.promise; };
  next.submitInbound = async () => { submissions++; return { messageId: "new", sequence: "2" }; };
  const first = fakeHost({ ownership: "external", onClose: (listener) => { close = listener; } });
  let attachments = 0;
  const second = fakeHost({ ownership: "external", attach: async (id) => {
    assert.equal(id, old.sessionId); attachments++; return next;
  } });
  let connects = 0;
  const app = new RustxTuiApp({ host: first, session: old, sessionSettings: SESSION_SETTINGS,
    reconnect: async () => { connects++; return second; } });
  const running = app.run();
  await waitForApplicationContinuation();
  process.stdin.emit("data", "first submission\r");
  await waitForApplicationContinuation();
  const error = new TransportClosedError("input_eof", "lost connection");
  Object.defineProperty(first.client, "closed", { value: error });
  close(error);
  await waitForApplicationContinuation();
  pending.reject(new UncertainOutcomeError("turn/start", error));
  await waitForApplicationContinuation();
  assert.equal(connects, 1);
  assert.equal(attachments, 1);
  assert.equal(submissions, 1, "the uncertain turn was not replayed");
  close(error);
  await waitForApplicationContinuation();
  assert.equal(connects, 1, "an old connection callback cannot replace recovery");
  process.stdin.emit("data", "new intentional submission\r");
  await waitForApplicationContinuation();
  assert.equal(submissions, 2, "new input uses the replacement attachment");
  await app.quit();
  await running;
});

it("quitting while remote recovery is pending closes the late connection", async () => {
  let close!: (error: TransportClosedError) => void;
  const connection = deferred<AppServerHost>();
  const first = fakeHost({ ownership: "external", onClose: (listener) => { close = listener; } });
  const log: string[] = [];
  const second = fakeHost({ ownership: "external", log });
  const app = new RustxTuiApp({ host: first, session: fakeSession(), sessionSettings: SESSION_SETTINGS,
    reconnect: () => connection.promise });
  close(new TransportClosedError("input_eof", "lost connection"));
  await app.quit();
  connection.resolve(second);
  await waitForApplicationContinuation();
  assert.deepEqual(log, ["disconnect"]);
});
