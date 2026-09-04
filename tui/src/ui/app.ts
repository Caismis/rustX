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
 * foreground tool cards, `ToolExecutionId` for background ones, `InteractionRef`
 * for pending approvals). Those are display choices, they are deliberately not
 * written into runtime state, and losing them on a rebuild costs nothing
 * semantic: every collapsed band is restored from `PresentationState` alone.
 */

import {
  Box,
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
  type SizeValue,
} from "@earendil-works/pi-tui";

import { SlashCommandAutocompleteProvider } from "../commands/autocomplete.ts";
import {
  CommandDispatcher,
  type DebugDiagnostics,
  type ExpandTarget,
  type PreferenceChange,
} from "../commands/dispatcher.ts";
import { sessionLabel } from "../presentation/selectors.ts";
import {
  reconcileInteractionFocus,
  sameInteractionRef,
} from "../presentation/interaction-focus.ts";
import { correlateTools } from "../presentation/tools.ts";
import { selectTodos } from "../presentation/todos.ts";
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
  InteractionRef,
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
  renderSubagentSection,
} from "./components/activity.ts";
import {
  cycleSubagentSelection,
  hasSubagentSelection,
} from "./subagent-navigation.ts";
import { ModelSelector } from "./components/model-selector.ts";
import { InspectionView } from "./components/inspection-view.ts";
import { PopupFrame, type PopupContent } from "./components/popup-frame.ts";
import { TransientFeedbackSurface } from "./components/transient-feedback.ts";
import {
  renderFooter,
  renderStartup,
  startupVisible,
  workingStatus,
} from "./components/status.ts";
import type { ConversationContext } from "./components/status.ts";
import { renderResourceBanner } from "./components/resources.ts";
import { renderTranscript } from "./components/transcript.ts";
import { renderTodoPanel } from "./components/todos.ts";
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
import { background, editorTheme, markdownTheme, style } from "./theme.ts";
import type { TranscriptBlock } from "./components/transcript.ts";
import { HumanInteractionOverlay } from "./components/hitl.ts";
import type { InteractionResponse } from "../protocol/types.ts";

export interface RustxTuiAppOptions {
  session: RuntimeClientAttachment;
  connection: RuntimeClientConnection;
  child: ChildRuntimeProcess;
  /**
   * Open the `/resume` selector as soon as the first attachment is live.
   * This is `--resume`: the picker is presentation over the Session the
   * launch already bound, and the choice becomes the ordinary Session
   * selection Rust publishes.
   */
  openSessionSelector?: boolean;
  /**
   * Whether the initial attachment is a read-only conversation inspection.
   * A child opened from this app sets the same presentation state internally;
   * neither form changes Runtime Client ownership.
   */
  readOnly?: boolean;
  /**
   * The workspace the runtime was launched against.
   *
   * Presentation only: it shortens the absolute paths the runtime publishes
   * for display. The client never reads it, resolves against it, or treats
   * it as a second opinion about where the runtime is running.
   */
  workspace?: string;
  /** Re-spawns and re-attaches after Rust publishes a lineage switch. */
  restartRuntime?: () => Promise<RuntimeAttachmentHandle>;
  /** Opens a known conversation identity in a fresh ordinary attachment. */
  openConversation?: (conversationId: string) => Promise<RuntimeAttachmentHandle>;
  /** How long the child gets to exit after the shutdown sequence. */
  terminationGraceMs?: number;
}

export interface RuntimeAttachmentHandle {
  session: RuntimeClientAttachment;
  connection: RuntimeClientConnection;
  child: ChildRuntimeProcess;
}

/**
 * A lease for one asynchronous client-side presentation continuation.
 *
 * The epoch is advanced whenever authoritative runtime ownership changes. The
 * attachment identity is checked as well, so a late continuation cannot
 * repaint a new attachment that happens to have the same projection shape.
 */
interface PresentationLease {
  epoch: number;
  session: RuntimeClientAttachment;
}

/** One presentation frame kept so Esc can return to the existing parent view. */
interface NavigationFrame {
  handle: RuntimeAttachmentHandle;
  readOnly: boolean;
  parentConversationId?: string;
}

export class RustxTuiApp {
  #session: RuntimeClientAttachment;
  #connection: RuntimeClientConnection;
  #child: ChildRuntimeProcess;
  readonly #dispatcher: CommandDispatcher;
  readonly #restartRuntime: (() => Promise<RuntimeAttachmentHandle>) | undefined;
  readonly #openConversation: ((conversationId: string) => Promise<RuntimeAttachmentHandle>) | undefined;
  readonly #openSessionSelectorAtStartup: boolean;
  readonly #workspace: string | undefined;
  readonly #terminationGraceMs: number | undefined;
  #readOnly: boolean;
  #parentConversationId: string | undefined;
  readonly #navigationStack: NavigationFrame[] = [];

  readonly #tui: TUI;
  readonly #startup = new Container();
  readonly #transcript = new Container();
  readonly #activity = new Container();
  /**
   * The task panel, drawn between the conversation and the editor.
   *
   * It is the plan the reader needs *while typing the next message*, so it
   * sits at the bottom of the scrollback rather than inside the transcript,
   * and it is rebuilt from the projection like every other component here.
   */
  readonly #todos = new Container();
  readonly #transient = new TransientFeedbackSurface();
  readonly #footer = new Text("", 1, 0);
  readonly #editor: Editor;
  readonly #loader: Loader;

  #preferences: PresentationPreferences = defaultPreferences();
  #overlay: OverlayHandle | undefined;
  #hitlOverlay: HumanInteractionOverlay | undefined;
  /**
   * Presentation-only focus over `pendingInteractions`, reconciled against
   * the authoritative projection on every render. Never a semantic fact: it
   * picks which interaction the human-input surface shows, nothing else.
   */
  #interactionFocus: InteractionRef | undefined;
  /**
   * The interaction the user dismissed the human-input surface on (approval
   * Esc). Presentation-only: the interaction stays pending, and the surface
   * reopens when the focus moves on or the user presses Ctrl+G.
   */
  #hitlDismissed: InteractionRef | undefined;
  #quitting = false;
  #exitCode = 0;
  #finished = false;
  #started = false;
  #restarting = false;
  #navigating = false;
  #subagentListFocused = false;
  #selectedSubagentId: string | undefined;
  #presentationEpoch = 0;
  #terminalFinishStarted = false;
  #removeStateListener: (() => void) | undefined;
  #removeSnapshotListener: (() => void) | undefined;
  #removeCloseListener: (() => void) | undefined;
  #resolveExit: ((code: number) => void) | undefined;

  constructor(options: RustxTuiAppOptions) {
    this.#session = options.session;
    this.#connection = options.connection;
    this.#child = options.child;
    this.#restartRuntime = options.restartRuntime;
    this.#openConversation = options.openConversation;
    this.#openSessionSelectorAtStartup = options.openSessionSelector ?? false;
    this.#workspace = options.workspace;
    this.#terminationGraceMs = options.terminationGraceMs;
    this.#readOnly = options.readOnly ?? false;

    this.#tui = new TUI(new ProcessTerminal());
    this.#editor = new Editor(this.#tui, editorTheme, { paddingX: 1 });
    this.#editor.setAutocompleteProvider(new SlashCommandAutocompleteProvider());
    this.#editor.onSubmit = (text) => {
      void this.#onSubmit(text);
    };
    this.#editor.disableSubmit = this.#readOnly;
    this.#loader = new Loader(this.#tui, style.cyan, style.dim, "");

    this.#dispatcher = new CommandDispatcher({
      session: this.#session,
      diagnostics: () => this.#diagnostics(),
    });

    this.#tui.addChild(this.#startup);
    this.#tui.addChild(this.#transcript);
    this.#tui.addChild(this.#activity);
    this.#tui.addChild(this.#transient);
    this.#tui.addChild(this.#todos);
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
    // Binding a new attachment invalidates every local surface and every
    // continuation that was started against the previous one.
    this.#invalidatePresentation();
    this.#removeStateListener?.();
    this.#removeSnapshotListener?.();
    this.#removeCloseListener?.();
    this.#session = session;
    this.#connection = connection;
    this.#child = child;
    this.#dispatcher.setSession(session);
    this.#removeSnapshotListener = session.onSnapshot(() => {
      // A resync is an authoritative replacement within this attachment. It
      // invalidates local inspection, picker, and transient ownership, while
      // the subsequent state publication still renders the new projection.
      if (this.#session !== session || this.#finished) return;
      this.#invalidatePresentation();
    });
    this.#removeStateListener = session.onState((state) => {
      // Runtime state rendering follows attachment identity, not the
      // presentation epoch: the state published immediately after a snapshot
      // replacement is the authoritative state we must render.
      if (this.#session !== session || this.#finished) return;
      this.#renderState(state);
    });
    const boundSession = session;
    this.#removeCloseListener = connection.onClose((error) => {
      if (this.#connection !== connection || this.#session !== boundSession) return;
      if (this.#restarting || this.#quitting || this.#terminalFinishStarted) return;
      if (this.#navigating) return;
      if (this.#navigationStack.length > 0) {
        this.#editor.disableSubmit = true;
        void this.#returnToParent(
          `inspection connection lost: ${compactDiagnostic(error)}`,
        );
        return;
      }
      // Transport loss is not cancellation. It only ends observation, so the
      // client says exactly that and stops accepting input.
      this.#editor.disableSubmit = true;
      void this.#showTerminalFailureAndFinish(
        `${compactDiagnostic(error)}\nThe runtime is no longer observable from this client.`,
        1,
      );
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
      if (!this.#isInspection()) {
        const refreshLease = this.#presentationLease();
        const refreshSession = (
          refreshLease.session as unknown as {
            refreshSession?: () => Promise<unknown>;
          }
        ).refreshSession;
        if (refreshSession !== undefined) {
          void refreshSession.call(refreshLease.session).then(
            () => {
              if (!this.#isCurrentPresentationLease(refreshLease)) return;
              const refreshed = refreshLease.session.state;
              if (refreshed !== undefined) this.#renderState(refreshed);
            },
            (error: unknown) => {
              if (!this.#isCurrentPresentationLease(refreshLease)) return;
              this.#showTransient("error", `session metadata unavailable: ${compactDiagnostic(error)}`);
            },
          );
        }
      }
      // `--resume` is the same selection `/resume` performs, asked for on the
      // command line: the runtime is already attached to the Session the
      // launch bound, and the picker is opened over it. Cancelling therefore
      // leaves that Session bound and publishes nothing.
      if (this.#openSessionSelectorAtStartup) {
        const selectorLease = this.#presentationLease();
        void selectorLease.session
          .listSessions()
          .then((page) => {
            if (!this.#isCurrentPresentationLease(selectorLease)) return;
            this.#showSessionSelector(
              page.sessions,
              page.nextOffset,
              "",
              selectorLease,
            );
          })
          .catch((error: unknown) => {
            if (!this.#isCurrentPresentationLease(selectorLease)) return;
            this.#showTransient(
              "error",
              `session list unavailable: ${compactDiagnostic(error)}`,
            );
          });
      }
      this.#tui.addInputListener((data) => {
        // Any user input acknowledges the one current transient feedback item.
        // A later command or lifecycle result may replace it explicitly.
        this.#acknowledgeTransient();
        // The subagent list is an explicit presentation focus. Ctrl+Up/Down
        // enters that focus so ordinary editor Enter remains ordinary message
        // submission until the user has selected a row.
        if (this.#overlay === undefined && !this.#restarting && !this.#navigating) {
          if (matchesKey(data, "ctrl+up")) {
            this.#moveSubagentSelection(-1);
            return { consume: true };
          }
          if (matchesKey(data, "ctrl+down")) {
            this.#moveSubagentSelection(1);
            return { consume: true };
          }
          if (matchesKey(data, "enter") && this.#subagentListFocused && this.#selectedSubagentId !== undefined) {
            void this.#inspectSelectedSubagent();
            return { consume: true };
          }
        }
        // Ctrl+L is presentation-only input. `/model` remains the canonical
        // semantic command, and its complete CommandOutcome comes back
        // through the one app-level interpreter below.
        if (matchesKey(data, "pageUp")) {
          if (this.#overlay !== undefined) {
            return undefined;
          }
          const lease = this.#presentationLease();
          void lease.session.loadOlderTranscript().then((loaded) => {
            if (!this.#isCurrentPresentationLease(lease) || !loaded) return;
            this.#showTransient("info", "loaded older transcript history");
          }).catch((error: unknown) => {
            if (this.#isCurrentPresentationLease(lease)) {
              this.#showTransient("error", `transcript page failed: ${compactDiagnostic(error)}`);
            }
          });
          return { consume: true };
        }
        if (matchesKey(data, "ctrl+l")) {
          // Do not steal this key from a focused overlay. Pi will deliver it
          // to the overlay, where it is ordinary non-editing input.
          if (this.#overlay !== undefined) {
            return undefined;
          }
          if (this.#isInspection()) {
            return { consume: true };
          }
          const lease = this.#presentationLease();
          void this.#dispatcher
            .submit("/model")
            .then((outcome) => this.#handleOutcome(outcome, lease))
            .catch((error: unknown) => {
              if (this.#isCurrentPresentationLease(lease)) {
                this.#showTransient("error", `model command failed: ${compactDiagnostic(error)}`);
              }
            });
          return { consume: true };
        }
        // Ctrl+G reopens the human-input surface after an approval Esc
        // dismissed it. Presentation-only: it settles nothing and never
        // targets a read-only inspection.
        if (matchesKey(data, "ctrl+g")) {
          if (!this.#isInspection() && this.#hitlOverlay === undefined) {
            this.#hitlDismissed = undefined;
            const state = this.#session.state;
            if (state !== undefined && state.pendingInteractions.length > 0) {
              this.#renderState(state);
            }
          }
          return { consume: true };
        }
        // Ctrl+C is a cancellation *intent*, routed through the protocol like
        // any other; it never kills the runtime behind the runtime's back.
        if (matchesKey(data, "ctrl+c")) {
          void this.#onInterrupt();
          return { consume: true };
        }
        if (matchesKey(data, "escape")) {
          const state = this.#session.state;
          const attempt = state?.attempt;
          const acted = this.#overlay !== undefined || (
            this.#subagentListFocused ||
            this.#navigationStack.length > 0 ||
            !this.#restarting &&
            ((state?.pendingInteractions.length ?? 0) > 0 ||
              (attempt !== undefined && attempt.phase.type !== "settled"))
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
    if (this.#restarting || this.#navigating || this.#finished || this.#isInspection()) return;
    const lease = this.#presentationLease();
    const line = text.trim();
    if (line.length === 0) {
      return;
    }
    this.#editor.addToHistory(text);
    this.#editor.setText("");

    try {
      const outcome = await this.#dispatcher.submit(text);
      if (!this.#isCurrentPresentationLease(lease)) return;
      await this.#handleOutcome(outcome, lease);
    } catch (error: unknown) {
      if (this.#isCurrentPresentationLease(lease)) {
        this.#showTransient("error", `command failed: ${compactDiagnostic(error)}`);
      }
    }
  }

  /** Moves the presentation focus through the authoritative child rows. */
  #moveSubagentSelection(direction: -1 | 1): void {
    const state = this.#session.state;
    if (state === undefined) return;
    const selected = cycleSubagentSelection(
      state.subagents,
      this.#selectedSubagentId,
      direction,
    );
    if (selected === undefined) {
      this.#subagentListFocused = false;
      this.#selectedSubagentId = undefined;
      return;
    }
    this.#subagentListFocused = true;
    this.#selectedSubagentId = selected;
    this.#renderState(state);
  }

  /** Opens the selected row by its canonical child conversation identity. */
  async #inspectSelectedSubagent(): Promise<void> {
    if (
      !this.#subagentListFocused ||
      this.#selectedSubagentId === undefined ||
      this.#finished ||
      this.#navigating ||
      this.#isInspection()
    ) {
      return;
    }
    const openConversation = this.#openConversation;
    const state = this.#session.state;
    const selected = state?.subagents.find(
      (subagent) => subagent.subagent_id === this.#selectedSubagentId,
    );
    if (openConversation === undefined) {
      this.#showTransient("error", "conversation inspection is unavailable from this attachment");
      return;
    }
    if (selected === undefined || selected.child_conversation_id.length === 0) {
      this.#showTransient("error", "the selected subagent has no child conversation identity");
      return;
    }
    const parentConversationId = this.#session.identity?.conversationId;
    if (parentConversationId === undefined) {
      this.#showTransient("error", "the parent conversation identity is not available");
      return;
    }

    const parent: NavigationFrame = {
      handle: {
        session: this.#session,
        connection: this.#connection,
        child: this.#child,
      },
      readOnly: this.#readOnly,
      parentConversationId: this.#parentConversationId,
    };
    this.#navigating = true;
    this.#editor.disableSubmit = true;
    try {
      const next = await openConversation(selected.child_conversation_id);
      this.#navigationStack.push(parent);
      this.#readOnly = true;
      this.#parentConversationId = parentConversationId;
      this.#subagentListFocused = false;
      this.#selectedSubagentId = undefined;
      this.#bindRuntime(next.session, next.connection, next.child);
      this.#editor.disableSubmit = true;
      const nextState = next.session.state;
      if (nextState !== undefined) this.#renderState(nextState);
      this.#showTransient(
        "info",
        `inspecting child conversation ${selected.child_conversation_id} · read-only · Esc returns to parent`,
      );
    } catch (error: unknown) {
      this.#showTransient("error", `conversation inspection failed: ${compactDiagnostic(error)}`);
    } finally {
      this.#navigating = false;
      if (!this.#finished) {
        this.#editor.disableSubmit = this.#isInspection() || this.#quitting;
      }
    }
  }

  /** Detaches only the inspection client and restores the parent frame. */
  async #returnToParent(reason?: string): Promise<void> {
    const frame = this.#navigationStack[this.#navigationStack.length - 1];
    if (frame === undefined || this.#navigating) return;
    const inspected = this.#session;
    const inspectedConnection = this.#connection;
    const inspectedChild = this.#child;
    this.#navigating = true;
    this.#editor.disableSubmit = true;
    this.#subagentListFocused = false;
    this.#selectedSubagentId = undefined;
    let closeFailure: string | undefined;
    try {
      await this.#detachOldAttachment(inspected, inspectedConnection);
      inspectedChild.closeStdin();
      await inspectedChild.waitOrTerminate(this.#terminationGraceMs);
    } catch (error: unknown) {
      closeFailure = compactDiagnostic(error);
    }

    this.#navigationStack.pop();
    this.#readOnly = frame.readOnly;
    this.#parentConversationId = frame.parentConversationId;
    this.#bindRuntime(frame.handle.session, frame.handle.connection, frame.handle.child);
    this.#editor.disableSubmit = this.#isInspection() || this.#quitting;
    const state = this.#session.state;
    if (state !== undefined) this.#renderState(state);
    this.#navigating = false;
    if (this.#finished || this.#quitting) return;
    if (reason !== undefined || closeFailure !== undefined) {
      this.#showTransient(
        "error",
        `${reason ?? "inspection detach failed"}${closeFailure === undefined ? "" : ` · ${closeFailure}`}; returned to parent`,
      );
    } else {
      this.#showTransient(
        "info",
        `returned to parent conversation ${this.#session.identity?.conversationId ?? "unknown"}`,
      );
    }
  }

  async #handleOutcome(
    outcome: Awaited<ReturnType<CommandDispatcher["submit"]>>,
    lease: PresentationLease,
  ): Promise<void> {
    if (!this.#isCurrentPresentationLease(lease)) return;
    switch (outcome.kind) {
      case "inspect":
        this.#showInspection(outcome.title, outcome.body, lease);
        break;
      case "transient":
        this.#showTransient(outcome.level, outcome.text);
        break;
      case "choose_model":
        this.#showModelSelector(outcome.models, lease);
        break;
      case "choose_session":
        this.#showSessionSelector(outcome.sessions, outcome.nextOffset, outcome.query, lease);
        break;
      case "choose_fork":
        this.#showBoundarySelector(
          outcome.boundaries,
          "Fork from user message",
          "fork",
          lease,
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
          lease,
        );
        break;
      case "session_switch":
        await this.#applySessionSwitch(outcome.change, lease);
        break;
      case "replacement_required":
        await this.#applyReplacementRequired(outcome.message, lease);
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
    this.#acknowledgeTransient();
    if (this.#hitlOverlay !== undefined) {
      // Escape behavior is defined per interaction kind by the surface
      // itself: a questionnaire gets its explicit typed decline; an approval
      // is dismissed without any answer. The app never answers here.
      this.#hitlOverlay.escape();
      return;
    }
    if (this.#overlay !== undefined) {
      this.#closeOverlay();
      return;
    }
    if (this.#navigationStack.length > 0) {
      await this.#returnToParent();
      return;
    }
    if (this.#subagentListFocused) {
      this.#subagentListFocused = false;
      this.#selectedSubagentId = undefined;
      const state = this.#session.state;
      if (state !== undefined) this.#renderState(state);
      return;
    }
    if (this.#isInspection()) return;
    if (this.#restarting) return;
    const state = this.#session.state;
    const attempt = state?.attempt;
    if ((state?.pendingInteractions.length ?? 0) > 0 ||
      (attempt !== undefined && attempt.phase.type !== "settled")) {
      const lease = this.#presentationLease();
      try {
        const outcome = await this.#dispatcher.submit("/cancel");
        if (this.#isCurrentPresentationLease(lease)) {
          await this.#handleOutcome(outcome, lease);
        }
      } catch (error: unknown) {
        if (this.#isCurrentPresentationLease(lease)) {
          this.#showTransient("error", `cancellation failed: ${compactDiagnostic(error)}`);
        }
      }
    }
  }

  async #onInterrupt(): Promise<void> {
    this.#acknowledgeTransient();
    if (this.#navigationStack.length > 0) {
      await this.#returnToParent();
      return;
    }
    if (this.#isInspection()) {
      await this.#closeInspection();
      return;
    }
    if (this.#restarting) return;
    const state = this.#session.state;
    if ((state?.pendingInteractions.length ?? 0) > 0 ||
      (state?.attempt !== undefined && state.attempt.phase.type !== "settled")) {
      const lease = this.#presentationLease();
      try {
        const outcome = await this.#dispatcher.submit("/cancel");
        if (this.#isCurrentPresentationLease(lease)) {
          await this.#handleOutcome(outcome, lease);
        }
      } catch (error: unknown) {
        if (this.#isCurrentPresentationLease(lease)) {
          this.#showTransient("error", `cancellation failed: ${compactDiagnostic(error)}`);
        }
      }
      return;
    }
    await this.quit();
  }

  /** Closes a direct read-only inspection without asking it to shut down. */
  async #closeInspection(): Promise<void> {
    if (this.#quitting || this.#finished) return;
    this.#quitting = true;
    this.#invalidatePresentation();
    this.#editor.disableSubmit = true;
    await this.#detachOldAttachment(this.#session, this.#connection);
    this.#child.closeStdin();
    const exit = await this.#child.waitOrTerminate(this.#terminationGraceMs);
    this.#finish(exit.code ?? 1);
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
    while (this.#navigationStack.length > 0) {
      await this.#returnToParent();
    }
    if (this.#quitting) {
      return;
    }
    this.#quitting = true;
    this.#invalidatePresentation();
    this.#editor.disableSubmit = true;
    this.#showTransient("info", "shutting the runtime down…");

    const attempt = this.#session.state?.attempt;
    const unsettledAttemptId =
      attempt !== undefined && attempt.phase.type !== "settled"
        ? attempt.attemptId
        : undefined;
    let lifecycleFailure: string | undefined;
    try {
      await this.#session.shutdown();
    } catch (error) {
      lifecycleFailure = `shutdown request failed: ${compactDiagnostic(error)}`;
    }

    if (lifecycleFailure === undefined && unsettledAttemptId !== undefined) {
      try {
        await this.#session.waitForAttemptSettlement(unsettledAttemptId);
      } catch (error) {
        lifecycleFailure = `attempt settlement was not observed: ${compactDiagnostic(error)}`;
      }
    }

    this.#child.closeStdin();
    const exit = await this.#child.waitOrTerminate(this.#terminationGraceMs);
    this.#exitCode = exit.code ?? 1;
    if (lifecycleFailure !== undefined && this.#exitCode === 0) {
      this.#exitCode = 1;
    }
    if (lifecycleFailure !== undefined) {
      await this.#showTerminalFailureAndFinish(lifecycleFailure, this.#exitCode);
    } else {
      this.#finish(this.#exitCode);
    }
  }

  /** Opens the shared focused surface for substantial read-only information. */
  #showInspection(title: string, body: string, lease: PresentationLease): void {
    if (!this.#isCurrentPresentationLease(lease)) return;
    const inspection = new InspectionView({ title, body });
    const handle = this.#showPopup(inspection, { width: "85%", heightPercent: 70 });
    inspection.onChange = () => {
      if (this.#isCurrentPresentationLease(lease)) this.#tui.requestRender();
    };
    inspection.onClose = () => {
      if (this.#overlay === handle) this.#closeOverlay();
    };
  }

  /**
   * Presents one transient surface inside the shared PopupFrame.
   *
   * The frame owns the popup's geometry — outer rectangle, boundary, title,
   * padding, background, footer — while the wrapped component keeps its
   * feature semantics. The height budget the frame receives is the same
   * percentage declared as pi-tui's `maxHeight`, so pi-tui never has to clip
   * the frame and the bottom boundary always renders. The `visible` hook
   * re-derives the budget on every render cycle, including terminal resizes.
   */
  #showPopup(
    content: PopupContent,
    options: { width: SizeValue; heightPercent: number; minWidth?: number },
  ): OverlayHandle {
    this.#closeOverlay();
    const frame = new PopupFrame(content);
    const handle = this.#tui.showOverlay(frame, {
      width: options.width,
      minWidth: options.minWidth,
      maxHeight: `${options.heightPercent}%`,
      anchor: "center",
      visible: (_width, height) => {
        frame.setViewportHeight(
          Math.max(1, Math.floor((height * options.heightPercent) / 100)),
        );
        return true;
      },
    });
    this.#overlay = handle;
    handle.focus();
    this.#tui.requestRender();
    return handle;
  }

  /**
   * Opens the model selector over the editor.
   *
   * The overlay owns focus while it is up and hands it straight back to the
   * editor on select or cancel, so the editor is never left unfocused.
   */
  #showModelSelector(models: CatalogModelView[], lease: PresentationLease): void {
    if (!this.#isCurrentPresentationLease(lease)) return;
    const state = lease.session.state;
    if (state === undefined) {
      return;
    }
    const selector = new ModelSelector({
      models,
      sessionModel: state.sessionModel,
      attempt: state.attempt,
    });
    const handle = this.#showPopup(selector, { width: "80%", heightPercent: 70 });

    const close = () => {
      if (this.#overlay !== handle) return;
      this.#closeOverlay();
    };

    selector.onChange = () => {
      if (this.#isCurrentPresentationLease(lease)) this.#tui.requestRender();
    };
    selector.onCancel = () => {
      if (this.#isCurrentPresentationLease(lease)) close();
    };
    selector.onSelect = (model) => {
      if (!this.#isCurrentPresentationLease(lease)) return;
      close();
      void this.#dispatcher.selectModel(model)
        .then((outcome) => this.#handleOutcome(outcome, lease))
        .catch((error: unknown) => {
          if (this.#isCurrentPresentationLease(lease)) {
            this.#showTransient("error", `model selection failed: ${compactDiagnostic(error)}`);
          }
        });
    };
  }

  #showSessionSelector(
    sessions: SessionSummaryView[],
    nextOffset: number | undefined,
    query: string,
    lease: PresentationLease,
  ): void {
    if (!this.#isCurrentPresentationLease(lease)) return;
    if (sessions.length === 0) {
      this.#showTransient("info", "no persisted sessions are available");
      return;
    }
    let currentQuery = query;
    let currentNextOffset = nextOffset;
    let requestSerial = 0;
    const selector = new SessionSelector({ sessions, nextOffset, query });
    const handle = this.#showPopup(selector, { width: "80%", heightPercent: 70 });
    selector.onChange = () => {
      if (this.#isCurrentPresentationLease(lease)) this.#tui.requestRender();
    };
    selector.onCancel = () => {
      if (this.#isCurrentPresentationLease(lease) && this.#overlay === handle) {
        this.#closeOverlay();
      }
    };
    selector.onQueryChange = (nextQuery) => {
      currentQuery = nextQuery;
      currentNextOffset = undefined;
      const serial = ++requestSerial;
      void lease.session.listSessions(nextQuery, 0).then((page) => {
        if (!this.#isCurrentPresentationLease(lease) || serial !== requestSerial) return;
        currentNextOffset = page.nextOffset;
        selector.replacePage(page.sessions, page.nextOffset);
      }).catch((error: unknown) => {
        if (!this.#isCurrentPresentationLease(lease) || serial !== requestSerial) return;
        this.#showTransient("error", `session search failed: ${compactDiagnostic(error)}`);
        selector.replacePage([], undefined);
      });
    };
    selector.onLoadMore = () => {
      const offset = currentNextOffset;
      if (offset === undefined) return;
      const serial = requestSerial;
      void lease.session.listSessions(currentQuery, offset).then((page) => {
        if (!this.#isCurrentPresentationLease(lease) || serial !== requestSerial) return;
        currentNextOffset = page.nextOffset;
        selector.appendPage(page.sessions, page.nextOffset);
      }).catch((error: unknown) => {
        if (!this.#isCurrentPresentationLease(lease) || serial !== requestSerial) return;
        this.#showTransient("error", `session page failed: ${compactDiagnostic(error)}`);
        selector.appendPage([], currentNextOffset);
      });
    };
    selector.onSelect = (session) => {
      if (!this.#isCurrentPresentationLease(lease)) return;
      this.#closeOverlay();
      void this.#dispatcher.selectSession(session.id)
        .then((outcome) => this.#handleOutcome(outcome, lease))
        .catch((error: unknown) => {
          if (this.#isCurrentPresentationLease(lease)) {
            this.#showTransient("error", `session selection failed: ${compactDiagnostic(error)}`);
          }
        });
    };
  }

  #showBoundarySelector(
    boundaries: SessionUserMessageBoundaryView[],
    title: string,
    operation: "fork" | "tree",
    lease: PresentationLease,
    nextOffset?: number,
  ): void {
    if (!this.#isCurrentPresentationLease(lease)) return;
    if (boundaries.length === 0) {
      this.#showTransient(
        "info",
        "the active lineage has no committed user-message boundary",
      );
      return;
    }
    let currentNextOffset = nextOffset;
    const selector = new BoundarySelector({ boundaries, title, nextOffset });
    const handle = this.#showPopup(selector, { width: "80%", heightPercent: 70 });
    selector.onChange = () => {
      if (this.#isCurrentPresentationLease(lease)) this.#tui.requestRender();
    };
    selector.onCancel = () => {
      if (this.#isCurrentPresentationLease(lease) && this.#overlay === handle) {
        this.#closeOverlay();
      }
    };
    selector.onLoadMore = () => {
      const offset = currentNextOffset;
      if (offset === undefined) return;
      void lease.session.sessionTreePage(0, offset).then((page) => {
        if (!this.#isCurrentPresentationLease(lease)) return;
        currentNextOffset = page.nextHistoryOffset;
        selector.appendPage(page.branchableMessages, page.nextHistoryOffset);
      }).catch((error: unknown) => {
        if (!this.#isCurrentPresentationLease(lease)) return;
        this.#showTransient("error", `history page failed: ${compactDiagnostic(error)}`);
        selector.appendPage([], currentNextOffset);
      });
    };
    selector.onSelect = (boundary) => {
      if (!this.#isCurrentPresentationLease(lease)) return;
      this.#closeOverlay();
      const request = operation === "fork"
        ? this.#dispatcher.forkAt(boundary)
        : this.#dispatcher.branchAt(boundary);
      void request
        .then((outcome) => this.#handleOutcome(outcome, lease))
        .catch((error: unknown) => {
          if (this.#isCurrentPresentationLease(lease)) {
            this.#showTransient("error", `session switch failed: ${compactDiagnostic(error)}`);
          }
        });
    };
  }

  #showTreeSelector(
    session: SessionView,
    nodes: import("../protocol/types.ts").SessionNodeView[],
    nextNodeOffset: number | undefined,
    boundaries: SessionUserMessageBoundaryView[],
    nextHistoryOffset: number | undefined,
    lease: PresentationLease,
  ): void {
    if (!this.#isCurrentPresentationLease(lease)) return;
    const selector = new TreeSelector({
      session,
      nodes,
      nextNodeOffset,
      boundaries,
      nextHistoryOffset,
    });
    const handle = this.#showPopup(selector, { width: "80%", heightPercent: 70 });
    selector.onChange = () => {
      if (this.#isCurrentPresentationLease(lease)) this.#tui.requestRender();
    };
    selector.onCancel = () => {
      if (this.#isCurrentPresentationLease(lease) && this.#overlay === handle) {
        this.#closeOverlay();
      }
    };
    selector.onLoadMore = () => {
      const request = selector.nextPageRequest();
      if (request === undefined) return;
      void lease.session.sessionTreePage(request.nodeOffset, request.historyOffset).then((page) => {
        if (!this.#isCurrentPresentationLease(lease)) return;
        selector.appendPage({
          nodes: page.nodes,
          nextNodeOffset: page.nextNodeOffset,
          boundaries: page.branchableMessages,
          nextHistoryOffset: page.nextHistoryOffset,
        });
      }).catch((error: unknown) => {
        if (!this.#isCurrentPresentationLease(lease)) return;
        this.#showTransient("error", `tree page failed: ${compactDiagnostic(error)}`);
        selector.retryPage();
      });
    };
    selector.onSelect = (selection: TreeSelection) => {
      if (!this.#isCurrentPresentationLease(lease)) return;
      this.#closeOverlay();
      const request = selection.kind === "node"
        ? this.#dispatcher.selectTreeNode(session.id, selection.node.id)
        : this.#dispatcher.branchAt(selection.boundary);
      void request
        .then((outcome) => this.#handleOutcome(outcome, lease))
        .catch((error: unknown) => {
          if (this.#isCurrentPresentationLease(lease)) {
            this.#showTransient("error", `session switch failed: ${compactDiagnostic(error)}`);
          }
        });
    };
  }

  #closeOverlay(): void {
    const handle = this.#overlay;
    if (handle === undefined) return;
    handle.hide();
    this.#overlay = undefined;
    this.#hitlOverlay = undefined;
    this.#tui.setFocus(this.#editor);
    this.#tui.requestRender();
  }

  async #applySessionSwitch(
    change: SessionSwitch,
    lease: PresentationLease,
  ): Promise<void> {
    if (!this.#isCurrentPresentationLease(lease)) return;

    // Accepting a committed Session transition is itself an ownership
    // boundary. The transition below is allowed to complete its runtime work,
    // but unrelated continuations from the old presentation are stale now.
    this.#invalidatePresentation();
    if (!change.restartRequired) {
      this.#showTransient(
        "info",
        `active session: ${sessionLabel(change.session)} · node ${change.session.active_node}`,
      );
      return;
    }
    const restart = this.#restartRuntime;
    if (restart === undefined) {
      this.#editor.disableSubmit = true;
      await this.#showTerminalFailureAndFinish(
        "the runtime cannot be replaced by this attachment",
        1,
      );
      return;
    }

    this.#restarting = true;
    this.#editor.disableSubmit = true;
    this.#showTransient(
      change.restartDiagnostic === undefined ? "info" : "error",
      change.restartDiagnostic === undefined
        ? `switching to session ${sessionLabel(change.session)}…`
        : `Session ${sessionLabel(change.session)} committed before durability became uncertain: ${compactDiagnostic(change.restartDiagnostic)}`,
    );
    const oldSession = this.#session;
    const oldConnection = this.#connection;
    const oldChild = this.#child;
    let transitionSession = oldSession;
    let refreshLease: PresentationLease | undefined;
    try {
      // Rust has already reached its semantic quiescence point before it
      // returned this result. Closing here only releases the old process
      // attachment; it is never the cancellation operation.
      await this.#detachOldAttachment(oldSession, oldConnection);
      oldChild.closeStdin();
      await oldChild.waitOrTerminate(this.#terminationGraceMs);

      const next = await restart();
      this.#bindRuntime(next.session, next.connection, next.child);
      transitionSession = next.session;
      refreshLease = this.#presentationLease();
      const authoritative = await next.session.refreshSession();
      if (!this.#isCurrentPresentationLease(refreshLease)) return;
      if (change.editorContent !== undefined) {
        if (!sameSessionLineage(change.session, authoritative)) {
          throw new Error(
            "restarted Rust process selected a different Session/node than the committed transition",
          );
        }
        this.#editor.setText(editorText(change.editorContent));
      }
      this.#showTransient(
        "info",
        `active session: ${sessionLabel(authoritative)} · node ${authoritative.active_node}`,
      );
      const state = refreshLease.session.state;
      if (state !== undefined) this.#renderState(state);
    } catch (error) {
      // A refresh continuation that lost its lease must not turn a newer
      // attachment's UI into a failure surface. Failures before the new
      // attachment is bound still belong to this accepted transition.
      if (
        (refreshLease !== undefined && !this.#isCurrentPresentationLease(refreshLease)) ||
        this.#session !== transitionSession ||
        this.#finished
      ) {
        return;
      }
      this.#editor.disableSubmit = true;
      await this.#showTerminalFailureAndFinish(
        `session switch failed: ${compactDiagnostic(error)}`,
        1,
      );
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
  async #applyReplacementRequired(
    message: string,
    lease: PresentationLease,
  ): Promise<void> {
    if (!this.#isCurrentPresentationLease(lease) || this.#restarting) return;
    this.#invalidatePresentation();
    const restart = this.#restartRuntime;
    this.#restarting = true;
    this.#editor.disableSubmit = true;
    this.#showTransient(
      "error",
      `the active Session attachment must be replaced: ${compactDiagnostic(message)}`,
    );
    if (restart === undefined) {
      await this.#showTerminalFailureAndFinish(
        `the active Session attachment must be replaced: ${compactDiagnostic(message)}`,
        1,
      );
      this.#restarting = false;
      return;
    }

    const oldSession = this.#session;
    const oldConnection = this.#connection;
    const oldChild = this.#child;
    let transitionSession = oldSession;
    let refreshLease: PresentationLease | undefined;
    try {
      await this.#detachOldAttachment(oldSession, oldConnection);
      oldChild.closeStdin();
      await oldChild.waitOrTerminate(this.#terminationGraceMs);

      const next = await restart();
      this.#bindRuntime(next.session, next.connection, next.child);
      transitionSession = next.session;
      refreshLease = this.#presentationLease();
      const authoritative = await next.session.refreshSession();
      if (!this.#isCurrentPresentationLease(refreshLease)) return;
      this.#showTransient(
        "info",
        `attached to authoritative Session ${sessionLabel(authoritative)} · node ${authoritative.active_node}`,
      );
      const state = refreshLease.session.state;
      if (state !== undefined) this.#renderState(state);
    } catch (error) {
      if (
        (refreshLease !== undefined && !this.#isCurrentPresentationLease(refreshLease)) ||
        this.#session !== transitionSession ||
        this.#finished
      ) {
        return;
      }
      this.#editor.disableSubmit = true;
      await this.#showTerminalFailureAndFinish(
        `Session attachment replacement failed: ${compactDiagnostic(error)}`,
        1,
      );
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
          change.interaction,
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
   * `ToolExecutionId`, and each pending interaction keyed by `InteractionRef`.
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
        (interaction) => interaction.interaction,
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

  #presentationLease(): PresentationLease {
    return {
      epoch: this.#presentationEpoch,
      session: this.#session,
    };
  }

  /** Whether the current attachment is a read-only conversation inspection. */
  #isInspection(): boolean {
    return this.#readOnly || this.#navigationStack.length > 0;
  }

  #isCurrentPresentationLease(lease: PresentationLease): boolean {
    return !this.#finished &&
      !this.#quitting &&
      !this.#terminalFinishStarted &&
      lease.epoch === this.#presentationEpoch &&
      lease.session === this.#session;
  }

  /** Invalidates attachment-local presentation work at one central boundary. */
  #invalidatePresentation(): void {
    this.#presentationEpoch += 1;
    this.#resetLocalSurfaces();
  }

  /**
   * Commits a fatal diagnostic before stopping Pi.
   *
   * pi-tui 0.82.1 schedules both normal and forced renders on
   * `process.nextTick`, while `stop()` marks the TUI stopped and cancels the
   * pending render timer. Awaiting one next-tick barrier after the forced
   * render therefore establishes the presentation commit point without a
   * timing delay.
   */
  async #showTerminalFailureAndFinish(message: string, code: number): Promise<void> {
    if (this.#finished || this.#terminalFinishStarted) return;
    this.#terminalFinishStarted = true;
    this.#editor.disableSubmit = true;
    this.#invalidatePresentation();
    this.#transient.replace({ level: "error", text: message });
    if (this.#started) {
      this.#tui.requestRender(true);
      await nextTick();
    } else {
      // There is no Pi-owned terminal frame before run() starts. Preserve the
      // diagnostic for startup failures on stderr, then settle the app.
      process.stderr.write(`${message}\n`);
    }
    this.#finish(code);
  }

  #showTransient(level: "info" | "error", text: string): void {
    if (this.#finished || this.#terminalFinishStarted) return;
    this.#transient.replace({ level, text });
    this.#tui.requestRender();
  }

  #acknowledgeTransient(): void {
    if (this.#transient.feedback === undefined) {
      return;
    }
    this.#transient.acknowledge();
    this.#tui.requestRender();
  }

  #resetLocalSurfaces(): void {
    this.#closeOverlay();
    // An authoritative replacement re-derives presentation focus from the
    // new projection: no stale overlay or dismissed marker may submit
    // against, or hide, an interaction the runtime owns now.
    this.#interactionFocus = undefined;
    this.#hitlDismissed = undefined;
    this.#transient.clear();
    if (this.#started) {
      this.#tui.requestRender();
    }
  }

  /**
   * Rebuilds the visible components from the projection.
   *
   * The rebuild is total: every component is discarded and reconstructed from
   * `state`. That is what makes the UI reconstructable from a fresh snapshot —
   * no Pi component carries state the projection does not have.
   */
  #renderState(state: PresentationState): void {
    if (!hasSubagentSelection(state.subagents, this.#selectedSubagentId)) {
      this.#selectedSubagentId = undefined;
      this.#subagentListFocused = false;
    }
    // Reconcile the presentation-only interaction focus with the
    // authoritative pending set before anything renders it: settling the
    // focused interaction advances focus deterministically, and an empty
    // queue drops it.
    this.#interactionFocus = reconcileInteractionFocus(
      state.pendingInteractions,
      this.#interactionFocus,
    );
    // Correlated once per render and shared: the transcript and the activity
    // area must agree on which calls have a transcript anchor.
    const correlation = correlateTools(state);

    // The welcome block is useful only before the first real turn. Session
    // metadata is refreshed from the native Session projection and is never
    // reconstructed from client or attachment identifiers.
    this.#startup.clear();
    if (startupVisible(state)) {
      this.#startup.addChild(
        new Text(
          renderStartup(
            state,
            this.#session.sessionInfo,
            this.#tui.terminal.columns,
          ),
          1,
          0,
        ),
      );
      // What the runtime is actually running with — its project context
      // files, its Skill catalog, its active Tools — stated once, before the
      // first turn, entirely from the capability and resource projections.
      const resources = renderResourceBanner(state, {
        workspace: this.#workspace,
      });
      if (resources.length > 0) {
        this.#startup.addChild(new Spacer(1));
        this.#startup.addChild(new Text(resources, 1, 0));
      }
    }

    this.#transcript.clear();
    for (const block of renderTranscript(state, this.#preferences, correlation)) {
      this.#transcript.addChild(banded(block));
      this.#transcript.addChild(new Spacer(1));
    }

    // The activity area holds only what is *not* conversation content. A
    // foreground tool call renders inside the assistant message that asked
    // for it, which is what keeps one call to one card.
    this.#activity.clear();
    for (const section of [
      renderOrphanExecutions(correlation, this.#preferences),
      renderBackgroundSection(state, this.#preferences),
      renderSubagentSection(
        state,
        this.#preferences,
        new Date(),
        this.#subagentListFocused ? this.#selectedSubagentId : undefined,
      ),
      renderInteractionSection(state, this.#preferences, this.#interactionFocus),
    ]) {
      if (section.length > 0) {
        this.#activity.addChild(new Text(section, 1, 0));
      }
    }

    // The plan, derived from the same transcript the conversation is drawn
    // from. An empty panel draws nothing at all.
    this.#todos.clear();
    const todos = renderTodoPanel(selectTodos(state), {
      columns: this.#tui.terminal.columns,
    });
    if (todos.length > 0) {
      this.#todos.addChild(new Text(todos, 1, 0));
    }

    const working = workingStatus(state);
    if (working === undefined) {
      this.#loader.stop();
    } else {
      this.#loader.setMessage(working);
      this.#loader.start();
      this.#activity.addChild(this.#loader);
    }

    this.#footer.setText(
      renderFooter(
        state,
        this.#connectionLabel(),
        this.#tui.terminal.columns,
        this.#session.sessionInfo,
        this.#conversationContext(),
      ),
    );
    this.#syncHitlOverlay(state);
    this.#tui.requestRender();
  }

  /**
   * Presents the unified human-input surface from authoritative state.
   *
   * One surface serves every pending routed interaction — approvals and
   * questionnaires, primary and subagent. It opens when the projection holds
   * a focused interaction the user has not dismissed, updates in place while
   * the projection evolves, and disappears when the interaction set empties
   * or the surface is dismissed. The runtime remains the only owner: closing
   * or dismissing this surface settles nothing, and a response always names
   * the exact `InteractionRef` it was collected for.
   */
  #syncHitlOverlay(state: PresentationState): void {
    const focused = this.#interactionFocus;
    // A read-only inspection is never an answer surface: it may show that a
    // child waits for human input, but the controls live only at the root.
    if (this.#isInspection() || focused === undefined) {
      if (this.#hitlOverlay !== undefined) this.#closeOverlay();
      return;
    }
    const existing = this.#hitlOverlay;
    if (existing !== undefined) {
      existing.update(state.pendingInteractions, focused, this.#preferences);
      this.#tui.requestRender();
      return;
    }
    if (
      this.#hitlDismissed !== undefined &&
      sameInteractionRef(this.#hitlDismissed, focused)
    ) {
      return;
    }
    const lease = this.#presentationLease();
    const overlay = new HumanInteractionOverlay({
      onDecision: (interaction, decision) =>
        this.#respondToInteraction(lease, overlay, interaction, {
          type: "approval",
          decision,
        }),
      onQuestionnaireSubmit: (interaction, response) =>
        this.#respondToInteraction(lease, overlay, interaction, {
          type: "questionnaire",
          response,
        }),
      onQuestionnaireDecline: (interaction) =>
        this.#respondToInteraction(lease, overlay, interaction, {
          type: "questionnaire",
          response: { type: "declined" },
        }),
      onDismiss: (interaction) => {
        this.#hitlDismissed = interaction;
        if (this.#hitlOverlay === overlay) this.#closeOverlay();
      },
      onInterrupt: () => void this.#onInterrupt(),
      onNavigate: (interaction) => {
        // Navigation is presentation-only: it moves the focus and redraws.
        this.#interactionFocus = interaction;
        const current = this.#session.state;
        if (current !== undefined) this.#renderState(current);
      },
      onToggleExpand: (interaction) => {
        // Disclosure only: the same preference domain `/expand interaction`
        // uses, never a second approval gate.
        this.#applyPreference({ type: "expand_interaction", interaction });
      },
      onChange: () => this.#tui.requestRender(),
    });
    overlay.update(state.pendingInteractions, focused, this.#preferences);
    this.#showPopup(overlay, { width: "94%", minWidth: 44, heightPercent: 90 });
    this.#hitlOverlay = overlay;
  }

  /**
   * The one typed response path for every interaction kind.
   *
   * Approval decisions and questionnaire responses both go through
   * `interaction_respond` with the exact routed identity the surface
   * collected them for. A rejection re-enables the panel that sent it —
   * routed by that same identity — so a failed response never double-sends
   * and never disturbs an unrelated pending interaction.
   */
  #respondToInteraction(
    lease: PresentationLease,
    overlay: HumanInteractionOverlay,
    interaction: InteractionRef,
    response: InteractionResponse,
  ): void {
    void lease.session
      .respondInteraction(interaction, response)
      .catch((error: unknown) => {
        if (!this.#isCurrentPresentationLease(lease)) return;
        if (this.#hitlOverlay === overlay) {
          overlay.submissionFailed(interaction);
        }
        this.#showTransient("error", `interaction response failed: ${compactDiagnostic(error)}`);
      });
  }

  #connectionLabel(): string {
    const closed = this.#connection.closed;
    return closed === undefined ? "connected" : `closed: ${closed.reason}`;
  }

  /** Builds the footer's current-conversation label from attachment identity. */
  #conversationContext(): ConversationContext | undefined {
    const conversationId = this.#session.identity?.conversationId;
    if (conversationId === undefined) return undefined;
    return {
      conversationId,
      parentConversationId: this.#parentConversationId,
      readOnly: this.#readOnly,
    };
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

function compactDiagnostic(error: unknown): string {
  return errorMessage(error).replace(/\s*\r?\n\s*/g, " · ").trim();
}

function nextTick(): Promise<void> {
  return new Promise((resolve) => process.nextTick(resolve));
}

/**
 * Lays one transcript block out, on its background band when it has one.
 *
 * The band is the app's job because the app is the only layer that knows how
 * wide the terminal is: a background has to be filled to the edge of the
 * line, and a component that composed one into its own string would paint a
 * ragged block whose colour stopped at its longest line. Pi's `Box` does the
 * filling; everything above only names the band.
 *
 * A banded block owns its horizontal padding through the box, so its inner
 * component takes none — otherwise the padding would be applied twice and the
 * band would sit one column further in than the content it frames.
 */
function banded(block: TranscriptBlock): Container | Box | Text | Markdown {
  const pad = block.background === undefined ? 1 : 0;
  const content =
    block.kind === "markdown"
      ? new Markdown(
          block.markdown,
          pad,
          0,
          markdownTheme,
          block.defaultTextStyle,
        )
      : new Text(block.text, pad, 0);
  if (block.background === undefined) {
    return content;
  }
  const box = new Box(1, 1, background[block.background]);
  box.addChild(content);
  return box;
}
