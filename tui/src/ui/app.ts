import {validateInteractionBody} from '../../../protocol/app-server/interaction.ts';
import { displayInteraction, hasInteractionBody, type DisplayInteraction } from '../presentation/interaction-display.ts';
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
 * disposable render target rebuilt from the bounded checkpoint and contiguous
 * events, with deferred content loaded through explicit bounded reads. Rendering
 * does not consult anything Pi remembers, and component instance continuity is never a
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

import { editorText } from "../app-server/editor.ts";
import { PasteGuard } from "./paste-guard.ts";
import { ComposerDraft } from "./composer-draft.ts";
import { ComposerCommand } from "./components/composer-command.ts";
import { open } from "node:fs/promises";
import { basename } from "node:path";
import { ComposerEditor, composerIntent } from "./composer.ts";
import { PromptHistory } from "./components/prompt-history.ts";
import { ComposerContext } from "./components/composer-context.ts";
import { PendingInputView } from "./components/pending-input.ts";
import { isAttemptActive } from "../presentation/state.ts";
import { historyPresentation } from "../presentation/projection.ts";
import { sanitizeField } from "../sanitize.ts";
import type { ConnectingSession, SessionCatalogPage } from "../startup.ts";
import {
  Container,
  Loader,
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
import type { CheckpointPage } from '../../../protocol/app-server/v51.ts';
import { AppServerRequestError, isDeliveryCancelled } from "../app-server/client.ts";
import type { AppServerHost } from "../app-server/host.ts";
import type { AppServerSession } from "../app-server/session.ts";
import type {
  CatalogModelView,
  SessionSettings,
  SessionDeleteResult,
  SessionSummaryView,
  SessionUserMessageBoundaryObservation,
  SessionView,
  ToolCallId,
  InteractionRef,
  RoutedInteraction,
  UserInputBlock,
} from "../protocol/app-server.ts";
import {
  BoundarySelector,
} from "./components/session-selector.ts";
import { TreeSelector, type TreeSelection } from "./components/tree-selector.ts";
import {
  DeliverySelector,
  type DeliveryAvailability,
} from "./components/delivery-selector.ts";
import type { DeliveryPage } from "../presentation/deliveries.ts";
import {
  DeliveryResidueError,
  DeliveryUncertainError,
  deliveryDestination,
  launchOpener,
  localEffect,
  openDelivery,
  residueText,
  saveDelivery,
  systemOpener,
  type LocalEffect,
  type Opener,
} from "../app-server/delivery-files.ts";
import { TransportClosedError } from "../app-server/transport.ts";
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
import { ConfirmationView } from "./components/confirmation.ts";
import { PopupFrame, type PopupContent } from "./components/popup-frame.ts";
import { TransientFeedbackSurface } from "./components/transient-feedback.ts";
import { SourceRepairStatus } from "./components/source-repair-status.ts";
import type { SourceRepairObservation } from "../presentation/source-repairs.ts";
import {
  FooterView,
  renderStartup,
  startupVisible,
  workingStatus,
} from "./components/status.ts";
import type { ConversationContext } from "./components/status.ts";
import { renderResourceBanner } from "./components/resources.ts";
import { renderTranscript } from "./components/transcript.ts";
import { renderTodoPanel, deferredTodoRecords } from "./components/todos.ts";
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
import { style } from "./theme.ts";
import { banded } from "./components/transcript-block.ts";
import { SubagentTranscript } from "../app-server/subagent-transcript.ts";
import { SubagentTranscriptView } from "./components/subagent-transcript-view.ts";
import { HumanInteractionOverlay } from "./components/hitl.ts";
import type { InteractionResponse, SourceTarget } from "../protocol/app-server.ts";

export interface RustxTuiAppOptions {
  /** The App Server connection, its Session catalog, and who owns its process. */
  host: AppServerHost;
  /** Reconnects to the externally managed server; never spawns a child. */
  reconnect?: () => Promise<AppServerHost>;
  /** Absent while browsing the resume catalog, before explicit selection. */
  session?: AppServerSession;
  connecting?: ConnectingSession;
  /** `session/create` inputs for Sessions this client creates. */
  sessionSettings: SessionSettings;
  /** A read-only catalog page acquired during resume bootstrap. */
  resumePage?: SessionCatalogPage;
  /**
   * The directory Sessions this launch creates are rooted at.
   *
   * Presentation only: it shortens the absolute paths the runtime publishes
   * for display. The client never reads it, resolves against it, or treats it
   * as a second opinion about where anything is running.
   */
  cwd?: string;
  /** The system opener `/files` Open asks; this platform's by default. */
  opener?: { command: string | undefined; launch: Opener };
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
  session: AppServerSession | undefined;
}

export class RustxTuiApp {
  #host: AppServerHost;
  readonly #reconnect: (() => Promise<AppServerHost>) | undefined;
  #recovering = false;
  #session: AppServerSession | undefined;
  readonly #dispatcher: CommandDispatcher;
  #initialConnection: ConnectingSession | undefined;
  #connecting: { queued?: string; history?: PresentationState } | undefined;
  #initialResumePage: SessionCatalogPage | undefined;
  readonly #workspace: string | undefined;
  readonly #opener: { command: string | undefined; launch: Opener };

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
  /** Newly observed configuration repairs of the current App Server
   * authority; presentation only (Issue #464). */
  readonly #repairStatus = new SourceRepairStatus();
  #removeRepairListener: () => void = () => {};
  readonly #footer = new FooterView(() => ({
    state: this.#connecting ? undefined : this.#session?.state,
    connection: this.#connectionLabel(),
    session: this.#connecting ? undefined : this.#sessionInfo,
    conversation: this.#connecting ? undefined : this.#conversationContext(),
  }));
  readonly #editor: ComposerEditor;
  readonly #promptHistory: string[] = [];
  readonly #composerContext = new ComposerContext(() => ({ state: this.#connecting ? undefined : this.#session?.state, draft: this.#draft }));
  #submitting = false;
  #draft = new ComposerDraft();
  readonly #drafts = new Map<string, ComposerDraft>();
  #uploading = false;
  readonly #inputPaste = new PasteGuard();
  readonly #loader: Loader;

  #preferences: PresentationPreferences = defaultPreferences();
  #overlay: OverlayHandle | undefined;
  /** Retires the work the current overlay owns when it closes or is replaced. */
  #overlayRetire: (() => void) | undefined;
  #hitlOverlay: HumanInteractionOverlay | undefined;
  /**
   * Presentation-only focus over `pendingInteractions`, reconciled against
   * the authoritative projection on every render. Never a semantic fact: it
   * picks which interaction the human-input surface shows, nothing else.
   */
  #interactionFocus: InteractionRef | undefined;
  #interactionPage: { session: AppServerSession; stamp: string; offset: number; page: CheckpointPage; items: DisplayInteraction[] } | undefined;
  #interactionRead = 0;
  #interactionBody?: {session:AppServerSession;stamp:string;interaction:InteractionRef;body:RoutedInteraction};
  #interactionBodyRead = 0;
  #interactionArgumentRange?: {session:AppServerSession;stamp:string;interaction:InteractionRef;text:string;offset:number;next:number|null;end:number;field:"arguments"|"reason"|"tool_name";decoder:InstanceType<typeof TextDecoder>};
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
  #childInspection: { reader: SubagentTranscript } | undefined;
  #reconnectChild: string | undefined;
  #selectedAgentId: string | undefined;
  #presentationEpoch = 0;
  #contentViewEpoch = 0;
  #deletion!: SessionDeletionWorkflow;
  #resumePresentation: ResumeSelector | undefined;
  #resumeQuery: string | undefined;
  #terminalFinishStarted = false;
  #removeStateListener: (() => void) | undefined;
  #removeSnapshotListener: (() => void) | undefined;
  #removeClosedListener: (() => void) | undefined;
  #removeConnectionListener: () => void = () => {};
  /** The durable metadata of the visible Session, refreshed authoritatively. */
  #sessionInfo: SessionView | undefined;
  #resolveExit: ((code: number) => void) | undefined;

  constructor(options: RustxTuiAppOptions) {
    this.#host = options.host;
    this.#reconnect = options.reconnect;
    this.#session = options.session;
    this.#initialResumePage = options.resumePage;
    this.#initialConnection = options.connecting;
    this.#workspace = options.cwd;
    this.#opener = options.opener ?? { command: systemOpener(), launch: launchOpener };

    this.#tui = new TUI(new ProcessTerminal());
    this.#editor = new ComposerEditor(this.#tui);
    this.#editor.setAutocompleteProvider(new SlashCommandAutocompleteProvider());
    this.#editor.onChange = () => { this.#draft.text = this.#editor.getExpandedText(); };
    this.#editor.running = () => this.#session !== undefined && isAttemptActive(this.#session.state);
    this.#editor.onPrompt = (text, commands) => { void this.#onSubmit(text, false, commands); };
    this.#editor.onQueue = text => { void this.#onSubmit(text, true, false); };
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
    this.#tui.addChild(this.#repairStatus);
    this.#tui.addChild(this.#todos);
    this.#tui.addChild(new Spacer(1));
    this.#tui.addChild(this.#composerContext);
    this.#tui.addChild(this.#editor);
    this.#tui.addChild(this.#footer);

    this.#watchConnection();
    this.#watchSourceRepairs();
    this.#bindSession(this.#session);
  }

  /**
   * Presents source repair observations of the current connection's App
   * Server authority: those already held by the host, then each new one. A
   * replacement authority retires the previous one's line, and its receipts
   * are its own. Only the current host is observed: a replaced connection's
   * late notice or read changes nothing.
   *
   * A reconnect to the same authority keeps the line, and then rereads each
   * source it still presents: the new connection observes a Workspace only
   * once it is addressed again, so a Workspace whose receipt was withdrawn
   * while this client was away is announced to no one until that read.
   */
  #watchSourceRepairs(): void {
    this.#removeRepairListener();
    const host = this.#host;
    const kept = this.#repairStatus.adopt(host.authorityId);
    const observe = (observation: SourceRepairObservation) => {
      if (this.#host !== host || this.#quitting || this.#finished || !this.#repairStatus.observe(host.authorityId, observation)) return;
      if (this.#started) this.#tui.requestRender();
    };
    for (const observation of host.sourceRepairs()) observe(observation);
    this.#removeRepairListener = host.onSourceRepair(observe);
    if (this.#started) this.#tui.requestRender();
    if (kept) void this.#reconcileSourceRepairs(host);
  }

  /**
   * Rereads, through `host`, every source the repair line still presents:
   * each Workspace, whose read also rereads the User, and the User alone
   * only when no Workspace read succeeded — a Workspace that can no longer
   * be read never keeps the independent User source from its own read.
   * Bounded by the line's finite sources, one read at a time, and read-only
   * (`configuration/sourcesRead` never repairs, reconciles, loads a Session
   * or reaches a model). Each result reaches the line through the host's
   * sequence-ordered observations, so it never overwrites a newer notice.
   * It stops once `host` is replaced, closed or the client quits. A failed
   * read proves nothing — the entry stays until a later read or notice —
   * and it never fails the connection.
   */
  async #reconcileSourceRepairs(host: AppServerHost): Promise<void> {
    const pending = this.#repairStatus.pendingSources();
    let userRead = false;
    const read = async (target: SourceTarget) => {
      if (this.#host !== host || host.client.closed !== undefined || this.#quitting || this.#finished) return false;
      try {
        await host.readSourceRepairs(target);
        userRead = true;
      } catch {
        // Not a withdrawal; `/settings repairs` reads again explicitly.
      }
      return true;
    };
    for (const target of pending.filter(source => source.kind === "workspace")) {
      if (!await read(target)) return;
    }
    if (!userRead && pending.some(source => source.kind === "user")) await read({ kind: "user" });
  }

  #watchConnection(): void {
    this.#removeConnectionListener();
    const host = this.#host;
    this.#removeConnectionListener = host.client.onClose((error) => {
      if (this.#host !== host || this.#quitting || this.#finished) return;
      if (this.#connecting) {
        this.#retainConnectingInput(this.#connecting.queued);
        this.#connecting = undefined; this.#loader.stop(); this.#activity.clear();
      }
      this.#deletion?.terminate();
      this.#initialResumePage = undefined;
      this.#reconnectChild = this.#childInspection?.reader.selected;
      this.#invalidatePresentation();
      this.#editor.disableSubmit = true;
      if (host.ownership === "external" && this.#reconnect !== undefined) {
        void this.#recoverConnection();
      } else {
        void host.shutdown().finally(() => this.#showTerminalFailureAndFinish(
          `${compactDiagnostic(error)}\nApp Server connection/process failure. Runtime outcomes are unknown.`, 1,
        ));
      }
    });
  }

  async #recoverConnection(): Promise<void> {
    if (this.#recovering || this.#reconnect === undefined || this.#quitting || this.#finished) return;
    this.#recovering = true;
    this.#switching = true;
    const oldHost = this.#host;
    const focused = this.#session;
    const deletion = this.#deletion.state;
    const deletionTarget = "sessionId" in deletion ? deletion.sessionId : undefined;
    let replacement: AppServerHost | undefined;
    this.#showTransient("info", "reconnecting; unanswered mutations have unknown outcomes and will not be resent…");
    try {
      await oldHost.shutdown();
      replacement = await this.#reconnect();
      replacement.inheritModelSelectionUncertainty(oldHost);
      let focus = focused;
      let observedDeletion: SessionDeleteResult | undefined;
      // A historical deletion target is independent of the currently focused Session.
      if (deletionTarget !== undefined) {
        const observed = await replacement.previewSessionDeletion(deletionTarget);
        observedDeletion = observed;
        if (focused?.sessionId === deletionTarget && (observed.status === 'deleted' || observed.status === 'not_found' || observed.status === 'committed_cleanup_pending' || observed.status === 'committed_durability_uncertain')) {
          focus = undefined;
          this.#showTransient('info', `Deletion observation: ${observed.status.replaceAll('_', ' ')}. No deletion was replayed.`);
        }
      }
      const session = focus === undefined ? undefined : await replacement.attach(focus.sessionId, focus.nodeId, focus);
      if (this.#quitting || this.#finished || this.#host !== oldHost) {
        await replacement.shutdown();
        return;
      }
      this.#host = replacement;
      this.#dispatcher.setHost(replacement);
      this.#bindSession(session);
      this.#watchConnection();
      this.#watchSourceRepairs();
      if (session !== undefined && this.#reconnectChild !== undefined) this.#openChildTranscript(this.#reconnectChild);
      this.#reconnectChild = undefined;
      if (session !== undefined) this.#renderState(session.state);
      else if (this.#started) await this.#openResumeSelector();
      // Rebinding creates a new connection-local workflow. Transfer only the
      // authoritative committed value, retaining its original Session identity.
      // Use the fresh selector context; a failed list read supplies no rows.
      if (observedDeletion?.status === "committed_cleanup_pending" || observedDeletion?.status === "committed_durability_uncertain") {
        this.#deletion.observeCommitted(observedDeletion,
          this.#resumePresentation?.reconciliationContext() ?? { query: "", ids: [], index: 0, loaded: 0 });
      }
      this.#showTransient("info", "reconnected from server state; no unanswered mutations were resent\nUnanswered mutation outcomes remain unknown.");
    } catch (error) {
      await replacement?.shutdown();
      this.#showTransient("error", `reconnect failed: ${compactDiagnostic(error)}. Ctrl+R retries; Ctrl+C exits. Unanswered mutations remain unknown.`);
    } finally {
      this.#recovering = false;
      this.#switching = false;
      this.#editor.disableSubmit = this.#quitting || this.#host.client.closed !== undefined;
    }
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
  #bindSession(session: AppServerSession | undefined): void {
    this.#draft.text = this.#editor.getExpandedText();
    if (this.#session !== undefined) this.#drafts.set(`${this.#session.sessionId}:${this.#session.target.conversation_id}`, this.#draft);
    const key = session === undefined ? undefined : `${session.sessionId}:${session.target.conversation_id}`;
    const draft = key === undefined ? new ComposerDraft() : this.#drafts.get(key) ?? new ComposerDraft();
    if (draft !== this.#draft) {
      this.#draft = draft;
      this.#editor.setText(draft.text);
      this.#refreshDraftLabel();
    }
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
    this.#editor.disableSubmit = this.#host.client.closed !== undefined;
    if (session === undefined) return;
    this.#removeSnapshotListener = session.onSnapshot(() => {
      // A resync is an authoritative replacement within this attachment. It
      // invalidates local inspection, picker, and transient ownership, while
      // the subsequent state publication still renders the new projection.
      if (this.#session !== session || this.#finished) return;
      const child = this.#childInspection?.reader.selected;
      this.#invalidatePresentation();
      if (child !== undefined) this.#openChildTranscript(child);
    });
    let configurationNotice:string|undefined;
    let observationNotice:string|undefined;
    this.#removeStateListener = session.onState((state) => {
      // Runtime state rendering follows attachment identity, not the
      // presentation epoch: the state published immediately after a snapshot
      // replacement is the authoritative state we must render.
      if (this.#session !== session || this.#finished) return;
      this.#renderState(state);
      if (session.observationError !== observationNotice) {
        observationNotice = session.observationError;
        if (observationNotice) this.#showTransient("error", `Conversation observation failed: ${observationNotice}`);
      }
      const notice=session.configurationObservationError ? `Configuration observation unavailable: ${session.configurationObservationError}` : session.deferredConfiguration ? 'Loading deferred configuration…' : undefined;
      if(notice!==configurationNotice){configurationNotice=notice;if(notice)this.#showTransient(session.configurationObservationError?'error':'info',notice);}
    });
    this.#removeClosedListener = session.onClosed(() => {
      // The server retired this attachment's residency. That is a statement
      // about observability, not a runtime outcome, and it is reported as one.
      if (this.#session !== session || this.#finished) return;
      if (this.#deletion.state.kind === "pending" && this.#deletion.state.sessionId === session.sessionId) {
        this.#editor.disableSubmit = true;
        return;
      }
      this.#showTransient(
        "error",
        "This Session connection closed. Open the Session again to inspect its current state.",
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
    editorContent: UserInputBlock[] | undefined,
    notice: string | undefined,
    lease: PresentationLease,
    confirmedNodeChange = false,
  ): Promise<void> {
    if (!this.#isCurrentPresentationLease(lease) || this.#switching) return;
    if (this.#session !== undefined && sessionId === this.#session.sessionId && editorContent === undefined &&
        (nodeId === undefined || nodeId === this.#session.nodeId) && !this.#session.serverClosed) {
      await this.#session.resync();
      if (notice !== undefined) this.#showTransient("info", notice);
      return;
    }
    const existing = this.#host.attachment(sessionId);
    const changingNode = nodeId !== undefined && existing !== undefined &&
      !existing.released && !existing.serverClosed && nodeId !== existing.nodeId;
    if (changingNode && !confirmedNodeChange) {
      const confirmation = new ConfirmationView({
        title: "Change conversation node", subject: `Session ${sessionId}`,
        confirmLabel: "Switch to this branch", permanent: false,
        warning: "Switching branches settles work on the current branch before opening the selected branch. Other Sessions keep running.",
        onConfirm: () => {
          this.#closeOverlay();
          void this.#focusSession(sessionId, nodeId, editorContent, notice, lease, true);
        },
        onCancel: () => this.#closeOverlay(),
      });
      this.#showPopup(confirmation, { width: "80%", minWidth: 48, heightPercent: 38 });
      return;
    }
    this.#switching = true;
    const previousDraft = this.#draft;
    previousDraft.text = this.#editor.getExpandedText();
    const connection: { queued?: string; history?: PresentationState } | undefined = changingNode ? undefined : {};
    this.#connecting = connection;
    if (connection) { this.#draft = new ComposerDraft(); this.#editor.setText(""); this.#renderConnecting(); }
    this.#editor.disableSubmit = changingNode;
    const host = this.#host;
    if (connection) void host.client.call("session/history", { session_id: sessionId, node_id: nodeId, at: { type: "latest" }, limit: 32 }, "session_history").then(result => {
      if (this.#connecting !== connection || this.#host !== host || this.#finished) return;
      connection.history = historyPresentation(result.conversation_id, result.window.page);
      this.#renderConnecting();
    }).catch(() => {});
    let installed: AppServerSession | undefined;
    try {
      const next = changingNode && nodeId !== undefined
        ? await this.#host.openNode(sessionId, nodeId)
        : await this.#host.attach(sessionId, nodeId);
      if (!this.#isCurrentPresentationLease(lease) || this.#host !== host) return;
      const typed = this.#editor.getExpandedText();
      if (connection) { this.#draft = previousDraft; this.#editor.setText(previousDraft.text); }
      this.#bindSession(next);
      if (connection && typed) this.#editor.setText(typed);
      installed = next;
      // A returning attachment may have folded events while it was off screen,
      // and the server is the only thing entitled to say what it holds now.
      if (this.#session === next) {
        await next.resync();
      }
      if (this.#finished || this.#quitting || this.#session !== next || this.#host.client.closed !== undefined) return;
      if (editorContent !== undefined && (!connection || !this.#editor.getExpandedText())) {
        this.#draft.restore(editorContent);
        this.#editor.setText(this.#draft.text);
      }
      this.#connecting = undefined;
      this.#switching = false;
      const queued = connection?.queued;
      if (queued !== undefined) {
        const typed = this.#editor.getExpandedText();
        const retainedDraft = this.#draft;
        this.#draft = new ComposerDraft();
        const sending = this.#onSubmit(queued, false, false);
        this.#draft = retainedDraft;
        this.#editor.setText(typed);
        void sending;
      }
      this.#refreshDraftLabel();
      this.#showTransient("info", notice ?? `showing session ${sessionId}`);
      this.#renderState(next.state);
    } catch (error: unknown) {
      if (this.#finished || this.#quitting || this.#host !== host ||
          (installed === undefined ? !this.#isCurrentPresentationLease(lease) : this.#session !== installed)) return;
      if (connection) {
        this.#retainConnectingInput(connection.queued);
        const retained = this.#editor.getExpandedText();
        this.#draft = previousDraft;
        this.#editor.setText(retained || previousDraft.text);
        this.#connecting = undefined;
        this.#loader.stop(); this.#activity.clear();
        if (this.#session) this.#renderState(this.#session.state);
        else {
          this.#startup.clear();
          this.#startup.addChild(new Text("Connection failed. Press Enter to choose a Session. Your input has been retained.", 1, 0));
        }
      }
      this.#showTransient(
        "error",
        `could not open Session ${sessionId}: ${compactDiagnostic(error)} · Input retained; nothing sent.`,
      );
    } finally {
      if (this.#host === host && !this.#recovering) this.#switching = false;
      if (!this.#finished) {
        this.#editor.disableSubmit = this.#switching || this.#quitting || this.#recovering || this.#host.client.closed !== undefined;
      }
    }
  }

  /** Reads the visible Session's durable metadata for the footer. */
  #refreshSessionInfo(): void {
    const session = this.#session;
    if (session === undefined) return;
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
      const state = this.#session?.state;
      if (state !== undefined) {
        this.#renderState(state);
      }
      if (this.#initialConnection) {
        const target = this.#initialConnection;
        this.#initialConnection = undefined;
        void this.#focusSession(target.sessionId, target.nodeId, undefined, undefined, this.#presentationLease());
      } else if (this.#session === undefined) {
        this.#startup.addChild(new Text("Choose a Session to resume. Enter reopens the picker; Ctrl+C exits.", 1, 0));
        const page = this.#initialResumePage;
        this.#initialResumePage = undefined;
        void this.#openResumeSelector(page);
      }
      this.#tui.addInputListener((data) => {
        // Paste packets are content, even when delivered in multiple fragments.
        // Pi owns their decoding; no global shortcut may inspect their payload.
        if (this.#inputPaste.content(data)) {
          return undefined;
        }
        // Any user input acknowledges the one current transient feedback item.
        // A later command or lifecycle result may replace it explicitly.
        this.#acknowledgeTransient();
        if (this.#host.client.closed !== undefined && this.#reconnect !== undefined) {
          if (matchesKey(data, "ctrl+r")) void this.#recoverConnection();
          if (matchesKey(data, "ctrl+c")) void this.quit();
          return { consume: true };
        }
        if (this.#connecting) {
          if (matchesKey(data, "ctrl+c")) { void this.quit(); return { consume: true }; }
          if (matchesKey(data, "escape")) {
            const queued = this.#connecting.queued;
            this.#connecting.queued = undefined;
            this.#retainConnectingInput(queued);
            this.#editor.disableSubmit = false;
            this.#renderConnecting();
            return { consume: true };
          }
          return undefined;
        }
        if (this.#session === undefined) {
          if (matchesKey(data, "ctrl+c")) { void this.quit(); return { consume: true }; }
          if (this.#overlay !== undefined) return undefined;
          if (matchesKey(data, "ctrl+r") || matchesKey(data, "enter")) void this.#openResumeSelector();
          return { consume: true };
        }
        if (this.#overlay === undefined && matchesKey(data, "ctrl+p")) {
          const command = new ComposerCommand(text => {
            this.#closeOverlay();
            void this.#onSubmit(text, false, true, true);
          }, () => this.#closeOverlay());
          this.#showPopup(command, { width: "90%", heightPercent: 50 });
          return { consume: true };
        }
        if (this.#overlay === undefined && matchesKey(data, "ctrl+r")) {
          const history = new PromptHistory(this.#promptHistory, text => {
            // Accept is an intentional replacement, so uploaded receipts are retired.
            this.#draft.clear(); this.#refreshDraftLabel();
            this.#editor.setText(text); this.#closeOverlay();
          }, () => this.#closeOverlay());
          this.#showPopup(history, { width: "85%", heightPercent: 60 });
          return { consume: true };
        }
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
          if (matchesKey(data, "enter") && this.#subagentListFocused && this.#selectedAgentId !== undefined) {
            this.#openChildTranscript(this.#selectedAgentId);
            return { consume: true };
          }
          if (matchesKey(data, "i") && this.#subagentListFocused) {
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
          if (lease.session === undefined) return { consume: true };
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
            const state = this.#session?.state;
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
          const state = this.#session?.state;
          const attempt = state?.attempt;
          const acted = this.#overlay !== undefined || (
            this.#subagentListFocused ||
            !this.#switching &&
            ((this.#session?.cancellationReady && this.#session.observationPhase !== 'following') || state.pendingInteractions.length > 0 ||
              (attempt !== undefined && ['admitted','running'].includes(attempt.phase.type)))
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

  async #onSubmit(text: string, queue = false, commands = true, preserveDraft = false): Promise<void> {
    if (this.#finished) return;
    if (this.#connecting) {
      if (!text.trim()) return;
      if (commands && text.trim().startsWith("/")) { this.#editor.setText(text); return; }
      if (this.#connecting.queued !== undefined) { this.#editor.setText(text); return; }
      this.#connecting.queued = text;
      this.#editor.setText("");
      this.#editor.disableSubmit = true;
      this.#renderConnecting();
      return;
    }
    if (this.#switching) return;
    if (commands && /^\/settings(?:\s|$)/.test(text.trim())) {
      const lease = this.#presentationLease();
      const result = await this.#dispatcher.submit(text);
      if (result.kind === "inspect") this.#showInspection(result.title, result.body, lease);
      else if (result.kind === "transient") this.#showTransient(result.level, result.text);
      return;
    }
    if (this.#session === undefined) return;
    if (this.#submitting || this.#uploading) { if (!preserveDraft) this.#editor.setText(text); return; }
    const lease = this.#presentationLease();
    const session = this.#session;
    const line = text.trim();
    if (!line && !this.#draft.blocks?.length) return;
    const command = commands && line.startsWith("/");
    const contentView = /^\/content(?:\s|$)/.test(line) ? this.#contentViewEpoch : undefined;
    if (preserveDraft && !command) { this.#showTransient("error", "Enter a slash command; the prompt draft is unchanged."); return; }
    if (!command) {
      if (!session.controlReady) {
        this.#editor.setText(text);
        this.#showTransient("error", "Session observation is not following. Request not sent; prompt draft is unchanged.");
        return;
      }
      let content: UserInputBlock[];
      try { content = this.#draft.submission(text); }
      catch (error) { this.#editor.setText(text); this.#showTransient("error", compactDiagnostic(error)); return; }
      const intent = composerIntent(session.state, queue);
      // Transfer this draft once. New typing cannot be cleared by a late result.
      this.#editor.setText(""); this.#draft.clear(); this.#refreshDraftLabel();
      const prompt = editorText(content);
      if (prompt) this.#promptHistory.push(prompt);
      if (this.#promptHistory.length > 1000) this.#promptHistory.shift();
      this.#editor.addToHistory(prompt);
      this.#submitting = true;
      try { if (intent === "steer") await session.steer(content); else await session.submitInbound(content); }
      catch (error) { if (this.#isCurrentPresentationLease(lease)) this.#showTransient("error", `${intent}: ${compactDiagnostic(error)} · Not replayed. Prompt remains in local history.`); }
      finally { this.#submitting = false; }
      return;
    }
    const agentMessage = /^\/send-message(?:\s|$)/.test(line);
    if (!preserveDraft) this.#editor.setText(agentMessage ? text : "");
    try {
      if (line === "/capabilities") {
        this.#showInspection("Agent capabilities", renderResourceBanner(session.state, { workspace: this.#workspace }), lease);
        return;
      }
      if (line === "/queue") {
        this.#showPopup(new PendingInputView(this.#tui, session, () => this.#closeOverlay(), () => this.#tui.requestRender()), { width: "90%", heightPercent: 75 });
        return;
      }
      if (line.startsWith("/attach ")) {
        await this.#attach(line.slice(8).trim(), lease);
        return;
      }
      const query = this.#resumeQuery ?? this.#deletion.context.query;
      const outcome = line === "/resume" && this.#deletion.state.kind !== "idle"
        ? { kind: "choose_session" as const, ...await this.#host.listSessions(query, 0), query }
        : await this.#dispatcher.submit(text);
      if (this.#isCurrentPresentationLease(lease) && (contentView === undefined || contentView === this.#contentViewEpoch)) {
        if (agentMessage && !preserveDraft && outcome.kind === "transient" && outcome.level === "info"
            && this.#editor.getExpandedText() === text) this.#editor.setText("");
        await this.#handleOutcome(outcome, lease);
      }
    } catch (error) {
      if (this.#isCurrentPresentationLease(lease) && (contentView === undefined || contentView === this.#contentViewEpoch)) this.#showTransient("error", `command: ${compactDiagnostic(error)}`);
    }
  }

  async #attach(path: string, lease: PresentationLease): Promise<void> {
    const session = lease.session;
    if (session === undefined) return;
    if (this.#uploading) throw new Error("An upload is already awaiting confirmation.");
    // Reserve the presentation position before any filesystem/transport await.
    // Newly typed text remains after the selected attachment, even during I/O.
    const draft = this.#draft;
    const prefix = draft.submission(this.#editor.getExpandedText());
    this.#uploading = true;
    draft.blocks = prefix;
    draft.tail = true;
    this.#editor.setText("");
    this.#refreshDraftLabel();
    this.#showTransient("info", "Uploading selected bytes; submission waits for native confirmation…");
    try {
      // Explicit user selection is the only intentional client file read.
      const file = await open(path, "r");
      let bytes: Buffer;
      try {
        const stat = await file.stat();
        if (!stat.isFile() || stat.size > session.uploadPolicy!.max_file_bytes) throw new Error("Select a regular file within the native upload policy.");
        const buffer = Buffer.alloc(session.uploadPolicy!.max_file_bytes + 1);
        let bytesRead = 0;
        while (bytesRead < buffer.length) {
          const read = await file.read(buffer, bytesRead, buffer.length - bytesRead, bytesRead);
          if (read.bytesRead === 0) break;
          bytesRead += read.bytesRead;
        }
        if (bytesRead > session.uploadPolicy!.max_file_bytes) throw new Error("Attachment exceeds native upload policy.");
        bytes = buffer.subarray(0, bytesRead);
      } finally { await file.close(); }
      const receipts = await session.upload(basename(path), bytes);
      // A resync cannot invalidate a confirmed Session receipt. Navigation
      // retains this draft under its original Session/conversation identity.
      draft.blocks = [...prefix, ...receipts];
      if (draft === this.#draft) {
        this.#refreshDraftLabel();
        this.#showTransient("info", `Attached ${basename(path)}`);
      }
    } finally { this.#uploading = false; }
  }

  #refreshDraftLabel(): void { this.#tui.requestRender(); }

  /** Moves the presentation focus through the authoritative child rows. */
  #moveSubagentSelection(direction: -1 | 1): void {
    const state = this.#session?.state;
    if (state === undefined) return;
    const selected = cycleSubagentSelection(
      state.agents,
      this.#selectedAgentId,
      direction,
    );
    if (selected === undefined) {
      this.#subagentListFocused = false;
      this.#selectedAgentId = undefined;
      return;
    }
    this.#subagentListFocused = true;
    this.#selectedAgentId = selected;
    this.#renderState(state);
  }

  /** Inspect history through the current parent, never a child Session. */
  #openChildTranscript(id: string): void {
    const lease = this.#presentationLease();
    const session = lease.session;
    if (session === undefined || !this.#isCurrentPresentationLease(lease)) return;
    const reader = new SubagentTranscript(session, id);
    const current = () => this.#childInspection?.reader === reader && this.#isCurrentPresentationLease(lease);
    const view = new SubagentTranscriptView(reader,
      () => reader.agent,
      this.#preferences,
      () => { if (current()) this.#closeOverlay(); },
      () => { if (current()) this.#tui.requestRender(); });
    this.#showPopup(view, { width: "90%", heightPercent: 80 });
    reader.onChange = () => { if (current()) this.#tui.requestRender(); };
    this.#childInspection = { reader };
    void reader.newest();
  }

  /** Existing lifecycle/resource details remain a separate surface (i). */
  async #inspectSelectedSubagent(): Promise<void> {
    if (
      !this.#subagentListFocused ||
      this.#selectedAgentId === undefined ||
      this.#finished
    ) {
      return;
    }
    const lease = this.#presentationLease();
    const attachedSession = lease.session;
    if (attachedSession === undefined) return;
    const agentId = this.#selectedAgentId;
    try {
      const subagent = await attachedSession.agentStatus(agentId);
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

  async #handleOutcome(
    outcome: Awaited<ReturnType<CommandDispatcher["submit"]>>,
    lease: PresentationLease,
  ): Promise<void> {
    if (!this.#isCurrentPresentationLease(lease)) return;
    switch (outcome.kind) {
      case "inspect":
        this.#showInspection(outcome.title, outcome.body, lease);
        break;
      case 'inspect_agent':
        this.#openChildTranscript(outcome.agentId);
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
      case "choose_delivery":
        this.#showDeliverySelector(outcome.page, lease);
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
    // A stale interaction cannot consume the gesture as an interaction response.
    // Cancellation still selects its owner through the live native attachment.
    if (this.#hitlOverlay !== undefined && this.#session?.observationPhase === 'following') {
      // Escape behavior is defined per interaction kind by the surface
      // itself: a questionnaire gets its explicit typed decline; an approval
      // is dismissed without any answer. The app never answers here.
      this.#hitlOverlay.escape();
      return;
    }
    if (this.#overlay !== undefined && this.#hitlOverlay === undefined) {
      this.#closeOverlay();
      return;
    }
    if (this.#subagentListFocused) {
      this.#subagentListFocused = false;
      this.#selectedAgentId = undefined;
      const state = this.#session?.state;
      if (state !== undefined) this.#renderState(state);
      return;
    }
    if (this.#switching) return;
    const state = this.#session?.state;
    if (state === undefined) return;
    const attempt = state.attempt;
    if (this.#session?.cancellationReady && (this.#session.observationPhase !== 'following' || state.pendingInteractions.length > 0 ||
      (attempt !== undefined && ['admitted','running'].includes(attempt.phase.type)))) {
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
    const state = this.#session?.state;
    if (state === undefined) { await this.quit(); return; }
    if (this.#session?.cancellationReady && (this.#session.observationPhase !== 'following' || state.pendingInteractions.length > 0 ||
      (state.attempt !== undefined && ['admitted','running'].includes(state.attempt.phase.type)))) {
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
   *   SIGTERM + close stdin       close this socket
   *   server-owned drain          server and accepted work continue
   *   wait for process exit       pending interactions stay pending
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
    retire?: () => void,
  ): OverlayHandle {
    this.#closeOverlay(true);
    this.#overlayRetire = retire;
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
  #showModelSelector(models: CatalogModelView[], lease: PresentationLease): void {
    const attachedSession = lease.session;
    if (attachedSession === undefined) return;
    if (!this.#isCurrentPresentationLease(lease)) return;
    const state = attachedSession.state;
    if (state === undefined) return;
    if (state.sessionModel === null) {
      this.#showInspection('Model catalog', `${attachedSession.catalogDeferredNotice ?? 'Session model configuration is deferred.'}\nUse /model-position for a deliberate native-defaults selection.`, lease);
      return;
    }
    const selector = new ModelSelector({
      models,
      sessionModel: state.sessionModel,
      attempt: state.attempt,
      deferredNotice: attachedSession.catalogDeferredNotice,
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
    const nextOffset=attachedSession.nextModelOffset;
    if(nextOffset!==undefined)selector.onNextPage=()=>{
      if(!this.#isCurrentPresentationLease(lease)||this.#overlay!==handle)return;
      void attachedSession.modelCatalog(nextOffset).then(catalog=>{if(this.#isCurrentPresentationLease(lease)&&this.#overlay===handle)this.#showModelSelector(catalog.models??[],lease);}).catch(error=>{if(this.#isCurrentPresentationLease(lease))this.#showTransient('error',String(error));});
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

  async #openResumeSelector(page?: SessionCatalogPage): Promise<void> {
    const lease = this.#presentationLease();
    try {
      const catalog = page ?? await this.#host.listSessions();
      if (!this.#isCurrentPresentationLease(lease)) return;
      this.#showSessionSelector(catalog.sessions, catalog.nextOffset, "", lease);
    } catch (error) {
      if (this.#isCurrentPresentationLease(lease)) {
        this.#showTransient("error", `session list unavailable: ${compactDiagnostic(error)}. Enter retries; Ctrl+C exits.`);
      }
    }
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
    if (this.#session !== undefined && sessions?.length === 0 && workflow.state.kind === "idle") {
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
    if (this.#session === undefined) {
      selector.onCreate = async () => {
        if (!this.#isCurrentPresentationLease(lease) || this.#overlay !== handle || this.#switching) return;
        const outcome = await this.#dispatcher.newSession();
        if (!this.#isCurrentPresentationLease(lease)) return;
        // A committed create invalidates the empty catalog even if attach fails.
        // Retire its action now; reopening resume must read fresh authority.
        if (outcome.kind === "focus_session") this.#closeOverlay();
        await this.#handleOutcome(outcome, lease);
      };
    }
    selector.onSelect = (session) => {
      if (!this.#isCurrentPresentationLease(lease)) return;
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
    const deletionState = workflow?.state;
    if (deletionState && 'sessionId' in deletionState && deletionState.sessionId === this.#session?.sessionId) {
      if (deletionState.kind === "pending") {
        this.#editor.disableSubmit = true;
        this.#dispatcher.setSession(undefined);
      } else if ((deletionState.kind === "needs_fresh_preview" || (deletionState.kind === "result" && deletionState.outcome.status === "blocked")) && !this.#session?.serverClosed) {
        this.#dispatcher.setSession(this.#session);
        this.#editor.disableSubmit = this.#host.client.closed !== undefined;
      } else if (deletionState.kind === "result" && ["deleted", "not_found", "committed_cleanup_pending"].includes(deletionState.outcome.status)) {
        const reconciliation = workflow.reconciliation;
        const next = reconciliation.kind === "ready" ? reconciliation.page.sessions.find(row => row.id !== deletionState.sessionId) : undefined;
        this.#session = undefined;
        this.#bindSession(undefined);
        this.#startup.clear(); this.#transcript.clear(); this.#activity.clear(); this.#todos.clear();
        this.#loader.stop();
        if (!next) void this.#openResumeSelector();
        if (next) void this.#focusSession(next.id, undefined, undefined, undefined, this.#presentationLease());
        return;
      }
    }
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
    boundaries: SessionUserMessageBoundaryObservation[],
    title: string,
    operation: "fork" | "tree",
    lease: PresentationLease,
    nextOffset?: number,
  ): void {
    const attachedSession = lease.session;
    if (attachedSession === undefined) return;
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
      void attachedSession.boundaries(offset).then((page) => {
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
    boundaries: SessionUserMessageBoundaryObservation[],
    nextHistoryOffset: number | undefined,
    lease: PresentationLease,
  ): void {
    const attachedSession = lease.session;
    if (attachedSession === undefined) return;
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
        attachedSession.boundaries(request.historyOffset),
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

  /**
   * `/files`: committed deliveries and explicit client-local actions.
   *
   * Availability comes from facts established at composition, never from
   * paths: delivery access is the connection's transport grant, and a shared
   * filesystem requires that this TUI spawned the server (Open additionally
   * verifies the leaf identity before launching anything). Each action is
   * one owned operation under the interaction's owner: retiring the surface
   * aborts it, and only an effect that already happened outlives it.
   */
  #showDeliverySelector(page: DeliveryPage, lease: PresentationLease): void {
    const session = lease.session;
    if (session === undefined || !this.#isCurrentPresentationLease(lease)) return;
    const sharedHost = this.#host.ownership === "owned_child";
    const opener = this.#opener.command;
    const launch = this.#opener.launch;
    const noAccess = this.#host.ownership === "owned_child"
      ? "the App Server did not grant delivery access to this connection"
      : "this connection holds no delivery access; reconnect with --delivery-access-token-file";
    const availability: DeliveryAvailability = {
      save: !session.deliveryAccess ? noAccess : undefined,
      open: !session.deliveryAccess ? noAccess
        : !sharedHost ? "this App Server's files are not on this machine; use Save"
          : opener === undefined ? "no system opener on this platform" : undefined,
    };
    const selector = new DeliverySelector(page, availability);
    // The one owner of this interaction's work. Every way it ends — Escape,
    // another overlay replacing it, Session focus change, attachment or
    // snapshot replacement, reconnect, terminal failure, quit — closes the
    // overlay, and closing aborts it: uncommitted local effects stop and the
    // native read or locate is cancelled on the server.
    const interaction = new AbortController();
    const handle = this.#showPopup(selector, { width: "80%", heightPercent: 70 }, () => interaction.abort());
    let inFlight: { operation: number; abort: AbortController } | undefined;
    const current = () => this.#isCurrentPresentationLease(lease) && this.#overlay === handle;
    selector.onChange = () => {
      if (current()) this.#tui.requestRender();
    };
    selector.onCancel = () => {
      if (current()) this.#closeOverlay();
    };
    selector.onAbort = (operation) => {
      if (inFlight?.operation === operation) inFlight.abort.abort();
    };
    selector.onLoadMore = () => {
      const before = selector.nextCursor;
      if (before === undefined) return;
      void session.deliveryPage(before, selector.readCut).then((older) => {
        if (current()) selector.appendPage(older);
      }).catch((error: unknown) => {
        if (!current()) return;
        this.#showTransient("error", `delivery history page failed: ${compactDiagnostic(error)}`);
        selector.retryPage();
      });
    };
    selector.onLoadCollection = (collection) => {
      void session.deliveryCollection(collection).then((next) => {
        if(current()) selector.replaceCollectionPage(next);
      }).catch((error:unknown)=>{
        if(!current())return;
        this.#showTransient("error", `delivery collection failed: ${compactDiagnostic(error)}`);
        selector.retryPage();
      });
    };
    selector.onAction = (operation, action, record, destination) => {
      const abort = new AbortController();
      const signal = AbortSignal.any([interaction.signal, abort.signal]);
      inFlight = { operation, abort };
      // What this action has done outside the process, recorded by the
      // action at the moment it happens.
      const effect = localEffect();
      const task = (async () => {
        if(action === "metadata") {
          const file = await session.deliveryMetadata(record, () => current() && !signal.aborted);
          if(current() && !signal.aborted) selector.installMetadata(operation,record,file);
          return "Original delivery metadata loaded";
        }
        if(record.file == null) throw new Error("Read original delivery metadata first.");
        if (action === "save") {
          const saved = await saveDelivery(
            () => session.readDelivery(record, signal),
            deliveryDestination(destination ?? ""),
            signal,
            undefined,
            effect,
          );
          const text = `Saved ${record.file.name} to ${saved.path}`;
          return saved.residue === undefined ? text
            : `${text}; warning: ${residueText(saved.residue.path, saved.residue.staged)} (${compactDiagnostic(saved.residue.cause)})`;
        }
        const requested = await openDelivery(() => session.locateDelivery(record, signal), {
          sharedHost,
          opener,
          launch,
          signal,
          effect,
        });
        return `${requested.opener} accepted the request to open ${requested.path}`;
      })();
      // One terminal report per operation. A live surface shows every
      // outcome. Once the interaction is retired, an outcome is still owed
      // only if the action committed an external effect (a dispatched save
      // link or a spawned opener) or left a staged file behind: it is reported
      // once, on the transient surface, never into a successor surface. A
      // cancelled or failed action that did neither is not reported.
      const report = (level: "info" | "error", text: string) => {
        const safe = sanitizeField(text);
        if (current()) selector.settle(operation, level, safe);
        else if (effect.committed || effect.residue) this.#showTransient(level, safe);
      };
      void task.then(
        (text) => report("info", text),
        (error: unknown) => report("error", deliveryFailure(error, signal, effect)),
      ).finally(() => {
        if (inFlight?.operation === operation) inFlight = undefined;
      });
    };
  }

  #closeOverlay(replacing = false): void {
    // A bounded byte read may finish after another view opens or closes. It
    // retains no right to repaint that successor, even on the same attachment.
    this.#contentViewEpoch += 1;
    if (this.#childInspection) {
      this.#childInspection.reader.dispose();
      this.#childInspection = undefined;
      this.#subagentListFocused = false;
    }
    const retire = this.#overlayRetire;
    this.#overlayRetire = undefined;
    retire?.();
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
    const state = this.#session?.state;
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
    const state = this.#session?.state;
    if (target === "none" || state === undefined) {
      return withAllCollapsed(this.#preferences);
    }
    const calls = [...correlateTools(state).byCallId.keys()];
    if (target === "all") {
      const executions = state.jobs.map(
        (execution) => execution.job_id,
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
    if (this.#transient.feedback === undefined && !this.#repairStatus.visible) {
      return;
    }
    this.#transient.acknowledge();
    this.#repairStatus.acknowledge();
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

  #retainConnectingInput(text: string | undefined): void {
    if (text === undefined) return;
    this.#promptHistory.push(text);
    if (this.#promptHistory.length > 1000) this.#promptHistory.shift();
    this.#editor.addToHistory(text);
    if (!this.#editor.getExpandedText()) this.#editor.setText(text);
  }

  #renderConnecting(): void {
    this.#startup.clear(); this.#transcript.clear(); this.#activity.clear(); this.#todos.clear();
    this.#loader.stop();
    const history = this.#connecting?.history;
    if (history) for (const block of renderTranscript(history, this.#preferences, correlateTools(history))) {
      this.#transcript.addChild(banded(block)); this.#transcript.addChild(new Spacer(1));
    }
    if (this.#connecting?.queued !== undefined) {
      this.#transcript.addChild(new Text(sanitizeField(this.#connecting.queued, true), 1, 0));
      this.#loader.setMessage("Connecting… Esc to withdraw.");
      this.#loader.start(); this.#activity.addChild(this.#loader);
    }
    this.#tui.requestRender();
  }

  /**
   * Rebuilds the visible components from the projection.
   *
   * The rebuild is total: every component is discarded and reconstructed from
   * `state`. That is what makes the UI reconstructable from a fresh snapshot —
   * no Pi component carries state the projection does not have.
   */
  #renderState(state: PresentationState): void {
    if (this.#connecting) { this.#renderConnecting(); return; }
    const source = this.#session;
    const stamp = this.#interactionStamp();
    if (this.#interactionPage && (this.#interactionPage.session !== source || this.#interactionPage.stamp !== stamp)) {
      this.#interactionPage = undefined;
      this.#interactionRead += 1;
    }
    // The viewport is presentation-only. It never replaces a native snapshot,
    // moves Applied Cursor, or infers settlement from an omitted identity.
    if (this.#interactionPage) state = { ...state, pendingInteractions: this.#interactionPage.items };
    const body=this.#interactionBody;
    if(body && body.session===source && body.stamp===stamp) state={...state,pendingInteractions:state.pendingInteractions.map(item=>sameInteractionRef(item.interaction,body.interaction)?body.body:item)};
    else if(body) {this.#interactionBody=undefined;this.#interactionBodyRead++;}
    if (!hasSubagentSelection(state.agents, this.#selectedAgentId)) {
      this.#selectedAgentId = undefined;
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
        this.#subagentListFocused ? this.#selectedAgentId : undefined,
      ),
      renderInteractionSection(state, this.#preferences, this.#interactionFocus),
    ]) {
      if (section.length > 0) {
        this.#activity.addChild(new Text(section, 1, 0));
      }
    }

    // The native loaded task prefix and its independently advertised range.
    // Extension absence and a deferred nonempty list remain distinct.
    this.#todos.clear();
    const todos = renderTodoPanel(selectTodos(state), {
      columns: this.#tui.terminal.columns,
      deferredTasks: deferredTodoRecords(this.#session?.checkpoint?.collections),
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

  #interactionStamp(): string {
    const cut = this.#session?.checkpoint?.read_cut;
    return cut?.domain_revisions['pending_interactions'] ?? cut?.revision ?? '';
  }

  #interactionTotal(): bigint {
    const frontier = this.#session?.checkpoint?.collections.find(value => value.domain === 'pending_interactions');
    return BigInt(this.#interactionPage?.page.total ?? frontier?.total ?? this.#session?.state.pendingInteractions.length ?? 0);
  }

  #interactionPartial(): boolean {
    return Boolean(this.#interactionPage?.offset) || this.#interactionTotal() > BigInt(this.#interactionPage?.items.length ?? this.#session?.state.pendingInteractions.length ?? 0);
  }

  #interactionPageLabel(): string | undefined {
    if (!this.#interactionPartial()) return undefined;
    const offset = this.#interactionPage?.offset ?? 0;
    const total = this.#interactionTotal();
    const end = BigInt(offset + (this.#interactionPage?.items.length ?? this.#session?.state.pendingInteractions.length ?? 0));
    return `Pending ${offset + 1}–${end < total ? end : total} of ${total} · Ctrl+PgUp/PgDn pages`;
  }

  async #pageInteractions(direction: -1 | 1): Promise<void> {
    const session = this.#session;
    if (!session || typeof session.checkpointPage !== 'function') return;
    const previous = this.#interactionPage;
    if(direction<0 && (previous?.offset ?? 0)===0) return;
    const offset = direction < 0 ? Math.max(0, (previous?.offset ?? 0) - 32)
      : Number(previous?.page.next_offset ?? (previous ? previous.offset + previous.items.length : session.state.pendingInteractions.length));
    if (!Number.isSafeInteger(offset) || offset < 0 || BigInt(offset) >= this.#interactionTotal()) return;
    const lease = this.#presentationLease(), overlay = this.#hitlOverlay, stamp = this.#interactionStamp(), read = ++this.#interactionRead;
    try {
      const { page, items } = await session.checkpointPage('pending_interactions', offset, previous?.page);
      if (!overlay || this.#hitlOverlay !== overlay || !this.#isCurrentPresentationLease(lease) || read !== this.#interactionRead || stamp !== this.#interactionStamp()) return;
      if (page.items.length > 32) throw new Error('Pending interaction page exceeded its item bound.');
      this.#interactionPage = { session, stamp, offset, page, items: (items as import('../../../protocol/app-server/v51.ts').RuntimeClientInteractionHeader[]).map(displayInteraction) };
      this.#interactionFocus = this.#interactionPage.items[0]?.interaction;
      this.#hitlDismissed = undefined;
      this.#renderState(session.state);
    } catch (error) {
      if (overlay && this.#hitlOverlay === overlay && this.#isCurrentPresentationLease(lease) && read === this.#interactionRead) {
        this.#showTransient('error', `Pending interactions unavailable: ${compactDiagnostic(error)}`);
      }
    }
  }

  async #readInteractionArguments(interaction: InteractionRef, next: boolean, selectedField: "arguments"|"reason"|"tool_name" = "arguments"): Promise<void> {
    const session=this.#session, overlay=this.#hitlOverlay;
    if(!session || !overlay) return;
    const rows=this.#interactionPage?.items ?? session.state.pendingInteractions;
    const index=rows.findIndex(item=>sameInteractionRef(item.interaction,interaction));
    if(index<0) return;
    const item=rows[index]!;
    if(hasInteractionBody(item) || item.kind!=='approval') return;
    const stamp=this.#interactionStamp(), old=this.#interactionArgumentRange;
    const previous=old?.session===session && old.stamp===stamp && sameInteractionRef(old.interaction,interaction) ? old : undefined;
    const offset=next ? previous?.next : 0;
    const field=next ? previous?.field ?? selectedField : selectedField;
    if(offset==null) return;
    const lease=this.#presentationLease(), generation=++this.#interactionBodyRead;
    const current=()=>this.#hitlOverlay===overlay && this.#isCurrentPresentationLease(lease) && generation===this.#interactionBodyRead && stamp===this.#interactionStamp() && sameInteractionRef(this.#interactionFocus ?? interaction,interaction);
    try {
      const chunk=await session.interactionValue(interaction,Number(this.#interactionPage?.page.items[index]?.offset ?? item.source_offset),this.#interactionPage?.page,offset,['request','kind',field]);
      if(!current()) return;
      if(chunk.bytes_base64.length>87_384 || chunk.bytes_base64.length%4!==0 || chunk.bytes_base64.length/4*3-(chunk.bytes_base64.endsWith('==')?2:chunk.bytes_base64.endsWith('=')?1:0)>65_536)throw new Error('Encoded native range exceeds the decoded byte bound.');
      const bytes=Uint8Array.from(atob(chunk.bytes_base64),character=>character.charCodeAt(0));
      if(bytes.length>65536 || !Number.isSafeInteger(chunk.readable_end) || offset+bytes.length>chunk.readable_end || (offset<chunk.readable_end && !bytes.length) || (chunk.next_offset ?? null)!==(offset+bytes.length<chunk.readable_end?offset+bytes.length:null) || (previous && next && previous.end!==chunk.readable_end)) throw new Error('Argument range frontier changed.');
      const decoder=previous && next ? previous.decoder : new TextDecoder('utf-8',{fatal:true,ignoreBOM:true});
      this.#interactionArgumentRange={session,stamp,interaction,text:decoder.decode(bytes,{stream:chunk.next_offset!=null}),offset,field,next:chunk.next_offset??null,end:chunk.readable_end,decoder};
      this.#tui.requestRender();
    } catch(error){if(current())this.#showTransient('error',`Arguments unavailable: ${compactDiagnostic(error)}`);}
  }

  async #loadInteractionBody(interaction: InteractionRef): Promise<void> {
    const session=this.#session, overlay=this.#hitlOverlay;
    if(!session || !overlay) return;
    const rows=this.#interactionPage?.items ?? session.state.pendingInteractions;
    const index=rows.findIndex(item=>sameInteractionRef(item.interaction,interaction));
    if(index<0 || hasInteractionBody(rows[index]!)) return;
    const selected=rows[index]!;
    if(hasInteractionBody(selected))return;
    const ordinal=Number(this.#interactionPage?.page.items[index]?.offset ?? selected.source_offset);
    const lease=this.#presentationLease(), stamp=this.#interactionStamp(), generation=++this.#interactionBodyRead;
    const current=()=>this.#hitlOverlay===overlay && this.#isCurrentPresentationLease(lease) && generation===this.#interactionBodyRead && stamp===this.#interactionStamp() && sameInteractionRef(this.#interactionFocus ?? interaction,interaction);
    try {
      let offset=0, total:number|undefined; const bytes:Uint8Array[]=[];
      do {
        const chunk=await session.interactionValue(interaction,ordinal,this.#interactionPage?.page,offset);
        if(!current()) return;
        if(!Number.isSafeInteger(chunk.readable_end) || chunk.readable_end>16*1024*1024 || (total!==undefined && total!==chunk.readable_end)) throw new Error('This body exceeds the selected-body cache; inspect bounded argument ranges.');
        total=chunk.readable_end;
        if(chunk.bytes_base64.length>87_384 || chunk.bytes_base64.length%4!==0 || chunk.bytes_base64.length/4*3-(chunk.bytes_base64.endsWith('==')?2:chunk.bytes_base64.endsWith('=')?1:0)>65_536)throw new Error('Encoded native range exceeds the decoded byte bound.');
        const value=Uint8Array.from(atob(chunk.bytes_base64),character=>character.charCodeAt(0));
        if(value.length>65536 || (offset<total && !value.length) || offset+value.length>total || (chunk.next_offset ?? null)!==(offset+value.length<total?offset+value.length:null)) throw new Error('Interaction range did not advance exactly.');
        bytes.push(value);offset+=value.length;
      } while(offset<total);
      const data=new Uint8Array(total);let at=0;for(const value of bytes){data.set(value,at);at+=value.length;}
      const body=JSON.parse(new TextDecoder('utf-8',{fatal:true,ignoreBOM:true}).decode(data)) as RoutedInteraction;
      const header=rows[index]!;
      if(hasInteractionBody(header)) return;
      validateInteractionBody(header,body);
      this.#interactionBody={session,stamp,interaction,body};this.#renderState(session.state);
    } catch(error) {if(current())this.#showTransient('error',`Interaction body unavailable: ${compactDiagnostic(error)}`);}
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
        !this.#interactionPartial() && findPendingInteraction(state.pendingInteractions, dismissed.interaction) === undefined
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
      existing.update(state.pendingInteractions, focused, this.#preferences, !this.#interactionPartial());
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
            (this.#session?.state?.pendingInteractions ?? []).map((entry) =>
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
        const current = this.#session?.state;
        if (current !== undefined) this.#renderState(current);
      },
      onToggleExpand: (interaction) => {
        // Disclosure only: the same preference domain `/expand interaction`
        // uses, never a second approval gate.
        this.#applyPreference({ type: "expand_interaction", interaction });
      },
      onChange: () => this.#tui.requestRender(),
      onPage: (direction) => { void this.#pageInteractions(direction); },
      pageLabel: () => this.#interactionPageLabel(),
      onLoadBody: (interaction) => { void this.#loadInteractionBody(interaction); },
      onArguments: (interaction,next,field) => {void this.#readInteractionArguments(interaction,next,field);},
      argumentRange: (interaction) => {const range=this.#interactionArgumentRange;return range && range.session===this.#session && range.stamp===this.#interactionStamp() && sameInteractionRef(range.interaction,interaction) ? {label:`${range.field} bytes ${range.offset}–${range.next ?? range.end} of ${range.end}`,text:range.text}:undefined;},
    });
    overlay.update(state.pendingInteractions, focused, this.#preferences, !this.#interactionPartial());
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
    const attachedSession = lease.session;
    if (attachedSession === undefined) return;
    void attachedSession
      .respondInteraction(interaction, response)
      .then(() => {
        if (this.#isCurrentPresentationLease(lease) && this.#interactionPartial()) {void attachedSession.resync().catch(error=>{if(this.#isCurrentPresentationLease(lease))this.#showTransient('error',`Interaction observation requires recovery: ${compactDiagnostic(error)}`);});}
      })
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
    return this.#session === undefined ? undefined : { conversationId: this.#session.target.conversation_id };
  }

  #diagnostics(): DebugDiagnostics {
    const stderr = this.#host.stderrTail();
    const exit = this.#host.childExit;
    const target = this.#session?.target;
    return {
      connection: this.#host.describe(),
      // Ownership is established by who spawned the process, never inferred
      // from the endpoint, the Session, or the transport.
      ownership:
        this.#host.ownership === "owned_child"
          ? "this client owns the App Server process and ends it on exit"
          : "an external owner runs the App Server; exiting only disconnects",
      sessionId: target?.session_id,
      attachmentId: target?.attachment_id,
      conversationId: target?.conversation_id,
      runtimeIncarnation: target?.runtime_incarnation,
      cursor: this.#session?.state.cursor,
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
      resyncCount: this.#session?.resyncCount ?? 0,
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

/** One bounded line for a failed or cancelled delivery action. */
function deliveryFailure(error: unknown, signal: AbortSignal, effect: LocalEffect): string {
  if (error instanceof DeliveryResidueError || error instanceof DeliveryUncertainError) return error.message;
  // Cancellation won before Save's publication admission, Open's launch, or
  // the server's response publication. A committed effect is never
  // described as nothing having happened, whatever aborted afterwards.
  if (!effect.committed && ((signal.aborted && error === signal.reason) || isDeliveryCancelled(error))) {
    return "Cancelled; nothing was saved or opened";
  }
  if (error instanceof TransportClosedError) return "Disconnected from the App Server; nothing was written";
  if (error instanceof AppServerRequestError && error.error.data?.kind === "session_file_read") {
    const reason = error.error.data.reason;
    const text: Record<typeof reason, string> = {
      missing: "the delivered file no longer exists",
      unauthorized: "this connection is not authorized to access it",
      unavailable: "its original Session or root is unavailable",
      not_regular: "it is no longer a regular file",
      replaced: "it was replaced during the operation; try again",
      too_large: "it exceeds the 512 KiB transfer limit",
      capacity: "the App Server is busy with other file reads; try again",
      read_failed: "the App Server could not read it",
    };
    return `Failed: ${text[reason]}`;
  }
  return `Failed: ${compactDiagnostic(error)}`;
}
