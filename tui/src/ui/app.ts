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
import type { ConnectingSession, SessionCatalogPage } from "../startup.ts";
import {
  ProcessTerminal,
  Spacer,
  TUI,
  matchesKey,
  type OverlayHandle,
  type SizeValue,
} from "@earendil-works/pi-tui";

import { SlashCommandAutocompleteProvider } from "../commands/autocomplete.ts";
import { CommandDispatcher } from "../commands/dispatcher.ts";
import type { ExpandTarget, PreferenceChange } from "../commands/outcome.ts";
import type { DebugDiagnostics } from "../commands/presentation.ts";
import { sessionLabel } from "../presentation/selectors.ts";
import { correlateTools } from "../presentation/tools.ts";
import type { PresentationState } from "../presentation/state.ts";
import type { AppServerHost } from "../app-server/host.ts";
import type { AppServerSession } from "../app-server/session.ts";
import type {
  SessionSettings,
  SessionDeleteResult,
  SessionView,
  ToolCallId,
  InteractionRef,
  UserInputBlock,
} from "../protocol/app-server.ts";
import { launchOpener, systemOpener, type Opener } from "../app-server/delivery-files.ts";
import {
  renderSubagentDetail,
} from "./components/activity.ts";
import {
  cycleSubagentSelection,
  hasSubagentSelection,
} from "./subagent-navigation.ts";
import { SelectorSurfaces } from "./selectors.ts";
import { compactDiagnostic } from "../diagnostic-text.ts";
import { InspectionView } from "./components/inspection-view.ts";
import { SessionDeletionWorkflow } from "./session-deletion-workflow.ts";
import type { ResumeSelector } from "./components/resume-selector.ts";
import { ConfirmationView } from "./components/confirmation.ts";
import { PopupFrame, type PopupContent } from "./components/popup-frame.ts";
import { TransientFeedbackSurface } from "./components/transient-feedback.ts";
import { SourceRepairStatus } from "./components/source-repair-status.ts";
import { FooterView } from "./components/status.ts";
import type { ConversationContext } from "./components/status.ts";
import { renderResourceBanner } from "./components/resources.ts";
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
import { SubagentTranscript } from "../app-server/subagent-transcript.ts";
import { SubagentTranscriptView } from "./components/subagent-transcript-view.ts";
import { HumanInputController } from "./human-input.ts";
import { ConversationView } from "./conversation-view.ts";
import type { PresentationLease } from "./presentation-lease.ts";

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

type PendingSessionConnection = ConnectingSession & {
  /** Observed native state while final repair is pending; not dispatch authority. */
  prepared?: AppServerSession;
  queued?: { content: UserInputBlock[]; queue: boolean };
  history?: PresentationState;
  intent?: Promise<void>;
};

export class RustxTuiApp {
  #host: AppServerHost;
  readonly #reconnect: (() => Promise<AppServerHost>) | undefined;
  #recovering = false;
  #session: AppServerSession | undefined;
  readonly #dispatcher: CommandDispatcher;
  #initialConnection: ConnectingSession | undefined;
  #connecting: PendingSessionConnection | undefined;
  #initialResumePage: SessionCatalogPage | undefined;
  readonly #workspace: string | undefined;
  readonly #opener: { command: string | undefined; launch: Opener };

  readonly #tui: TUI;
  readonly #view: ConversationView;
  readonly #transient = new TransientFeedbackSurface();
  /** Newly observed configuration repairs of the current App Server
   * authority; presentation only (Issue #464). */
  readonly #repairStatus = new SourceRepairStatus();
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

  #preferences: PresentationPreferences = defaultPreferences();
  #overlay: OverlayHandle | undefined;
  /** Retires the work the current overlay owns when it closes or is replaced. */
  #overlayRetire: (() => void) | undefined;
  readonly #humanInput = new HumanInputController({
    session: () => this.#session,
    lease: () => this.#presentationLease(),
    isCurrent: lease => this.#isCurrentPresentationLease(lease),
    render: state => this.#renderState(state),
    requestRender: () => this.#tui.requestRender(),
    showTransient: (level, text) => this.#showTransient(level, text),
    showPopup: (content, options) => { this.#showPopup(content, options); },
    closeOverlay: () => this.#closeOverlay(),
    interrupt: () => void this.#onInterrupt(),
    applyPreference: change => this.#applyPreference(change),
    preferences: () => this.#preferences,
  });
  readonly #selectors: SelectorSurfaces;
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
    this.#selectors = new SelectorSurfaces({
      lease: () => this.#presentationLease(),
      isCurrent: lease => this.#isCurrentPresentationLease(lease),
      host: () => this.#host,
      session: () => this.#session,
      dispatcher: () => this.#dispatcher,
      deletion: () => this.#deletion,
      switching: () => this.#switching,
      overlay: () => this.#overlay,
      humanInputOverlay: () => this.#humanInput.overlay,
      showPopup: (content, options, retire) => this.#showPopup(content, options, retire),
      closeOverlay: () => this.#closeOverlay(),
      requestRender: () => this.#tui.requestRender(),
      showTransient: (level, text) => this.#showTransient(level, text),
      showInspection: (title, body, lease) => this.#showInspection(title, body, lease),
      handleOutcome: (outcome, lease) => this.#handleOutcome(outcome, lease),
      presentResume: selector => { this.#resumePresentation = selector; },
    }, this.#opener);

    this.#tui = new TUI(new ProcessTerminal());
    this.#editor = new ComposerEditor(this.#tui);
    this.#editor.setAutocompleteProvider(new SlashCommandAutocompleteProvider());
    this.#editor.onChange = () => { this.#draft.text = this.#editor.getExpandedText(); };
    this.#editor.running = () => {
      const observed = this.#session ?? this.#connecting?.prepared;
      return observed !== undefined && isAttemptActive(observed.state);
    };
    this.#editor.onPrompt = (text, commands) => { void this.#onSubmit(text, false, commands); };
    this.#editor.onQueue = text => { void this.#onSubmit(text, true, false); };
    this.#view = new ConversationView(this.#tui);

    this.#dispatcher = new CommandDispatcher({
      host: this.#host,
      session: this.#session,
      sessionSettings: options.sessionSettings,
      diagnostics: () => this.#diagnostics(),
    });

    this.#tui.addChild(this.#view.startup);
    this.#tui.addChild(this.#view.transcript);
    this.#tui.addChild(this.#view.activity);
    this.#tui.addChild(this.#transient);
    this.#tui.addChild(this.#repairStatus);
    this.#tui.addChild(this.#view.todos);
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
    this.#repairStatus.watch(this.#host, host => this.#host === host && !this.#quitting && !this.#finished, () => { if (this.#started) this.#tui.requestRender(); });
  }

  #watchConnection(): void {
    this.#removeConnectionListener();
    const host = this.#host;
    this.#removeConnectionListener = host.client.onClose((error) => {
      if (this.#host !== host || this.#quitting || this.#finished) return;
      const focused = this.#session ?? this.#connecting?.prepared;
      if (this.#connecting) {
        const queued = this.#connecting.queued;
        this.#connecting.queued = undefined;
        this.#retainConnectingInput(queued?.content);
        this.#connecting = undefined; this.#view.stopWorking();
      }
      this.#deletion?.terminate();
      this.#initialResumePage = undefined;
      this.#reconnectChild = this.#childInspection?.reader.selected;
      this.#invalidatePresentation();
      this.#editor.disableSubmit = true;
      if (host.ownership === "external" && this.#reconnect !== undefined) {
        void this.#recoverConnection(focused);
      } else {
        void host.shutdown().finally(() => this.#showTerminalFailureAndFinish(
          `${compactDiagnostic(error)}\nApp Server connection/process failure. Runtime outcomes are unknown.`, 1,
        ));
      }
    });
  }

  async #recoverConnection(focused = this.#session): Promise<void> {
    if (this.#recovering || this.#reconnect === undefined || this.#quitting || this.#finished) return;
    this.#recovering = true;
    this.#switching = true;
    const oldHost = this.#host;
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
      if (this.#quitting || this.#finished || this.#host !== oldHost) {
        await replacement.shutdown();
        return;
      }
      if (replacement.client.closed !== undefined) throw replacement.client.closed;
      // A negotiated replacement can receive a fresh submission intent while
      // its known Session is still preparing. Retire the old presentation
      // before entering the ordinary editable connecting owner; never keep the
      // closed Host installed behind an MCP-dependent attachment wait.
      this.#host = replacement;
      this.#dispatcher.setHost(replacement);
      // Reconnection keeps the same logical draft. Its cached Session entry
      // and editor retain edits, paste provenance and submission clearing while
      // attachment is pending; unbinding presentation must not replace text.
      this.#bindSession(undefined, this.#draft);
      this.#watchConnection();
      this.#watchSourceRepairs();
      if (focus !== undefined) {
        this.#switching = false;
        await this.#focusSession(focus.sessionId, focus.nodeId, undefined, undefined, this.#presentationLease(), false, focus);
        if (this.#quitting || this.#finished || this.#host !== replacement) return;
        if (replacement.client.closed !== undefined) throw replacement.client.closed;
        // Attachment failure already retained input and reported its exact
        // outcome. Transport readiness alone cannot announce recovered focus.
        if (this.#session === undefined) { this.#reconnectChild = undefined; return; }
      }
      const session = this.#session;
      if (session !== undefined && this.#reconnectChild !== undefined) this.#openChildTranscript(this.#reconnectChild);
      this.#reconnectChild = undefined;
      if (session !== undefined) this.#renderState(session.state);
      else if (this.#started) await this.#selectors.openResumeSelector();
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
  #bindSession(session: AppServerSession | undefined, retainedDraft?: ComposerDraft): void {
    this.#captureDraft();
    if (this.#session !== undefined) this.#drafts.set(`${this.#session.sessionId}:${this.#session.target.conversation_id}`, this.#draft);
    const key = session === undefined ? undefined : `${session.sessionId}:${session.target.conversation_id}`;
    const draft = retainedDraft ?? (key === undefined ? new ComposerDraft() : this.#drafts.get(key) ?? new ComposerDraft());
    this.#activateDraft(draft);
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

  #captureDraft(): void {
    this.#draft.text = this.#editor.getExpandedText();
    this.#draft.provenance = this.#editor.captureProvenance();
  }

  /** Switching presentation never turns a retained message into a command. */
  #activateDraft(draft: ComposerDraft): void {
    if (draft === this.#draft) return;
    this.#captureDraft();
    this.#draft = draft;
    this.#editor.restoreDraftText(draft.text, draft.provenance);
    this.#refreshDraftLabel();
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
    previous?: AppServerSession,
    retryInitialization = false,
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
    this.#captureDraft();
    const host = this.#host;
    // Selection owns subsequent edits before durable metadata can resolve its
    // node. Unbinding saves the old complete draft under its actual target and
    // removes its submission authority while this target is not yet attached.
    const pendingKey = `pending:${sessionId}`;
    const targetConversation = previous?.target.conversation_id;
    // Prepared transport state is not proof that this pending draft transferred.
    // Keep its responsibility until attach confirms the exact Conversation.
    const targetDraft = previous !== undefined ? previousDraft : (changingNode ? undefined : this.#drafts.get(pendingKey)) ?? new ComposerDraft();
    this.#bindSession(undefined, targetDraft);
    lease = this.#presentationLease();
    // Only never-bound input uses this key. Previously bound drafts remain
    // under their exact Conversation key, including after a refused reconnect.
    if (targetConversation === undefined) this.#drafts.set(pendingKey, targetDraft);
    const connection: PendingSessionConnection = { sessionId };
    this.#connecting = connection;
    const beginConnecting = (nodeId: string) => {
      if (!this.#isCurrentPresentationLease(lease) || this.#host !== host) return;
      connection.nodeId = nodeId;
      this.#editor.disableSubmit = false;
      this.#renderConnecting();
      void host.client.call("session/history", { session_id: sessionId, node_id: nodeId, at: { type: "latest" }, limit: 32 }, "session_history").then(result => {
        if (this.#connecting !== connection || this.#host !== host || this.#finished) return;
        connection.history = historyPresentation(result.conversation_id, result.window.page);
        this.#renderConnecting();
      }).catch(() => {});
    };
    this.#editor.disableSubmit = true;
    this.#renderConnecting();
    let installed: AppServerSession | undefined;
    try {
      // Resolve cold navigation through durable metadata before any native
      // preparation wait. Attach and first Send must name the same exact node.
      const selectedNode = nodeId ?? (await host.readSession(sessionId)).active_node;
      if (!this.#isCurrentPresentationLease(lease) || this.#host !== host) return;
      if (retryInitialization) {
        await host.retryInitialization(sessionId, selectedNode);
        if (!this.#isCurrentPresentationLease(lease) || this.#host !== host) return;
      }
      if (!changingNode) beginConnecting(selectedNode);
      const next = changingNode
        ? await host.openNode(sessionId, selectedNode, () => beginConnecting(selectedNode))
        : await host.attach(sessionId, selectedNode, previous);
      if (!this.#isCurrentPresentationLease(lease) || this.#host !== host) return;
      if (targetConversation !== undefined && next.target.conversation_id !== targetConversation) {
        throw new Error('The attached Conversation changed. Original input remains in its previous Conversation draft; select the current target explicitly.');
      }
      connection.prepared = next;
      // Keep cached P separate from the selected draft's queued Q / newer R
      // until every asynchronous repair and intent fence has settled. A refusal
      // or withdrawal therefore restores Q onto R before the one final join.
      await next.resync();
      // The first Send can arm native preparation while attach is pending.
      // Its acknowledgement also fences forwarding the captured input blocks.
      while (connection.intent) {
        const intent = connection.intent;
        await intent;
        if (connection.intent === intent) break;
      }
      if (!this.#isCurrentPresentationLease(lease) || this.#host !== host || this.#connecting !== connection || host.client.closed !== undefined) return;
      if (next.released || next.serverClosed || host.attachment(sessionId) !== next) {
        throw new Error("The prepared Session connection closed before focus could be installed.");
      }
      let queued = connection.queued;
      let refused = false;
      if (queued !== undefined && (this.#submitting || this.#uploading || !next.controlReady)) {
        refused = true;
        connection.queued = undefined;
        this.#retainConnectingInput(queued.content);
        queued = undefined;
      }
      this.#captureDraft();
      const key = `${sessionId}:${next.target.conversation_id}`;
      const cached = this.#drafts.get(key);
      const boundDraft = cached === undefined ? targetDraft : cached.combine(targetDraft);
      this.#bindSession(next, boundDraft);
      this.#drafts.set(key, this.#draft);
      if (this.#drafts.get(pendingKey) === targetDraft) this.#drafts.delete(pendingKey);
      installed = next;
      if (editorContent !== undefined && !this.#editor.getExpandedText()) {
        this.#draft.restore(editorContent);
        this.#editor.restoreDraftText(this.#draft.text, this.#draft.provenance);
      }
      this.#connecting = undefined;
      this.#switching = false;
      if (queued !== undefined) {
        connection.queued = undefined;
        void this.#submitInput(next, queued.content, queued.queue);
      }
      this.#refreshDraftLabel();
      if (refused) this.#showTransient("error", "Session is not ready to submit. Input retained; nothing sent.");
      else this.#showTransient("info", notice ?? `showing session ${sessionId}`);
      this.#renderState(next.state);
    } catch (error: unknown) {
      if (this.#finished || this.#quitting || this.#host !== host ||
          (installed === undefined ? !this.#isCurrentPresentationLease(lease) : this.#session !== installed)) return;
      const queued = connection.queued;
      connection.queued = undefined;
      this.#retainConnectingInput(queued?.content);
      this.#connecting = undefined;
      this.#view.stopWorking();
      if (this.#session) this.#renderState(this.#session.state);
      else {
        this.#view.notice("Connection failed. Press Enter to choose a Session. Your input has been retained.");
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
        this.#view.notice("Choose a Session to resume. Enter reopens the picker; Ctrl+C exits.", false);
        const page = this.#initialResumePage;
        this.#initialResumePage = undefined;
        void this.#selectors.openResumeSelector(page);
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
            this.#retainConnectingInput(queued?.content);
            this.#editor.disableSubmit = this.#connecting.nodeId === undefined;
            this.#renderConnecting();
            return { consume: true };
          }
          return undefined;
        }
        if (this.#session === undefined) {
          if (matchesKey(data, "ctrl+c")) { void this.quit(); return { consume: true }; }
          if (this.#overlay !== undefined) return undefined;
          if (matchesKey(data, "ctrl+r") || matchesKey(data, "enter")) void this.#selectors.openResumeSelector();
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
          if (this.#humanInput.reopen()) {
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
          if (this.#overlay !== undefined && this.#humanInput.overlay === undefined) return undefined;
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
      const nodeId = this.#connecting.nodeId;
      if (nodeId === undefined) {
        this.#editor.restoreDraftText(text, this.#editor.captureProvenance());
        return;
      }
      if (!text.trim() && !this.#draft.blocks?.length) return;
      if (commands && text.trim().startsWith("/")) { this.#editor.setText(text); return; }
      if (this.#connecting.queued !== undefined) { this.#editor.setText(text); return; }
      if (this.#submitting || this.#uploading) { this.#editor.setText(text); return; }
      let content: UserInputBlock[];
      try { content = structuredClone(this.#draft.submission(text)); }
      catch (error) { this.#editor.setText(text); this.#showTransient("error", compactDiagnostic(error)); return; }
      const connection = this.#connecting;
      const host = this.#host;
      connection.queued = { content, queue };
      // Transfer the complete ordered draft once, including confirmed receipts.
      // Later edits belong to the now-empty draft, not this queued submission.
      this.#draft.clear();
      const intent = host.submissionIntent(connection.sessionId, nodeId).catch(error => {
        if (connection.intent !== intent || this.#connecting !== connection || this.#host !== host || this.#finished) return;
        const retained = connection.queued;
        connection.queued = undefined;
        this.#retainConnectingInput(retained?.content);
        this.#editor.disableSubmit = false;
        this.#renderConnecting();
        this.#showTransient('error', `could not prepare submission: ${compactDiagnostic(error)} · Input retained; nothing sent.`);
      });
      connection.intent = intent;
      this.#editor.setText("");
      this.#refreshDraftLabel();
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
      try { content = structuredClone(this.#draft.submission(text)); }
      catch (error) { this.#editor.setText(text); this.#showTransient("error", compactDiagnostic(error)); return; }
      // Transfer this draft once. New typing cannot be cleared by a late result.
      this.#editor.setText(""); this.#draft.clear(); this.#refreshDraftLabel();
      await this.#submitInput(session, content, queue);
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

  /** Dispatches already captured input without touching the current draft. */
  async #submitInput(session: AppServerSession, content: UserInputBlock[], queue: boolean): Promise<void> {
    const lease = this.#presentationLease();
    const intent = composerIntent(session.state, queue);
    const prompt = editorText(content);
    if (prompt) this.#promptHistory.push(prompt);
    if (this.#promptHistory.length > 1000) this.#promptHistory.shift();
    this.#editor.addToHistory(prompt);
    this.#submitting = true;
    try { if (intent === "steer") await session.steer(content); else await session.submitInbound(content); }
    catch (error) { if (this.#isCurrentPresentationLease(lease)) this.#showTransient("error", `${intent}: ${compactDiagnostic(error)} · Not replayed. Prompt remains in local history.`); }
    finally { this.#submitting = false; }
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
        if (!stat.isFile()) throw new Error("Select a regular file.");
        bytes = await file.readFile();
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
        this.#selectors.showModelSelector(outcome.models, lease);
        break;
      case "choose_session":
        this.#selectors.showSessionSelector(outcome.sessions, outcome.nextOffset, outcome.query, lease);
        break;
      case "choose_fork":
        this.#selectors.showBoundarySelector(
          outcome.boundaries,
          "Fork from user message",
          "fork",
          lease,
          outcome.nextOffset,
        );
        break;
      case "choose_tree":
        this.#selectors.showTreeSelector(
          outcome.session,
          outcome.nodes,
          outcome.nextNodeOffset,
          outcome.boundaries,
          outcome.nextHistoryOffset,
          lease,
        );
        break;
      case "choose_delivery":
        this.#selectors.showDeliverySelector(outcome.page, lease);
        break;
      case "focus_session":
        await this.#focusSession(
          outcome.sessionId,
          outcome.nodeId,
          outcome.editorContent,
          outcome.notice,
          lease,
          false,
          undefined,
          outcome.retryInitialization === true,
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
    if (this.#humanInput.overlay !== undefined && this.#session?.observationPhase === 'following') {
      // Escape behavior is defined per interaction kind by the surface
      // itself: a questionnaire gets its explicit typed decline; an approval
      // is dismissed without any answer. The app never answers here.
      this.#humanInput.overlay.escape();
      return;
    }
    if (this.#overlay !== undefined && this.#humanInput.overlay === undefined) {
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
        this.#view.clear();
        if (!next) void this.#selectors.openResumeSelector();
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
    this.#selectors.showSessionSelector(page?.sessions, page?.nextOffset,
      query, this.#presentationLease());
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
    this.#humanInput.overlayClosed();
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
    this.#humanInput.reset();
    this.#transient.clear();
    if (this.#started) {
      this.#tui.requestRender();
    }
  }

  #retainConnectingInput(content: UserInputBlock[] | undefined): void {
    if (content === undefined) return;
    const text = editorText(content);
    this.#promptHistory.push(text);
    if (this.#promptHistory.length > 1000) this.#promptHistory.shift();
    this.#editor.addToHistory(text);
    this.#captureDraft();
    this.#draft.prepend(content);
    this.#editor.restoreDraftText(this.#draft.text, this.#draft.provenance);
    this.#refreshDraftLabel();
  }

  #renderConnecting(): void {
    const queued = this.#connecting?.queued?.content.map(block => block.type === "text" ? block.text : "[attachment]").join("");
    this.#view.renderConnecting(this.#connecting?.history, queued, this.#preferences);
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
    state = this.#humanInput.project(state);
    if (!hasSubagentSelection(state.agents, this.#selectedAgentId)) {
      this.#selectedAgentId = undefined;
      this.#subagentListFocused = false;
    }
    this.#view.render(state, {
      preferences: this.#preferences,
      sessionInfo: this.#sessionInfo,
      workspace: this.#workspace,
      selectedAgentId: this.#subagentListFocused ? this.#selectedAgentId : undefined,
      interactionFocus: this.#humanInput.focus,
      collections: this.#session?.checkpoint?.collections,
    });
    this.#humanInput.sync(state);
    this.#syncDeletionPresentation();
    this.#tui.requestRender();
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
    this.#view.stop();
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

function nextTick(): Promise<void> {
  return new Promise((resolve) => process.nextTick(resolve));
}
