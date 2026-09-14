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
  findPendingInteraction,
  reconcileInteractionFocus,
  sameInteractionRef,
} from "../presentation/interaction-focus.ts";
import { correlateTools } from "../presentation/tools.ts";
import { selectTodos } from "../presentation/todos.ts";
import type { PresentationState } from "../presentation/state.ts";
import { AppServerRequestError } from "../app-server/client.ts";
import type { AppServerHost } from "../app-server/host.ts";
import type { AppServerSession } from "../app-server/session.ts";
import type {
  CatalogModelView,
  SessionSettings,
  SessionSummaryView,
  SessionUserMessageBoundaryView,
  SessionView,
  ToolCallId,
  InteractionRef,
  UserContentBlock,
} from "../protocol/app-server.ts";
import {
  BoundarySelector,
} from "./components/session-selector.ts";
import { TreeSelector, type TreeSelection } from "./components/tree-selector.ts";
import {
  renderBackgroundSection,
  renderInteractionSection,
  renderOrphanExecutions,
  renderSubagentDetail,
  renderSubagentSection,
} from "./components/activity.ts";
import {
  cycleSubagentSelection,
  hasSubagentSelection,
} from "./subagent-navigation.ts";
import { ModelSelector } from "./components/model-selector.ts";
import { InspectionView } from "./components/inspection-view.ts";
import { SessionDeletionWorkflow } from "./session-deletion-workflow.ts";
import { ResumeSelector } from "./components/resume-selector.ts";
import { ApprovalSelector } from "./components/approval-selector.ts";
import { approvalLabel } from "../presentation/selectors.ts";
import { ConfirmationView } from "./components/confirmation.ts";
import { PopupFrame, type PopupContent } from "./components/popup-frame.ts";
import { TransientFeedbackSurface } from "./components/transient-feedback.ts";
import {
  FooterView,
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
  interactionKey,
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
import type { InteractionResponse } from "../protocol/app-server.ts";

export interface RustxTuiAppOptions {
  /** The App Server connection, its Session catalog, and who owns its process. */
  host: AppServerHost;
  /** The Session the terminal opens on. */
  session: AppServerSession;
  /** `session/create` inputs for Sessions this client creates. */
  sessionSettings: SessionSettings;
  /**
   * Open the `/resume` selector as soon as the first Session is visible.
   *
   * This is `--resume`: the picker is presentation over the Session already in
   * focus, and choosing another simply moves focus.
   */
  openSessionSelector?: boolean;
  /**
   * The directory Sessions this launch creates are rooted at.
   *
   * Presentation only: it shortens the absolute paths the runtime publishes
   * for display. The client never reads it, resolves against it, or treats it
   * as a second opinion about where anything is running.
   */
  cwd?: string;
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
  sessionListGeneration: number;
  session: AppServerSession;
}

/** One submitted approval operation; object identity is its completion token. */
interface PendingApprovalRequest {
  readonly owner: PresentationLease;
}

export class RustxTuiApp {
  readonly #host: AppServerHost;
  #session: AppServerSession;
  readonly #dispatcher: CommandDispatcher;
  readonly #openSessionSelectorAtStartup: boolean;
  readonly #workspace: string | undefined;

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
  readonly #footer = new FooterView(() => ({
    state: this.#session.state,
    connection: this.#connectionLabel(),
    session: this.#sessionInfo,
    conversation: this.#conversationContext(),
  }));
  readonly #editor: Editor;
  readonly #loader: Loader;

  #preferences: PresentationPreferences = defaultPreferences();
  #overlay: OverlayHandle | undefined;
  #pendingApprovalRequest: PendingApprovalRequest | undefined;
  #hitlOverlay: HumanInteractionOverlay | undefined;
  /**
   * Presentation-only focus over `pendingInteractions`, reconciled against
   * the authoritative projection on every render. Never a semantic fact: it
   * picks which interaction the human-input surface shows, nothing else.
   */
  #interactionFocus: InteractionRef | undefined;
  /**
   * The interaction the user dismissed the human-input surface on (approval
   * Esc), plus the pending queue exactly as it stood at dismissal.
   *
   * Presentation-only: the dismissed interaction stays pending and
   * unanswered, and the surface reopens on Ctrl+G. The dismissal is scoped
   * to that exact interaction and that exact queue: the moment a *distinct*
   * interaction arrives, the dismissal is superseded — a previously
   * dismissed approval must never silently hide newly required human input.
   * Settlement or removal of the dismissed interaction retires the marker.
   */
  #hitlDismissed:
    | { interaction: InteractionRef; known: ReadonlySet<string> }
    | undefined;
  #quitting = false;
  #exitCode = 0;
  #finished = false;
  #started = false;
  /** True only while a focus change is installing a different Session. */
  #switching = false;
  #subagentListFocused = false;
  #selectedSubagentId: string | undefined;
  #presentationEpoch = 0;
  #deletion!: SessionDeletionWorkflow;
  #resumePresentation: ResumeSelector | undefined;
  #resumeQuery: string | undefined;
  #terminalFinishStarted = false;
  #removeStateListener: (() => void) | undefined;
  #removeSnapshotListener: (() => void) | undefined;
  #removeClosedListener: (() => void) | undefined;
  readonly #removeConnectionListener: () => void;
  /** The durable metadata of the visible Session, refreshed authoritatively. */
  #sessionInfo: SessionView | undefined;
  #resolveExit: ((code: number) => void) | undefined;

  constructor(options: RustxTuiAppOptions) {
    this.#host = options.host;
    this.#session = options.session;
    this.#openSessionSelectorAtStartup = options.openSessionSelector ?? false;
    this.#workspace = options.cwd;

    this.#tui = new TUI(new ProcessTerminal());
    this.#editor = new Editor(this.#tui, editorTheme, { paddingX: 1 });
    this.#editor.setAutocompleteProvider(new SlashCommandAutocompleteProvider());
    this.#editor.onSubmit = (text) => {
      void this.#onSubmit(text);
    };
    this.#loader = new Loader(this.#tui, style.cyan, style.dim, "");

    this.#dispatcher = new CommandDispatcher({
      host: this.#host,
      session: this.#session,
      sessionSettings: options.sessionSettings,
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

    // The connection is the one thing every Session shares. Losing it ends
    // observation of all of them at once — and says nothing about any of them.
    this.#removeConnectionListener = this.#host.client.onClose((error) => {
      this.#deletion?.terminate();
      if (this.#quitting || this.#terminalFinishStarted || this.#finished) return;
      this.#editor.disableSubmit = true;
      void this.#showTerminalFailureAndFinish(
        `${compactDiagnostic(error)}\nThe App Server is no longer reachable from this client. Work it already accepted is unaffected; this client can no longer observe it.`,
        1,
      );
    });
    this.#bindSession(this.#session);
  }

  /**
   * Binds the Session the terminal is showing.
   *
   * This is a **focus change and nothing else**. The Session being replaced on
   * screen keeps its attachment, keeps its subscription, and keeps executing in
   * the same App Server process. Nothing here detaches it, cancels a turn,
   * answers an interaction, unloads a runtime, or replaces a process.
   *
   * What it does invalidate is local presentation: overlays, transient
   * feedback and in-flight continuations belong to the Session that started
   * them, so they are dropped rather than repainted over a different one.
   */
  #bindSession(session: AppServerSession): void {
    this.#deletion?.terminate();
    this.#invalidatePresentation();
    this.#removeStateListener?.();
    this.#removeSnapshotListener?.();
    this.#removeClosedListener?.();
    this.#session = session;
    this.#sessionInfo = undefined;
    this.#dispatcher.setSession(session);
    const deletion = new SessionDeletionWorkflow(
      this.#host,
      () =>
        this.#session === session &&
        this.#host.client.closed === undefined &&
        !this.#finished &&
        !this.#terminalFinishStarted,
      (text) => this.#showTransient("info", text),
    );
    this.#deletion = deletion;
    this.#resumeQuery = undefined;
    deletion.subscribe(() => {
      if (this.#deletion !== deletion) return;
      this.#syncDeletionPresentation();
      this.#tui.requestRender();
    });
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
    this.#removeClosedListener = session.onClosed(() => {
      // The server retired this attachment's residency. That is a statement
      // about observability, not a runtime outcome, and it is reported as one.
      if (this.#session !== session || this.#finished) return;
      this.#showTransient(
        "error",
        "this Session's runtime was unloaded; reopen it with /resume to attach again",
      );
      this.#editor.disableSubmit = true;
    });
    this.#refreshSessionInfo();
  }

  /**
   * Moves the terminal's focus to a different Session.
   *
   * Attach is idempotent per Session on one connection, so returning to a
   * Session reuses the attachment it already has — and its projection is
   * repaired from authoritative state rather than from whatever this client
   * last believed.
   */
  async #focusSession(
    sessionId: string,
    nodeId: string | undefined,
    editorContent: UserContentBlock[] | undefined,
    notice: string | undefined,
    lease: PresentationLease,
  ): Promise<void> {
    if (!this.#isCurrentPresentationLease(lease) || this.#switching) return;
    if (sessionId === this.#session.sessionId && editorContent === undefined) {
      // Already the visible Session. Repair rather than re-attach, so the
      // picker's "choose the current one" is still an authoritative refresh.
      if (notice !== undefined) this.#showTransient("info", notice);
      return;
    }
    this.#switching = true;
    this.#editor.disableSubmit = true;
    try {
      const next = await this.#host.attach(sessionId, nodeId);
      this.#bindSession(next);
      // A returning attachment may have folded events while it was off screen,
      // and the server is the only thing entitled to say what it holds now.
      if (next.resyncCount === 0 && this.#session === next) {
        await next.resync();
      }
      if (this.#session !== next) return;
      if (editorContent !== undefined) {
        this.#editor.setText(editorText(editorContent));
      }
      this.#showTransient("info", notice ?? `showing session ${sessionId}`);
      this.#renderState(next.state);
    } catch (error: unknown) {
      if (this.#finished) return;
      this.#showTransient(
        "error",
        `could not open that Session: ${compactDiagnostic(error)}`,
      );
    } finally {
      this.#switching = false;
      if (!this.#finished) {
        this.#editor.disableSubmit = this.#quitting;
      }
    }
  }

  /** Reads the visible Session's durable metadata for the footer. */
  #refreshSessionInfo(): void {
    const session = this.#session;
    void this.#host.readSession(session.sessionId).then(
      (info) => {
        if (this.#session !== session || this.#finished) return;
        this.#sessionInfo = info;
        this.#renderState(session.state);
      },
      (error: unknown) => {
        if (this.#session !== session || this.#finished) return;
        this.#showTransient(
          "error",
          `session metadata unavailable: ${compactDiagnostic(error)}`,
        );
      },
    );
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
      // `--resume` is the same selection `/resume` performs, asked for on the
      // command line: the runtime is already attached to the Session the
      // launch bound, and the picker is opened over it. Cancelling therefore
      // leaves that Session bound and publishes nothing.
      if (this.#openSessionSelectorAtStartup) {
        const selectorLease = this.#presentationLease();
        void this.#host
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
        if (this.#overlay === undefined && !this.#switching) {
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
          if (matchesKey(data, "d") && this.#subagentListFocused && this.#selectedSubagentId !== undefined) {
            this.#confirmDisposeSelectedSubagent();
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
          if (this.#hitlOverlay === undefined) {
            this.#hitlDismissed = undefined;
            const state = this.#session.state;
            if (state.pendingInteractions.length > 0) {
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
          // Focused popup components own cancellation and nested presentation state.
          if (this.#overlay !== undefined && this.#hitlOverlay === undefined) return undefined;
          const state = this.#session.state;
          const attempt = state?.attempt;
          const acted = this.#overlay !== undefined || (
            this.#subagentListFocused ||
            !this.#switching &&
            (state.pendingInteractions.length > 0 ||
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
    if (this.#switching || this.#finished) return;
    const lease = this.#presentationLease();
    const line = text.trim();
    if (line.length === 0) {
      return;
    }
    this.#editor.addToHistory(text);
    this.#editor.setText("");

    try {
      // Reopening a retained deletion workflow queries its actual native search,
      // rather than relabeling an unfiltered page with the preserved query.
      const query = this.#resumeQuery ?? this.#deletion.context.query;
      const outcome = line === "/resume" && this.#deletion.state.kind !== "idle"
        ? { kind: "choose_session" as const, ...await this.#host.listSessions(query, 0), query }
        : await this.#dispatcher.submit(text);
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

  /**
   * Opens an authoritative detail view of the selected subagent.
   *
   * The App Server projects a child's identity, state, activity, execution
   * profile and workspace; it exposes no method for attaching to a child
   * conversation, so this reads `subagent/status` rather than composing a
   * second conversation client of its own.
   */
  async #inspectSelectedSubagent(): Promise<void> {
    if (
      !this.#subagentListFocused ||
      this.#selectedSubagentId === undefined ||
      this.#finished
    ) {
      return;
    }
    const lease = this.#presentationLease();
    const subagentId = this.#selectedSubagentId;
    try {
      const subagent = await lease.session.subagentStatus(subagentId);
      if (!this.#isCurrentPresentationLease(lease)) return;
      this.#showInspection(
        `Subagent ${subagent.agent}`,
        renderSubagentDetail(subagent),
        lease,
      );
    } catch (error: unknown) {
      if (!this.#isCurrentPresentationLease(lease)) return;
      this.#showTransient(
        "error",
        `subagent status unavailable: ${compactDiagnostic(error)}`,
      );
    }
  }

  /** Asks for confirmation before disposing the selected runtime-owned workspace. */
  #confirmDisposeSelectedSubagent(): void {
    if (
      !this.#subagentListFocused ||
      this.#selectedSubagentId === undefined ||
      this.#finished
    ) {
      return;
    }
    const state = this.#session.state;
    const selected = state.subagents.find(
      (subagent) => subagent.subagent_id === this.#selectedSubagentId,
    );
    if (selected === undefined) {
      this.#showTransient("error", "the selected subagent is no longer known to the runtime");
      return;
    }
    const resourceState = selected.workspace?.resource_state;
    const disposalRetryable =
      resourceState === "preserved_unresolved" ||
      resourceState === "disposal_in_progress" ||
      resourceState === "worktree_removed";
    if (selected.workspace?.handoff == null && !disposalRetryable) {
      this.#showTransient("info", "the selected subagent has no retained workspace");
      return;
    }

    const subagentId = selected.subagent_id;
    const lease = this.#presentationLease();
    const confirmation = new ConfirmationView({
      title: "Dispose retained workspace",
      confirmLabel: "Dispose workspace",
      subject: `Subagent ${subagentId}`,
      warning: resourceState === "preserved_unresolved"
        ? "This workspace was preserved because physical settlement could not be proven. rustX will re-check ownership before attempting disposal."
        : disposalRetryable
        ? "This resumes the runtime-authorized disposal of the selected subagent workspace."
        : "This permanently removes the retained Git worktree and discards its uncommitted source changes.",
      onConfirm: () => {
        this.#closeOverlay();
        void this.#disposeSelectedSubagent(subagentId, lease);
      },
      onCancel: () => this.#closeOverlay(),
    });
    this.#showPopup(confirmation, { width: "80%", minWidth: 48, heightPercent: 38 });
  }

  /** Routes the confirmed operation through the App Server boundary. */
  async #disposeSelectedSubagent(
    subagentId: string,
    lease: PresentationLease,
  ): Promise<void> {
    try {
      const result = await lease.session.disposeSubagent(subagentId);
      if (!this.#isCurrentPresentationLease(lease)) return;
      const subject = `subagent ${subagentId}`;
      switch (result.outcome) {
        case "disposed":
          this.#showTransient("info", `retained workspace for ${subject} disposed; source changes discarded`);
          return;
        case "already_disposed":
          this.#showTransient("info", `retained workspace for ${subject} was already disposed`);
          return;
        case "disposal_pending":
          this.#showTransient(
            "info",
            `retained worktree for ${subject} was removed; branch cleanup remains pending`,
          );
          return;
        case "no_retained_workspace":
          this.#showTransient("info", `${subject} has no retained workspace`);
          return;
      }
    } catch (error: unknown) {
      if (!this.#isCurrentPresentationLease(lease)) return;
      if (error instanceof AppServerRequestError) {
        this.#showTransient("error", `workspace disposal refused: ${compactDiagnostic(error)}`);
        return;
      }
      this.#showTransient("error", `workspace disposal failed: ${compactDiagnostic(error)}`);
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
      case "choose_approval":
        this.#showApprovalSelector(lease);
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
      case "focus_session":
        await this.#focusSession(
          outcome.sessionId,
          outcome.nodeId,
          outcome.editorContent,
          outcome.notice,
          lease,
        );
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
    if (this.#subagentListFocused) {
      this.#subagentListFocused = false;
      this.#selectedSubagentId = undefined;
      this.#renderState(this.#session.state);
      return;
    }
    if (this.#switching) return;
    const state = this.#session.state;
    const attempt = state.attempt;
    if (state.pendingInteractions.length > 0 ||
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
    if (this.#switching) return;
    const state = this.#session.state;
    if (state.pendingInteractions.length > 0 ||
      (state.attempt !== undefined && state.attempt.phase.type !== "settled")) {
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

  /**
   * Exits the terminal, according to who owns the App Server process.
   *
   * ```text
   * local self-hosted            existing / remote
   *   close the child's stdin      close this socket
   *   the child sees EOF,          the server keeps running, every
   *   detaches and exits           Session stays loaded, accepted
   *   (SIGTERM/SIGKILL only if     work keeps executing, pending
   *    it overstays its grace)     interactions stay pending
   * ```
   *
   * The local path may end work that was in flight. That happens because the
   * process this TUI owns is deliberately shutting down — not because detaching
   * a transport is execution authority, and not because this client decided
   * anything was finished. Nothing here reports a settlement it did not
   * observe, in either mode. Persistent execution across TUI exit is what an
   * externally managed App Server is for.
   *
   * #291 owns graceful drain policy for the owned process; this is the
   * shutdown boundary that exists today, used as it exists today.
   */
  async quit(): Promise<void> {
    if (this.#quitting) {
      return;
    }
    this.#quitting = true;
    this.#invalidatePresentation();
    this.#editor.disableSubmit = true;
    this.#showTransient(
      "info",
      this.#host.ownership === "owned_child"
        ? "shutting down the App Server this client started…"
        : "disconnecting; the App Server keeps running…",
    );

    const exit = await this.#host.shutdown();
    this.#exitCode = exit === undefined ? 0 : (exit.code ?? 1);
    this.#finish(this.#exitCode);
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
    this.#closeOverlay(true);
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
   * Opens approval selection over the editor without changing runtime policy.
   *
   * The overlay owns focus while it is up and hands it straight back to the
   * editor on select or cancel, so the editor is never left unfocused.
   */
  #showApprovalSelector(lease: PresentationLease): void {
    const ownerCurrent = () => this.#isCurrentPresentationLease(lease);
    const ownerPending = () => this.#pendingApprovalRequest !== undefined &&
      this.#isCurrentPresentationLease(this.#pendingApprovalRequest.owner);
    if (!ownerCurrent()) return;
    if (ownerPending()) {
      this.#showTransient("info", "Approval change is still pending.");
      return;
    }
    let handle: OverlayHandle;
    const overlayAlive = () => ownerCurrent() && this.#overlay === handle;
    const selector = new ApprovalSelector({
      state: () => lease.session.state,
      change: () => { if (overlayAlive()) this.#tui.requestRender(); },
      close: () => { if (overlayAlive()) this.#closeOverlay(); },
      submit: async (mode) => {
        if (!overlayAlive() || ownerPending()) return;
        const request: PendingApprovalRequest = { owner: lease };
        this.#pendingApprovalRequest = request;
        try {
          const result = await lease.session.approvalModeSet(mode);
          // Esc ends the popup, not the submitted operation. Feedback belongs
          // to its presentation owner even when that owner's popup is closed.
          if (!ownerCurrent()) return;
          const latest = lease.session.state;
          const fact = latest && latest.approvalModeRevision > result.revision ? latest : result;
          this.#showTransient("info", `Approval request accepted: effective ${approvalLabel(fact.effectiveApprovalMode)}${fact.pendingApprovalMode == null ? "" : ` · next attempt ${approvalLabel(fact.pendingApprovalMode)}`}`);
        } catch (error) {
          if (ownerCurrent()) this.#showTransient("error", `Approval change failed: ${compactDiagnostic(error)}`);
        } finally {
          // A superseded owner may settle after a new owner submitted another
          // request. Only this exact token may clear the stored operation.
          if (this.#pendingApprovalRequest === request) this.#pendingApprovalRequest = undefined;
        }
      },
    });
    handle = this.#showPopup(selector, { width: "85%", heightPercent: 85 });
  }

  #showModelSelector(models: CatalogModelView[], lease: PresentationLease): void {
    if (!this.#isCurrentPresentationLease(lease)) return;
    const state = lease.session.state;
    if (state === undefined || state.sessionModel === null) {
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
    sessions: SessionSummaryView[] | undefined,
    nextOffset: number | undefined,
    query: string,
    lease: PresentationLease,
  ): void {
    if (!this.#isCurrentPresentationLease(lease)) return;
    if (this.#hitlOverlay !== undefined) return;
    const workflow = this.#deletion;
    // Initial /resume command responses obey the same mutation boundary as pages.
    if (lease.sessionListGeneration !== workflow.generation) return;
    if (sessions?.length === 0 && workflow.state.kind === "idle") {
      this.#showTransient("info", "no persisted sessions are available");
      return;
    }
    const selector = new ResumeSelector({
      initialPage: sessions === undefined ? undefined : { sessions, nextOffset }, query, client: this.#host, workflow,
      alive: () => this.#isCurrentPresentationLease(lease) && this.#overlay === handle,
      feedback: (text) => this.#showTransient("info", text),
    });
    const handle = this.#showPopup(selector, { width: "80%", heightPercent: 70 });
    this.#resumePresentation = selector;
    selector.onChange = () => {
      if (this.#isCurrentPresentationLease(lease)) this.#tui.requestRender();
    };
    selector.onCancel = () => {
      if (this.#isCurrentPresentationLease(lease) && this.#overlay === handle) this.#closeOverlay();
    };
    selector.onSelect = (session) => {
      if (!this.#isCurrentPresentationLease(lease)) return;
      this.#closeOverlay();
      void this.#handleOutcome(
        this.#dispatcher.selectSession(session.id),
        lease,
      ).catch((error: unknown) => {
        if (this.#isCurrentPresentationLease(lease)) {
          this.#showTransient("error", `session selection failed: ${compactDiagnostic(error)}`);
        }
      });
    };
  }

  /** HITL and any current popup keep focus; unresolved native outcomes wait here. */
  #syncDeletionPresentation(): void {
    const workflow = this.#deletion;
    if (workflow?.state.kind === "result" && workflow.state.outcome.status === "deleted" && !this.#resumePresentation) {
      workflow.dismiss();
      return;
    }
    if (!workflow?.needsPresentation || this.#overlay !== undefined || this.#finished || this.#terminalFinishStarted) return;
    const reconciliation = workflow.reconciliation;
    const query = this.#resumeQuery ?? workflow.context.query;
    const page = reconciliation.kind === "ready" && reconciliation.query === query ? reconciliation.page : undefined;
    this.#showSessionSelector(page?.sessions, page?.nextOffset,
      query, this.#presentationLease());
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
      void lease.session.boundaries(offset).then((page) => {
        if (!this.#isCurrentPresentationLease(lease)) return;
        currentNextOffset = page.nextOffset;
        selector.appendPage(page.boundaries, page.nextOffset);
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
    nodes: import("../protocol/app-server.ts").SessionNodeView[],
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
      // The graph page and the boundary page come from their own owners.
      void Promise.all([
        this.#host.sessionTree(session.id, request.nodeOffset),
        lease.session.boundaries(request.historyOffset),
      ]).then(([tree, page]) => {
        if (!this.#isCurrentPresentationLease(lease)) return;
        selector.appendPage({
          nodes: tree.nodes,
          nextNodeOffset: tree.nextOffset,
          boundaries: page.boundaries,
          nextHistoryOffset: page.nextOffset,
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
      const outcome = selection.kind === "node"
        ? Promise.resolve(
            this.#dispatcher.selectTreeNode(session.id, selection.node.id),
          )
        : this.#dispatcher.branchAt(selection.boundary);
      void outcome
        .then((resolved) => this.#handleOutcome(resolved, lease))
        .catch((error: unknown) => {
          if (this.#isCurrentPresentationLease(lease)) {
            this.#showTransient("error", `session focus change failed: ${compactDiagnostic(error)}`);
          }
        });
    };
  }

  #closeOverlay(replacing = false): void {
    const handle = this.#overlay;
    if (handle === undefined) return;
    if (this.#resumePresentation) this.#resumeQuery = this.#resumePresentation.reconciliationContext().query;
    this.#resumePresentation?.dispose();
    this.#resumePresentation = undefined;
    handle.hide();
    this.#overlay = undefined;
    this.#hitlOverlay = undefined;
    this.#tui.setFocus(this.#editor);
    if (!replacing) this.#syncDeletionPresentation();
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
      sessionListGeneration: this.#deletion.generation,
      session: this.#session,
    };
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
    // Submitted approval requests retain their tokens until completion. The
    // old lease becomes stale; it neither blocks nor clears a new owner's work.
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
    this.#closeOverlay(true);
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
            this.#sessionInfo,
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

    this.#syncHitlOverlay(state);
    this.#syncDeletionPresentation();
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
    // A dismissal is scoped to the exact dismissed interaction and to the
    // queue as it stood then. A distinct arrival supersedes the dismissal:
    // the newly required human input takes the focus (the surface was
    // dismissed, so no in-progress panel is interrupted) and the surface
    // reopens, while the dismissed interaction stays pending and unanswered.
    // The move itself is presentation-only — it emits no response.
    const dismissed = this.#hitlDismissed;
    if (dismissed !== undefined) {
      if (
        findPendingInteraction(state.pendingInteractions, dismissed.interaction) ===
        undefined
      ) {
        this.#hitlDismissed = undefined;
      } else {
        const arrival = state.pendingInteractions.find(
          (entry) => !dismissed.known.has(interactionKey(entry.interaction)),
        );
        if (arrival !== undefined) {
          this.#hitlDismissed = undefined;
          this.#interactionFocus = arrival.interaction;
        }
      }
    }
    const focused = this.#interactionFocus;
    if (focused === undefined) {
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
      sameInteractionRef(this.#hitlDismissed.interaction, focused)
    ) {
      return;
    }
    const lease = this.#presentationLease();
    const overlay = new HumanInteractionOverlay({
      onReview: (interaction, response) => this.#respondToInteraction(lease, overlay, interaction, { type: "review", response }),
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
        // Scope the dismissal to this exact interaction and to the queue as
        // it stands now, so a later distinct arrival supersedes it.
        this.#hitlDismissed = {
          interaction,
          known: new Set(
            (this.#session.state?.pendingInteractions ?? []).map((entry) =>
              interactionKey(entry.interaction),
            ),
          ),
        };
        if (this.#hitlOverlay === overlay) {
          this.#closeOverlay();
        }
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
    const closed = this.#host.client.closed;
    return closed === undefined ? "connected" : `closed: ${closed.reason}`;
  }

  /** Builds the footer's current-conversation label from attachment identity. */
  #conversationContext(): ConversationContext | undefined {
    return { conversationId: this.#session.target.conversation_id };
  }

  #diagnostics(): DebugDiagnostics {
    const stderr = this.#host.stderrTail();
    const exit = this.#host.childExit;
    const target = this.#session.target;
    return {
      connection: this.#host.describe(),
      // Ownership is established by who spawned the process, never inferred
      // from the endpoint, the Session, or the transport.
      ownership:
        this.#host.ownership === "owned_child"
          ? "this client owns the App Server process and ends it on exit"
          : "an external owner runs the App Server; exiting only disconnects",
      sessionId: target.session_id,
      attachmentId: target.attachment_id,
      conversationId: target.conversation_id,
      runtimeIncarnation: target.runtime_incarnation,
      cursor: this.#session.state.cursor,
      attachedSessions: this.#host.attached.length,
      connectionState: this.#connectionLabel(),
      childStatus:
        this.#host.ownership === "external"
          ? "not applicable (external App Server)"
          : exit === undefined
            ? "running"
            : `exited (code ${exit.code ?? "none"}, signal ${exit.signal ?? "none"})`,
      stderrTail: stderr.text,
      stderrTruncatedBytes: stderr.truncatedBytes,
      pendingRequests: this.#host.client.pendingCount,
      resyncCount: this.#session.resyncCount,
    };
  }

  #finish(code: number): void {
    this.#deletion?.terminate();
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

function editorText(content: UserContentBlock[] | undefined): string {
  const nonText = (content ?? []).find((block) => block.type !== "text");
  if (nonText !== undefined) {
    throw new Error(
      `fork/tree editor restoration does not support ${nonText.type} content yet`,
    );
  }
  return (content ?? [])
    .map((block) => (block.type === "text" ? block.text : ""))
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
