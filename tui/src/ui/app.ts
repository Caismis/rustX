/**
 * The rustX terminal application.
 *
 * This is the **outermost** layer, and the only file that imports Pi. Pi
 * supplies terminal mechanics — a differential renderer, a multiline editor
 * with history and autocomplete, Markdown layout, overlays, a spinner. rustX
 * supplies every semantic: what a message means, which model an attempt is
 * on, what a tool is doing, what is still running in the background.
 *
 * ```text
 * PresentationProjection -> selectors/renderers -> rustX components
 *                                                        |
 *                                                        v
 *                                                  Pi primitives
 * ```
 *
 * No Pi class holds authoritative rustX state. Pi components here are
 * disposable render targets rebuilt from the projection, so a fresh
 * `RuntimeClientSnapshot` reconstructs the entire UI without consulting
 * anything Pi remembers. Nothing resembling Pi's `AgentSession`,
 * `SessionManager`, model runtime, provider registry, tool registry, or
 * `InteractiveMode` exists here or anywhere in this package.
 */

import {
  Container,
  Editor,
  Loader,
  Markdown,
  ProcessTerminal,
  SelectList,
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
} from "../commands/dispatcher.ts";
import {
  withNotice,
  withPendingSubmission,
} from "../presentation/projection.ts";
import { workingLabel } from "../presentation/selectors.ts";
import type { PresentationState } from "../presentation/state.ts";
import type { ChildRuntimeProcess } from "../runtime/child-process.ts";
import type { RuntimeClientConnection } from "../runtime/connection.ts";
import type { RuntimeClientSession } from "../runtime/session.ts";
import type { CatalogModelView } from "../protocol/types.ts";
import {
  renderBackgroundSection,
  renderEntry,
  renderFooter,
  renderForegroundTool,
} from "./render.ts";
import { editorTheme, markdownTheme, selectListTheme, style } from "./theme.ts";

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

  #overlay: OverlayHandle | undefined;
  #quitting = false;
  #exitCode = 0;
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
      this.#finish(this.#quitting ? this.#exitCode : 1);
    });
  }

  /** Starts the terminal and resolves with the process exit code. */
  run(): Promise<number> {
    this.#tui.start();
    this.#tui.setFocus(this.#editor);
    const state = this.#session.state;
    if (state !== undefined) {
      this.#renderState(state);
    }
    // Ctrl+C is a cancellation *intent*, routed through the protocol like any
    // other; it never kills the runtime behind the runtime's back.
    this.#tui.addInputListener((data) => {
      if (matchesKey(data, "ctrl+c")) {
        void this.#onInterrupt();
        return { consume: true };
      }
      return undefined;
    });
    return new Promise<number>((resolve) => {
      this.#resolveExit = resolve;
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
        this.#showModelChooser(outcome.models, outcome.rows);
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
   *   -> shutdown          (the active attempt settles under runtime semantics)
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

    try {
      await this.#session.shutdown();
    } catch (error) {
      this.#note("error", `shutdown request failed: ${(error as Error).message}`);
    }

    this.#child.closeStdin();
    const exit = await this.#child.waitOrTerminate(this.#terminationGraceMs);
    this.#exitCode = exit.code ?? 0;
    this.#finish(this.#exitCode);
  }

  #showModelChooser(
    models: CatalogModelView[],
    rows: Array<{ value: string; label: string; description: string }>,
  ): void {
    const list = new SelectList(rows, 12, selectListTheme);
    const handle = this.#tui.showOverlay(list, {
      width: "80%",
      maxHeight: "60%",
      anchor: "center",
    });
    this.#overlay = handle;

    const close = () => {
      handle.hide();
      this.#overlay = undefined;
      this.#tui.setFocus(this.#editor);
      this.#tui.requestRender();
    };

    list.onCancel = close;
    list.onSelect = (item) => {
      close();
      const chosen = models.find((model) => model.model === item.value);
      if (chosen === undefined) {
        return;
      }
      void this.#dispatcher.selectModel(chosen).then((outcome) => {
        if (outcome.kind === "message") {
          this.#note(outcome.level, outcome.text);
        }
      });
    };

    handle.focus();
    this.#tui.requestRender();
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
    this.#transcript.clear();
    for (const entry of state.transcript) {
      const body = renderEntry(entry);
      if (body.length === 0) {
        continue;
      }
      this.#transcript.addChild(new Markdown(body, 1, 0, markdownTheme));
      this.#transcript.addChild(new Spacer(1));
    }

    for (const pending of state.pendingSubmissions) {
      // Marked as unacknowledged so it can never read as canonical history.
      this.#transcript.addChild(
        new Markdown(
          `${style.dim("▌ you (awaiting runtime acknowledgement)")}\n${pending.text}`,
          1,
          0,
          markdownTheme,
        ),
      );
      this.#transcript.addChild(new Spacer(1));
    }

    this.#activity.clear();
    for (const execution of state.attempt?.foreground ?? []) {
      this.#activity.addChild(
        new Markdown(renderForegroundTool(execution), 1, 0, markdownTheme),
      );
    }
    const background = renderBackgroundSection(state);
    if (background.length > 0) {
      this.#activity.addChild(new Text(background, 1, 0));
    }

    const working = workingLabel(state);
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

    this.#footer.setText(renderFooter(state, this.#connectionLabel()));
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
    const resolve = this.#resolveExit;
    if (resolve === undefined) {
      return;
    }
    this.#resolveExit = undefined;
    this.#loader.stop();
    this.#overlay?.hide();
    this.#tui.stop();
    resolve(code);
  }
}
