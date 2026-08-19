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
 * {@link PresentationPreferences} — reasoning visibility and which tool cards
 * are expanded. Those are display choices, they are deliberately not written
 * into runtime state, and losing them on a rebuild costs nothing semantic.
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
import type { RuntimeClientSession } from "../runtime/session.ts";
import type { CatalogModelView, ToolCallId } from "../protocol/types.ts";
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
  withExpandedCalls,
  withReasoningVisible,
  withToggledCall,
} from "./preferences.ts";
import { editorTheme, markdownTheme, style } from "./theme.ts";

export interface RustxTuiAppOptions {
  session: RuntimeClientSession;
  connection: RuntimeClientConnection;
  child: ChildRuntimeProcess;
  /** How long the child gets to exit after the shutdown sequence. */
  terminationGraceMs?: number;
}

export class RustxTuiApp {
  readonly #session: RuntimeClientSession;
  readonly #connection: RuntimeClientConnection;
  readonly #child: ChildRuntimeProcess;
  readonly #dispatcher: CommandDispatcher;
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
  #resolveExit: ((code: number) => void) | undefined;

  constructor(options: RustxTuiAppOptions) {
    this.#session = options.session;
    this.#connection = options.connection;
    this.#child = options.child;
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

    this.#session.onState((state) => this.#renderState(state));
    this.#connection.onClose((error) => {
      // Transport loss is not cancellation. It only ends observation, so the
      // client says exactly that and stops accepting input.
      this.#note(
        "error",
        `${error.message}\nThe runtime is no longer observable from this client.`,
      );
      this.#editor.disableSubmit = true;
      if (!this.#quitting) {
        this.#finish(1);
      }
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
      this.#tui.addInputListener((data) => {
        // Ctrl+C is a cancellation *intent*, routed through the protocol like
        // any other; it never kills the runtime behind the runtime's back.
        if (matchesKey(data, "ctrl+c")) {
          void this.#onInterrupt();
          return { consume: true };
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

    const outcome = await this.#dispatcher.submit(text);
    switch (outcome.kind) {
      case "message":
        this.#note(outcome.level, outcome.text);
        break;
      case "choose_model":
        this.#showModelSelector(outcome.models);
        break;
      case "preference":
        this.#applyPreference(outcome.preference);
        break;
      case "quit":
        await this.quit();
        break;
      default:
        break;
    }
  }

  async #onInterrupt(): Promise<void> {
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
      void this.#dispatcher.selectModel(model).then((outcome) => {
        if (outcome.kind === "message") {
          this.#note(outcome.level, outcome.text);
        }
      });
    };

    handle.focus();
    this.#tui.requestRender();
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
        this.#preferences = withToggledCall(this.#preferences, change.callId);
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

  #expandTarget(target: "all" | "none" | "latest"): PresentationPreferences {
    const state = this.#session.state;
    if (target === "none" || state === undefined) {
      return withAllCollapsed(this.#preferences);
    }
    const calls = [...correlateTools(state).byCallId.keys()];
    if (target === "all") {
      return withExpandedCalls(this.#preferences, calls);
    }
    // "latest" is the most recently correlated call, which is the one a user
    // pressing ctrl+o is looking at. Correlation order follows the transcript,
    // never screen position.
    const latest: ToolCallId | undefined = calls[calls.length - 1];
    return latest === undefined
      ? this.#preferences
      : withToggledCall(this.#preferences, latest);
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
      renderInteractionSection(state),
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
