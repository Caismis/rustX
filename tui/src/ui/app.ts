/**
 * The rustX terminal application.
 *
 * This is the **outermost** layer. Pi supplies terminal mechanics — a
 * differential renderer, a multiline editor with history and autocomplete,
 * Markdown layout, overlays, a spinner. rustX supplies every semantic: what a
 * message means, which model an attempt is on, what a tool is doing, what is
 * still running in the background.
 *
 * ```text
 * PresentationProjection
 *        |
 *        +-- correlation/selectors
 *        |
 *        +-- rustX semantic components
 *                  |
 *                  v
 *            Pi primitives
 * ```
 *
 * No Pi class holds authoritative rustX state. Every component here is a
 * disposable render target rebuilt wholesale from the projection, so a fresh
 * `RuntimeClientSnapshot` reconstructs the entire UI without consulting
 * anything Pi remembers, and component instance continuity is never a
 * correctness requirement. Nothing resembling Pi's `AgentSession`,
 * `SessionManager`, model runtime, provider registry, tool registry, or
 * `InteractiveMode` exists here or anywhere in this package.
 *
 * The one thing the app owns that the projection does not is
 * {@link PresentationPreferences} — reasoning visibility, and which cards are
 * expanded in each of the three runtime identity domains (`ToolCallId` for
 * foreground tool cards, `ToolExecutionId` for background ones, `InteractionId`
 * for pending approvals). Those are display choices, they are deliberately not
 * written into runtime state, and losing them on a rebuild costs nothing
 * semantic: every collapsed band is restored from `PresentationState` alone.
 */

import {
  Container,
  Editor,
  Loader,
  Markdown,
  ProcessTerminal,
  Spacer,
  Text,
  TUI,
  matchesKey,
  type OverlayHandle,
} from "@earendil-works/pi-tui";

import { SlashCommandAutocompleteProvider } from "../commands/autocomplete.ts";
import {
  CommandDispatcher,
  type DebugDiagnostics,
  type ExpandTarget,
  type PreferenceChange,
} from "../commands/dispatcher.ts";
import {
  withNotice,
  withPendingSubmission,
} from "../presentation/projection.ts";
import { correlateTools } from "../presentation/tools.ts";
import type { PresentationState } from "../presentation/state.ts";
import type { ChildRuntimeProcess } from "../runtime/child-process.ts";
import type { RuntimeClientConnection } from "../runtime/connection.ts";
import type { SessionSwitch } from "../runtime/attachment.ts";
import type { RuntimeClientAttachment } from "../runtime/attachment.ts";
import type {
  CatalogModelView,
  SessionSummaryView,
  SessionUserMessageBoundaryView,
  SessionView,
  ToolCallId,
} from "../protocol/types.ts";
import {
  BoundarySelector,
  SessionSelector,
} from "./components/session-selector.ts";
import { TreeSelector, type TreeSelection } from "./components/tree-selector.ts";
import {
  renderBackgroundSection,
  renderInteractionSection,
  renderOrphanExecutions,
} from "./components/activity.ts";
import { ModelSelector } from "./components/model-selector.ts";
import { renderFooter, workingStatus } from "./components/status.ts";
import { renderTranscript } from "./components/transcript.ts";
import {
  type PresentationPreferences,
  defaultPreferences,
  withAllCollapsed,
  withExpandedBackgroundExecutions,
  withExpandedInteractions,
  withExpandedToolCalls,
  withReasoningVisible,
  withToggledBackgroundExecution,
  withToggledInteraction,
  withToggledToolCall,
} from "./preferences.ts";
import { editorTheme, markdownTheme, style } from "./theme.ts";

export interface RustxTuiAppOptions {
  session: RuntimeClientAttachment;
  connection: RuntimeClientConnection;
  child: ChildRuntimeProcess;
  /** Re-spawns and re-attaches after Rust publishes a lineage switch. */
  restartRuntime?: () => Promise<RuntimeAttachmentHandle>;
  /** How long the child gets to exit after the shutdown sequence. */
  terminationGraceMs?: number;
}

export interface RuntimeAttachmentHandle {
  session: RuntimeClientAttachment;
  connection: RuntimeClientConnection;
  child: ChildRuntimeProcess;
}

export class RustxTuiApp {
  #session: RuntimeClientAttachment;
  #connection: RuntimeClientConnection;
  #child: ChildRuntimeProcess;
  readonly #dispatcher: CommandDispatcher;
  readonly #restartRuntime: (() => Promise<RuntimeAttachmentHandle>) | undefined;
  readonly #terminationGraceMs: number | undefined;

  readonly #tui: TUI;
  readonly #transcript = new Container();
  readonly #activity = new Container();
  readonly #notices = new Container();
  readonly #footer = new Text("", 1, 0);
  readonly #editor: Editor;
  readonly #loader: Loader;

  #preferences: PresentationPreferences = defaultPreferences();
  #overlay: OverlayHandle | undefined;
  #quitting = false;
  #exitCode = 0;
  #finished = false;
  #started = false;
  #restarting = false;
  #removeStateListener: (() => void) | undefined;
  #removeCloseListener: (() => void) | undefined;
  #resolveExit: ((code: number) => void) | undefined;

  constructor(options: RustxTuiAppOptions) {
    this.#session = options.session;
    this.#connection = options.connection;
    this.#child = options.child;
    this.#restartRuntime = options.restartRuntime;
    this.#terminationGraceMs = options.terminationGraceMs;

    this.#tui = new TUI(new ProcessTerminal());
    this.#editor = new Editor(this.#tui, editorTheme, { paddingX: 1 });
    this.#editor.setAutocompleteProvider(new SlashCommandAutocompleteProvider());
    this.#editor.onSubmit = (text) => {
      void this.#onSubmit(text);
    };
    this.#loader = new Loader(this.#tui, style.cyan, style.dim, "");

    this.#dispatcher = new CommandDispatcher({
      session: this.#session,
      diagnostics: () => this.#diagnostics(),
    });

    this.#tui.addChild(this.#transcript);
    this.#tui.addChild(this.#activity);
    this.#tui.addChild(this.#notices);
    this.#tui.addChild(new Spacer(1));
    this.#tui.addChild(this.#editor);
    this.#tui.addChild(this.#footer);

    this.#bindRuntime(this.#session, this.#connection, this.#child);
  }

  #bindRuntime(
    session: RuntimeClientAttachment,
    connection: RuntimeClientConnection,
    child: ChildRuntimeProcess,
  ): void {
    this.#removeStateListener?.();
    this.#removeCloseListener?.();
    this.#session = session;
    this.#connection = connection;
    this.#child = child;
    this.#dispatcher.setSession(session);
    this.#removeStateListener = session.onState((state) => this.#renderState(state));
    this.#removeCloseListener = connection.onClose((error) => {
      if (this.#restarting) return;
      // Transport loss is not cancellation. It only ends observation, so the
      // client says exactly that and stops accepting input.
      this.#note(
        "error",
        `${error.message}\nThe runtime is no longer observable from this client.`,
      );
      this.#editor.disableSubmit = true;
      if (!this.#quitting) this.#finish(1);
    });
  }

  /** Starts the terminal and resolves with the process exit code. */
  run(): Promise<number> {
    return new Promise<number>((resolve) => {
      this.#resolveExit = resolve;
      // The connection can become terminal before run() installs its waiter.
      // #finish records that result, so startup never returns a promise that
      // can no longer be resolved.
      if (this.#finished) {
        this.#resolveExit = undefined;
        resolve(this.#exitCode);
        return;
      }

      this.#started = true;
      this.#tui.start();
      this.#tui.setFocus(this.#editor);
      const state = this.#session.state;
      if (state !== undefined) {
        this.#renderState(state);
      }
      // Session metadata is a separate native product projection from the
      // conversation snapshot. Refresh it after the attachment handshake so
      // startup remains the same initialize/subscribe cut used by existing
      // Runtime Client consumers, while the footer still becomes session-aware
      // as soon as the authoritative read returns.
      const refreshSession = (
        this.#session as unknown as {
          refreshSession?: () => Promise<unknown>;
        }
      ).refreshSession;
      if (refreshSession !== undefined) {
        void refreshSession.call(this.#session).then(
          () => {
            const refreshed = this.#session.state;
            if (refreshed !== undefined) this.#renderState(refreshed);
          },
          (error: unknown) => {
            this.#note("error", `session metadata unavailable: ${(error as Error).message}`);
          },
        );
      }
      this.#tui.addInputListener((data) => {
        // Ctrl+C is a cancellation *intent*, routed through the protocol like
        // any other; it never kills the runtime behind the runtime's back.
        if (matchesKey(data, "ctrl+c")) {
          void this.#onInterrupt();
          return { consume: true };
        }
        if (matchesKey(data, "escape")) {
          const attempt = this.#session.state?.attempt;
          const acted = this.#overlay !== undefined || (
            !this.#restarting &&
            attempt !== undefined &&
            attempt.phase.type !== "settled"
          );
          void this.#onEscape();
          return acted ? { consume: true } : undefined;
        }
        // Ctrl+O and Ctrl+T are presentation only. They change what is drawn
        // and send nothing to the runtime.
        if (matchesKey(data, "ctrl+o")) {
          this.#applyPreference({ type: "expand", target: "latest" });
          return { consume: true };
        }
        if (matchesKey(data, "ctrl+t")) {
          this.#applyPreference({ type: "reasoning" });
          return { consume: true };
        }
        return undefined;
      });
    });
  }

  async #onSubmit(text: string): Promise<void> {
    if (this.#restarting || this.#finished) return;
    const line = text.trim();
    if (line.length === 0) {
      return;
    }
    this.#editor.addToHistory(text);
    this.#editor.setText("");

    // Optimistic echo, explicitly transient: it is reconciled away by the
    // runtime's authoritative inbound fact and is never canonical history.
    const key = `local-${Date.now()}-${line.length}`;
    const optimistic = !line.startsWith("/");
    if (optimistic) {
      this.#session.updateState((state) =>
        withPendingSubmission(state, key, line),
      );
    }

    await this.#handleOutcome(await this.#dispatcher.submit(text));
  }

  async #handleOutcome(
    outcome: Awaited<ReturnType<CommandDispatcher["submit"]>>,
  ): Promise<void> {
    switch (outcome.kind) {
      case "message":
        this.#note(outcome.level, outcome.text);
        break;
      case "choose_model":
        this.#showModelSelector(outcome.models);
        break;
      case "choose_session":
        this.#showSessionSelector(outcome.sessions, outcome.nextOffset, outcome.query);
        break;
      case "choose_fork":
        this.#showBoundarySelector(
          outcome.boundaries,
          "Fork from user message",
          "fork",
          outcome.nextOffset,
        );
        break;
      case "choose_tree":
        this.#showTreeSelector(
          outcome.session,
          outcome.nodes,
          outcome.nextNodeOffset,
          outcome.boundaries,
          outcome.nextHistoryOffset,
        );
        break;
      case "session_switch":
        await this.#applySessionSwitch(outcome.change);
        break;
      case "replacement_required":
        await this.#applyReplacementRequired(outcome.message);
        break;
      case "preference":
        this.#applyPreference(outcome.preference);
        break;
      case "quit":
        await this.quit();
        break;
      case "none":
        break;
    }
  }

  async #onEscape(): Promise<void> {
    if (this.#overlay !== undefined) {
      this.#closeOverlay();
      return;
    }
    if (this.#restarting) return;
    const attempt = this.#session.state?.attempt;
    if (attempt !== undefined && attempt.phase.type !== "settled") {
      await this.#handleOutcome(await this.#dispatcher.submit("/cancel"));
    }
  }

  async #onInterrupt(): Promise<void> {
    if (this.#restarting) return;
    const state = this.#session.state;
    if (state?.attempt !== undefined && state.attempt.phase.type !== "settled") {
      const outcome = await this.#dispatcher.submit("/cancel");
      if (outcome.kind === "message") {
        this.#note(outcome.level, outcome.text);
      }
      return;
    }
    await this.quit();
  }

  /**
   * The controlling-client shutdown sequence.
   *
   * ```text
   * disable new input
   *   -> shutdown          (canonical runtime request)
   *   -> wait for the exact active AttemptSettled fact, when needed
   *   -> close stdin       (transport EOF, never cancellation)
   *   -> wait              (bounded process-level fallback if it overstays)
   * ```
   *
   * None of these steps claims that background work completed: the process
   * stopping is not a semantic settlement, and this client never reports one.
   */
  async quit(): Promise<void> {
    if (this.#quitting) {
      return;
    }
    this.#quitting = true;
    this.#editor.disableSubmit = true;
    this.#note("info", "shutting the runtime down…");

    const attempt = this.#session.state?.attempt;
    const unsettledAttemptId =
      attempt !== undefined && attempt.phase.type !== "settled"
        ? attempt.attemptId
        : undefined;
    let lifecycleFailure = false;
    try {
      await this.#session.shutdown();
    } catch (error) {
      lifecycleFailure = true;
      this.#note("error", `shutdown request failed: ${(error as Error).message}`);
    }

    if (!lifecycleFailure && unsettledAttemptId !== undefined) {
      try {
        await this.#session.waitForAttemptSettlement(unsettledAttemptId);
      } catch (error) {
        lifecycleFailure = true;
        this.#note(
          "error",
          `attempt settlement was not observed: ${(error as Error).message}`,
        );
      }
    }

    this.#child.closeStdin();
    const exit = await this.#child.waitOrTerminate(this.#terminationGraceMs);
    this.#exitCode = exit.code ?? 1;
    if (lifecycleFailure && this.#exitCode === 0) {
      this.#exitCode = 1;
    }
    this.#finish(this.#exitCode);
  }

  /**
   * Opens the model selector over the editor.
   *
   * The overlay owns focus while it is up and hands it straight back to the
   * editor on select or cancel, so the editor is never left unfocused.
   */
  #showModelSelector(models: CatalogModelView[]): void {
    const state = this.#session.state;
    if (state === undefined) {
      return;
    }
    const selector = new ModelSelector({
      models,
      sessionModel: state.sessionModel,
      attempt: state.attempt,
    });
    const handle = this.#tui.showOverlay(selector, {
      width: "80%",
      maxHeight: "70%",
      anchor: "center",
    });
    this.#overlay = handle;

    const close = () => {
      handle.hide();
      this.#overlay = undefined;
      this.#tui.setFocus(this.#editor);
      this.#tui.requestRender();
    };

    selector.onChange = () => this.#tui.requestRender();
    selector.onCancel = close;
    selector.onSelect = (model) => {
      close();
      void this.#dispatcher.selectModel(model).then((outcome) => this.#handleOutcome(outcome)).catch((error: unknown) => {
        this.#note("error", `model selection failed: ${errorMessage(error)}`);
      });
    };

    handle.focus();
    this.#tui.requestRender();
  }

  #showSessionSelector(
    sessions: SessionSummaryView[],
    nextOffset: number | undefined,
    query: string,
  ): void {
    if (sessions.length === 0) {
      this.#note("info", "no persisted sessions are available");
      return;
    }
    let currentQuery = query;
    let currentNextOffset = nextOffset;
    let requestSerial = 0;
    const selector = new SessionSelector({ sessions, nextOffset, query });
    const handle = this.#tui.showOverlay(selector, {
      width: "80%",
      maxHeight: "70%",
      anchor: "center",
    });
    this.#overlay = handle;
    selector.onChange = () => this.#tui.requestRender();
    selector.onCancel = () => this.#closeOverlay();
    selector.onQueryChange = (nextQuery) => {
      currentQuery = nextQuery;
      currentNextOffset = undefined;
      const serial = ++requestSerial;
      void this.#session.listSessions(nextQuery, 0).then((page) => {
        if (serial !== requestSerial) return;
        currentNextOffset = page.nextOffset;
        selector.replacePage(page.sessions, page.nextOffset);
      }).catch((error: unknown) => {
        if (serial !== requestSerial) return;
        this.#note("error", `session search failed: ${errorMessage(error)}`);
        selector.replacePage([], undefined);
      });
    };
    selector.onLoadMore = () => {
      const offset = currentNextOffset;
      if (offset === undefined) return;
      const serial = requestSerial;
      void this.#session.listSessions(currentQuery, offset).then((page) => {
        if (serial !== requestSerial) return;
        currentNextOffset = page.nextOffset;
        selector.appendPage(page.sessions, page.nextOffset);
      }).catch((error: unknown) => {
        this.#note("error", `session page failed: ${errorMessage(error)}`);
        selector.appendPage([], currentNextOffset);
      });
    };
    selector.onSelect = (session) => {
      this.#closeOverlay();
      void this.#dispatcher.selectSession(session.id)
        .then((outcome) => this.#handleOutcome(outcome))
        .catch((error: unknown) => {
          this.#note("error", `session selection failed: ${errorMessage(error)}`);
        });
    };
    handle.focus();
    this.#tui.requestRender();
  }

  #showBoundarySelector(
    boundaries: SessionUserMessageBoundaryView[],
    title: string,
    operation: "fork" | "tree",
    nextOffset?: number,
  ): void {
    if (boundaries.length === 0) {
      this.#note(
        "info",
        "the active lineage has no committed user-message boundary",
      );
      return;
    }
    let currentNextOffset = nextOffset;
    const selector = new BoundarySelector({ boundaries, title, nextOffset });
    const handle = this.#tui.showOverlay(selector, {
      width: "80%",
      maxHeight: "70%",
      anchor: "center",
    });
    this.#overlay = handle;
    selector.onChange = () => this.#tui.requestRender();
    selector.onCancel = () => this.#closeOverlay();
    selector.onLoadMore = () => {
      const offset = currentNextOffset;
      if (offset === undefined) return;
      void this.#session.sessionTreePage(0, offset).then((page) => {
        currentNextOffset = page.nextHistoryOffset;
        selector.appendPage(page.branchableMessages, page.nextHistoryOffset);
      }).catch((error: unknown) => {
        this.#note("error", `history page failed: ${errorMessage(error)}`);
        selector.appendPage([], currentNextOffset);
      });
    };
    selector.onSelect = (boundary) => {
      this.#closeOverlay();
      const request = operation === "fork"
        ? this.#dispatcher.forkAt(boundary)
        : this.#dispatcher.branchAt(boundary);
      void request
        .then((outcome) => this.#handleOutcome(outcome))
        .catch((error: unknown) => {
          this.#note("error", `session switch failed: ${errorMessage(error)}`);
        });
    };
    handle.focus();
    this.#tui.requestRender();
  }

  #showTreeSelector(
    session: SessionView,
    nodes: import("../protocol/types.ts").SessionNodeView[],
    nextNodeOffset: number | undefined,
    boundaries: SessionUserMessageBoundaryView[],
    nextHistoryOffset: number | undefined,
  ): void {
    const selector = new TreeSelector({
      session,
      nodes,
      nextNodeOffset,
      boundaries,
      nextHistoryOffset,
    });
    const handle = this.#tui.showOverlay(selector, {
      width: "80%",
      maxHeight: "70%",
      anchor: "center",
    });
    this.#overlay = handle;
    selector.onChange = () => this.#tui.requestRender();
    selector.onCancel = () => this.#closeOverlay();
    selector.onLoadMore = () => {
      const request = selector.nextPageRequest();
      if (request === undefined) return;
      void this.#session.sessionTreePage(request.nodeOffset, request.historyOffset).then((page) => {
        selector.appendPage({
          nodes: page.nodes,
          nextNodeOffset: page.nextNodeOffset,
          boundaries: page.branchableMessages,
          nextHistoryOffset: page.nextHistoryOffset,
        });
      }).catch((error: unknown) => {
        this.#note("error", `tree page failed: ${errorMessage(error)}`);
        selector.retryPage();
      });
    };
    selector.onSelect = (selection: TreeSelection) => {
      this.#closeOverlay();
      const request = selection.kind === "node"
        ? this.#dispatcher.selectTreeNode(session.id, selection.node.id)
        : this.#dispatcher.branchAt(selection.boundary);
      void request
        .then((outcome) => this.#handleOutcome(outcome))
        .catch((error: unknown) => {
          this.#note("error", `session switch failed: ${errorMessage(error)}`);
        });
    };
    handle.focus();
    this.#tui.requestRender();
  }

  #closeOverlay(): void {
    const handle = this.#overlay;
    if (handle === undefined) return;
    handle.hide();
    this.#overlay = undefined;
    this.#tui.setFocus(this.#editor);
    this.#tui.requestRender();
  }

  async #applySessionSwitch(change: SessionSwitch): Promise<void> {
    if (!change.restartRequired) {
      this.#note(
        "info",
        `active session: ${change.session.name} · node ${change.session.active_node}`,
      );
      return;
    }
    const restart = this.#restartRuntime;
    if (restart === undefined) {
      this.#note("error", "the runtime cannot be replaced by this attachment");
      this.#editor.disableSubmit = true;
      this.#finish(1);
      return;
    }

    this.#restarting = true;
    this.#editor.disableSubmit = true;
    this.#note(
      change.restartDiagnostic === undefined ? "info" : "error",
      change.restartDiagnostic === undefined
        ? `switching to session ${change.session.name}…`
        : `Session ${change.session.name} committed before durability became uncertain: ${change.restartDiagnostic}`,
    );
    const oldSession = this.#session;
    const oldConnection = this.#connection;
    const oldChild = this.#child;
    try {
      // Rust has already reached its semantic quiescence point before it
      // returned this result. Closing here only releases the old process
      // attachment; it is never the cancellation operation.
      await this.#detachOldAttachment(oldSession, oldConnection);
      oldChild.closeStdin();
      await oldChild.waitOrTerminate(this.#terminationGraceMs);

      const next = await restart();
      this.#bindRuntime(next.session, next.connection, next.child);
      const authoritative = await next.session.refreshSession();
      if (change.editorContent !== undefined) {
        if (!sameSessionLineage(change.session, authoritative)) {
          throw new Error(
            "restarted Rust process selected a different Session/node than the committed transition",
          );
        }
        this.#editor.setText(editorText(change.editorContent));
      }
      this.#note(
        "info",
        `active session: ${authoritative.name} · node ${authoritative.active_node}`,
      );
      const state = this.#session.state;
      if (state !== undefined) this.#renderState(state);
    } catch (error) {
      this.#note("error", `session switch failed: ${errorMessage(error)}`);
      this.#editor.disableSubmit = true;
      this.#finish(1);
    } finally {
      this.#restarting = false;
      if (!this.#finished) {
        this.#editor.disableSubmit = this.#quitting;
      }
    }
  }

  /**
   * Replaces an attachment after Rust reports its terminal Session state.
   * The restarted process reads the catalog again; this method deliberately
   * does not infer a destination Session from the failed command.
   */
  async #applyReplacementRequired(message: string): Promise<void> {
    if (this.#restarting || this.#finished) return;
    const restart = this.#restartRuntime;
    this.#restarting = true;
    this.#editor.disableSubmit = true;
    this.#closeOverlay();
    this.#note("error", `the active Session attachment must be replaced: ${message}`);
    if (restart === undefined) {
      this.#finish(1);
      return;
    }

    const oldSession = this.#session;
    const oldConnection = this.#connection;
    const oldChild = this.#child;
    try {
      await this.#detachOldAttachment(oldSession, oldConnection);
      oldChild.closeStdin();
      await oldChild.waitOrTerminate(this.#terminationGraceMs);

      const next = await restart();
      this.#bindRuntime(next.session, next.connection, next.child);
      const authoritative = await next.session.refreshSession();
      this.#note(
        "info",
        `attached to authoritative Session ${authoritative.name} · node ${authoritative.active_node}`,
      );
      const state = this.#session.state;
      if (state !== undefined) this.#renderState(state);
    } catch (error) {
      this.#note("error", `Session attachment replacement failed: ${errorMessage(error)}`);
      this.#editor.disableSubmit = true;
      this.#finish(1);
    } finally {
      this.#restarting = false;
      if (!this.#finished) {
        this.#editor.disableSubmit = this.#quitting;
      }
    }
  }

  async #detachOldAttachment(
    session: RuntimeClientAttachment,
    connection: RuntimeClientConnection,
  ): Promise<void> {
    try {
      await session.detach();
    } catch {
      // The transport may already be closing. Closing it still releases the
      // client-side attachment and never claims semantic cancellation.
    }
    connection.close();
  }

  /**
   * Applies one presentation preference and redraws.
   *
   * Nothing here touches `PresentationState`, sends a request, or changes what
   * rustX was asked to do.
   */
  #applyPreference(change: PreferenceChange): void {
    switch (change.type) {
      case "reasoning":
        this.#preferences = withReasoningVisible(
          this.#preferences,
          change.visible ?? !this.#preferences.reasoningVisible,
        );
        break;
      case "expand_call":
        this.#preferences = withToggledToolCall(this.#preferences, change.callId);
        break;
      case "expand_background":
        this.#preferences = withToggledBackgroundExecution(
          this.#preferences,
          change.executionId,
        );
        break;
      case "expand_interaction":
        this.#preferences = withToggledInteraction(
          this.#preferences,
          change.interactionId,
        );
        break;
      case "expand":
        this.#preferences = this.#expandTarget(change.target);
        break;
      default:
        break;
    }
    const state = this.#session.state;
    if (state !== undefined) {
      this.#renderState(state);
    }
  }

  /**
   * The bulk expansion targets.
   *
   * `all` and `none` cover *every* identity domain — each renderable tool card
   * keyed by `ToolCallId`, each renderable background card keyed by
   * `ToolExecutionId`, and each pending approval keyed by `InteractionId`.
   * The three sets are kept separate so ids that happen to serialize alike
   * never cross-toggle.
   *
   * `all` names only entities the projection currently renders, so it never
   * seeds a preference for something already settled.
   */
  #expandTarget(target: ExpandTarget): PresentationPreferences {
    const state = this.#session.state;
    if (target === "none" || state === undefined) {
      return withAllCollapsed(this.#preferences);
    }
    const calls = [...correlateTools(state).byCallId.keys()];
    if (target === "all") {
      const executions = state.background.map(
        (execution) => execution.execution_id,
      );
      const interactions = state.pendingInteractions.map(
        (interaction) => interaction.id,
      );
      return withExpandedInteractions(
        withExpandedBackgroundExecutions(
          withExpandedToolCalls(this.#preferences, calls),
          executions,
        ),
        interactions,
      );
    }
    // "latest" is the most recently correlated *tool call*, which is the one a
    // user pressing ctrl+o is looking at. Correlation order follows the
    // transcript, never screen position. It deliberately stays scoped to one
    // domain: "the latest" across three unrelated identity domains would name
    // whichever entity a tie-break rule picked, not the one on screen.
    const latest: ToolCallId | undefined = calls[calls.length - 1];
    return latest === undefined
      ? this.#preferences
      : withToggledToolCall(this.#preferences, latest);
  }

  #note(level: "info" | "error", text: string): void {
    this.#session.updateState((state) =>
      withNotice(state, { key: `note-${state.notices.length}`, level, text }),
    );
  }

  /**
   * Rebuilds the visible components from the projection.
   *
   * The rebuild is total: every component is discarded and reconstructed from
   * `state`. That is what makes the UI reconstructable from a fresh snapshot —
   * no Pi component carries state the projection does not have.
   */
  #renderState(state: PresentationState): void {
    // Correlated once per render and shared: the transcript and the activity
    // area must agree on which calls have a transcript anchor.
    const correlation = correlateTools(state);

    this.#transcript.clear();
    for (const block of renderTranscript(state, this.#preferences, correlation)) {
      this.#transcript.addChild(
        block.kind === "markdown"
          ? new Markdown(
              block.markdown,
              1,
              0,
              markdownTheme,
              block.defaultTextStyle,
            )
          : new Text(block.text, 1, 0),
      );
      this.#transcript.addChild(new Spacer(1));
    }

    // The activity area holds only what is *not* conversation content. A
    // foreground tool call renders inside the assistant message that asked
    // for it, which is what keeps one call to one card.
    this.#activity.clear();
    for (const section of [
      renderOrphanExecutions(correlation, this.#preferences),
      renderBackgroundSection(state, this.#preferences),
      renderInteractionSection(state, this.#preferences),
    ]) {
      if (section.length > 0) {
        this.#activity.addChild(new Text(section, 1, 0));
      }
    }

    const working = workingStatus(state);
    if (working === undefined) {
      this.#loader.stop();
    } else {
      this.#loader.setMessage(working);
      this.#loader.start();
      this.#activity.addChild(this.#loader);
    }

    this.#notices.clear();
    // Only the most recent notices are shown; client chatter never grows
    // without bound and never competes with runtime facts for the screen.
    for (const notice of state.notices.slice(-4)) {
      this.#notices.addChild(
        new Markdown(
          notice.level === "error" ? style.red(notice.text) : notice.text,
          1,
          0,
          markdownTheme,
        ),
      );
    }

    this.#footer.setText(
      renderFooter(
        state,
        this.#connectionLabel(),
        this.#tui.terminal.columns,
        this.#session.sessionInfo,
      ),
    );
    this.#tui.requestRender();
  }

  #connectionLabel(): string {
    const closed = this.#connection.closed;
    return closed === undefined ? "connected" : `closed: ${closed.reason}`;
  }

  #diagnostics(): DebugDiagnostics {
    const stderr = this.#child.stderrTail();
    const exit = this.#child.exited;
    return {
      attachmentId: this.#session.identity?.attachmentId,
      conversationId: this.#session.identity?.conversationId,
      agentId: this.#session.identity?.agentId,
      cursor: this.#session.state?.cursor,
      connectionState: this.#connectionLabel(),
      childStatus:
        exit === undefined
          ? `running (pid ${this.#child.pid ?? "unknown"})`
          : `exited (code ${exit.code ?? "none"}, signal ${exit.signal ?? "none"})`,
      stderrTail: stderr.text,
      stderrTruncatedBytes: stderr.truncatedBytes,
      pendingRequests: this.#connection.pendingCount,
      resyncCount: this.#session.resyncCount,
    };
  }

  #finish(code: number): void {
    if (this.#finished) {
      return;
    }
    this.#finished = true;
    this.#exitCode = code;
    const resolve = this.#resolveExit;
    this.#resolveExit = undefined;
    this.#loader.stop();
    this.#overlay?.hide();
    if (this.#started) {
      this.#tui.stop();
    }
    if (resolve === undefined) {
      return;
    }
    resolve(code);
  }
}

function editorText(content: SessionSwitch["editorContent"]): string {
  const nonText = (content ?? []).find((block) => block.type !== "text");
  if (nonText !== undefined) {
    throw new Error(
      `fork/tree editor restoration does not support ${nonText.type} content yet`,
    );
  }
  return (content ?? [])
    .map((block) => block.text)
    .join("\n");
}

function sameSessionLineage(expected: SessionView, actual: SessionView): boolean {
  return expected.id === actual.id &&
    expected.active_node === actual.active_node &&
    expected.active_conversation_id === actual.active_conversation_id;
}

function errorMessage(error: unknown): string {
  return error instanceof Error ? error.message : String(error);
}
