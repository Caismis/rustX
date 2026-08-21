/**
 * The client-facing attachment lifecycle.
 *
 * ```text
 * spawn -> initialize(v1) -> authoritative snapshot + cursor
 *       -> install presentation projection
 *       -> subscribe_events(after cursor)
 *       -> interactive
 * ```
 *
 * This owner sequences attachment, snapshot/cursor installation, subscription,
 * resync repair, and shutdown. It owns no agent semantics: it starts nothing,
 * settles nothing, and interprets no model, tool, or capability value.
 *
 * # Resync
 *
 * `resync_required` means the incremental projection can no longer be
 * trusted. The response is never to guess at the gap:
 *
 * ```text
 * incremental state no longer trusted
 *   -> snapshot_get
 *   -> replace the presentation projection wholesale
 *   -> subscribe after the new cursor
 *   -> continue
 * ```
 *
 * The snapshot is authoritative. Nothing is replayed from what the UI thought
 * happened.
 */

import {
  ConnectionClosedError,
  RuntimeClientConnection,
  RuntimeRequestError,
} from "./connection.ts";
import {
  reduce,
  replaceFromSnapshot,
} from "../presentation/projection.ts";
import type { PresentationState } from "../presentation/state.ts";
import {
  RUNTIME_CLIENT_PROTOCOL_VERSION_V1,
  type AgentId,
  type CapabilityView,
  type ConversationId,
  type ModelCatalogView,
  type RuntimeClientBackgroundExecution,
  type RuntimeClientCursor,
  type RuntimeClientProtocolEvent,
  type RuntimeClientResult,
  type RuntimeClientSnapshot,
  type RuntimeClientOutcome,
  type SessionSummaryView,
  type SessionNodeView,
  type SessionUserMessageBoundaryView,
  type SessionView,
  type SessionNodeId,
  type SessionId,
  type SurfaceRevision,
  type InteractionId,
  type InteractionResponse,
  type SessionModelConfig,
  type SessionModelView,
  type ToolExecutionId,
  type UserContentBlock,
} from "../protocol/types.ts";

/** Native owner bound for one Session list/tree response. */
export const SESSION_PROJECTION_PAGE_LIMIT = 32;

export interface RuntimeClientAttachmentOptions {
  connection: RuntimeClientConnection;
}

/** The runtime identities the attachment reported. */
export interface AttachmentIdentity {
  attachmentId: string;
  conversationId: ConversationId;
  agentId: AgentId;
}

export interface SessionSwitch {
  session: SessionView;
  editorContent?: UserContentBlock[];
  restartRequired: boolean;
}

type StateListener = (state: PresentationState) => void;

export class RuntimeClientAttachment {
  readonly #connection: RuntimeClientConnection;
  readonly #listeners = new Set<StateListener>();
  #state: PresentationState | undefined;
  #identity: AttachmentIdentity | undefined;
  #sessionInfo: SessionView | undefined;
  #resyncCount = 0;
  #detachedEvents: (() => void) | undefined;

  constructor(options: RuntimeClientAttachmentOptions) {
    this.#connection = options.connection;
  }

  /** The current presentation state, once attached. */
  get state(): PresentationState | undefined {
    return this.#state;
  }

  get identity(): AttachmentIdentity | undefined {
    return this.#identity;
  }

  /** The native product Session projection, once attached. */
  get sessionInfo(): SessionView | undefined {
    return this.#sessionInfo;
  }

  /** How many authoritative repairs this attachment has performed. */
  get resyncCount(): number {
    return this.#resyncCount;
  }

  /** Subscribes to presentation state changes. */
  onState(listener: StateListener): () => void {
    this.#listeners.add(listener);
    return () => this.#listeners.delete(listener);
  }

  /**
   * Negotiates the protocol, installs the authoritative snapshot, and
   * subscribes from that exact cursor.
   *
   * Events are buffered from the moment the subscription request is issued so
   * an event published between `initialize` and `subscribe_events` is folded
   * in order rather than dropped.
   */
  async attach(): Promise<AttachmentIdentity> {
    const result = await this.#connection.request({
      method: "initialize",
      protocol_version: RUNTIME_CLIENT_PROTOCOL_VERSION_V1,
    });
    if (result.type !== "initialized") {
      throw new Error(`initialize returned ${result.type}`);
    }

    this.#identity = {
      attachmentId: result.attachment_id,
      conversationId: result.conversation_id,
      agentId: result.agent_id,
    };
    this.#install(result.snapshot, result.cursor);
    await this.#subscribe(result.cursor);
    return this.#identity;
  }

  /** Submits one inbound message. Acceptance, never completion. */
  async submitInbound(
    content: UserContentBlock[],
  ): Promise<{ messageId: string; sequence: number }> {
    const result = await this.#connection.request({
      method: "submit_inbound",
      content,
    });
    if (result.type !== "inbound_accepted") {
      throw new Error(`submit_inbound returned ${result.type}`);
    }
    return {
      messageId: result.message_id,
      sequence: result.inbound_sequence,
    };
  }

  /** Requests cancellation of the current attempt. Acceptance, not settlement. */
  async cancelCurrentAttempt(): Promise<string> {
    const result = await this.#connection.request({
      method: "cancel_current_attempt",
    });
    if (result.type !== "attempt_cancellation_accepted") {
      throw new Error(`cancel_current_attempt returned ${result.type}`);
    }
    return result.attempt_id;
  }

  /** Answers one runtime-owned approval interaction. */
  async respondInteraction(
    interactionId: InteractionId,
    response: InteractionResponse,
  ): Promise<void> {
    const result = await this.#connection.request({
      method: "interaction_respond",
      interaction_id: interactionId,
      response,
    });
    if (result.type !== "interaction_response_accepted") {
      throw new Error(`interaction_respond returned ${result.type}`);
    }
  }

  /**
   * Requests cancellation of one background execution.
   *
   * The returned registry snapshot is *acceptance*. The terminal fact arrives
   * later on the event stream, and only the runtime decides it.
   */
  async cancelBackground(
    executionId: ToolExecutionId,
  ): Promise<RuntimeClientBackgroundExecution> {
    const result = await this.#connection.request({
      method: "background_cancel",
      execution_id: executionId,
    });
    if (result.type !== "background_cancel_accepted") {
      throw new Error(`background_cancel returned ${result.type}`);
    }
    return result.execution;
  }

  async backgroundStatus(
    executionId: ToolExecutionId,
  ): Promise<RuntimeClientBackgroundExecution> {
    const result = await this.#connection.request({
      method: "background_status",
      execution_id: executionId,
    });
    if (result.type !== "background_status") {
      throw new Error(`background_status returned ${result.type}`);
    }
    return result.execution;
  }

  /** The safe public catalog. This is why the client never reads models.json. */
  async modelCatalog(): Promise<ModelCatalogView> {
    const result = await this.#connection.request({
      method: "model_catalog_get",
    });
    if (result.type !== "model_catalog") {
      throw new Error(`model_catalog_get returned ${result.type}`);
    }
    return result.catalog;
  }

  async modelGet(): Promise<SessionModelView> {
    const result = await this.#connection.request({ method: "model_get" });
    if (result.type !== "model") {
      throw new Error(`model_get returned ${result.type}`);
    }
    return result.model;
  }

  /**
   * Replaces the authoritative session model configuration.
   *
   * A whole-state replacement, never a patch: callers send back the complete
   * configuration they read. The update affects future admissions only; an
   * already-admitted attempt keeps the model it froze.
   */
  async modelSet(config: SessionModelConfig): Promise<SessionModelView> {
    const result = await this.#connection.request({
      method: "model_set",
      config,
    });
    if (result.type !== "model_set") {
      throw new Error(`model_set returned ${result.type}`);
    }
    return result.model;
  }

  async capabilityGet(): Promise<CapabilityView> {
    const result = await this.#connection.request({ method: "capability_get" });
    if (result.type !== "capability") {
      throw new Error(`capability_get returned ${result.type}`);
    }
    return result.capabilities;
  }

  /** Takes a fresh authoritative snapshot and replaces the projection. */
  async resync(): Promise<void> {
    const result = await this.#connection.request({ method: "snapshot_get" });
    if (result.type !== "snapshot") {
      throw new Error(`snapshot_get returned ${result.type}`);
    }
    this.#resyncCount += 1;
    this.#install(result.snapshot, result.cursor);
    await this.#subscribe(result.cursor);
  }

  /**
   * Requests runtime shutdown.
   *
   * Shutdown is not detach or process exit: the runtime cancels and settles
   * all conversation-owned work before this method resolves, and the
   * transport stays open.
   */
  async shutdown(): Promise<void> {
    const result = await this.#connection.request({ method: "shutdown" });
    if (result.type !== "shutdown_completed") {
      throw new Error(`shutdown returned ${result.type}`);
    }
  }

  /**
   * Waits for the runtime's terminal settlement fact for one known attempt.
   *
   * The current projection is checked before and after listener installation,
   * so a settlement that raced the waiter is still returned. No process exit
   * or client-side inference can satisfy this wait.
   */
  async waitForAttemptSettlement(
    attemptId: string,
  ): Promise<RuntimeClientOutcome> {
    const settled = settlementOf(this.#state, attemptId);
    if (settled !== undefined) {
      return settled;
    }

    return new Promise<RuntimeClientOutcome>((resolve, reject) => {
      let done = false;
      let removeStateListener = () => {};
      let removeCloseListener = () => {};

      const finish = (callback: () => void): void => {
        if (done) {
          return;
        }
        done = true;
        removeStateListener();
        removeCloseListener();
        callback();
      };

      const check = (): void => {
        const outcome = settlementOf(this.#state, attemptId);
        if (outcome !== undefined) {
          finish(() => resolve(outcome));
        }
      };

      removeStateListener = this.onState(() => check());
      removeCloseListener = this.#connection.onClose((error) =>
        finish(() => reject(error)),
      );
      // The settlement may have been published between the initial check and
      // listener installation.
      check();
    });
  }

  /** Releases the attachment without cancelling anything. */
  async detach(): Promise<void> {
    const result = await this.#connection.request({ method: "detach" });
    if (result.type !== "detached") {
      throw new Error(`detach returned ${result.type}`);
    }
  }

  async #subscribe(afterCursor: RuntimeClientCursor): Promise<void> {
    // Buffer from before the request so an event published while the
    // subscription is in flight is folded in cursor order, not dropped.
    const buffered: RuntimeClientProtocolEvent[] = [];
    let subscribed = false;
    this.#detachedEvents?.();
    this.#detachedEvents = this.#connection.onEvent((event) => {
      if (subscribed) {
        this.#applyEvent(event);
      } else {
        buffered.push(event);
      }
    });

    try {
      const result = await this.#connection.request({
        method: "subscribe_events",
        after_cursor: afterCursor,
      });
      if (result.type !== "subscribed") {
        throw new Error(`subscribe_events returned ${result.type}`);
      }
    } catch (error) {
      if (
        error instanceof RuntimeRequestError &&
        error.error.type === "resync_required"
      ) {
        // The cursor fell out of the bounded replay window between the
        // snapshot and the subscription. Repair authoritatively.
        subscribed = true;
        await this.resync();
        return;
      }
      throw error;
    }

    subscribed = true;
    for (const event of buffered) {
      this.#applyEvent(event);
    }
  }

  /** Reads the authoritative active Session metadata. */
  async refreshSession(): Promise<SessionView> {
    const result = await this.#connection.request({ method: "session_get" });
    if (result.type !== "session") {
      throw new Error(`session_get returned ${result.type}`);
    }
    this.#sessionInfo = result.session;
    return result.session;
  }

  /** Lists bounded persisted Sessions for `/resume`. */
  async listSessions(
    query?: string,
    offset = 0,
    limit = SESSION_PROJECTION_PAGE_LIMIT,
  ): Promise<{ sessions: SessionSummaryView[]; nextOffset?: number }> {
    const result = await this.#connection.request({
      method: "session_list",
      query: query === undefined || query.length === 0 ? undefined : query,
      offset,
      limit,
    });
    if (result.type !== "session_list") {
      throw new Error(`session_list returned ${result.type}`);
    }
    return { sessions: result.sessions, nextOffset: result.next_offset };
  }

  /** Reads the active graph and native historical fork boundaries. */
  async sessionTree(): Promise<{
    session: SessionView;
    nodes: SessionNodeView[];
    nextNodeOffset?: number;
    branchableMessages: SessionUserMessageBoundaryView[];
    nextHistoryOffset?: number;
  }> {
    return this.sessionTreePage();
  }

  /** Reads one bounded active graph/history page from the native owner. */
  async sessionTreePage(
    nodeOffset = 0,
    historyOffset = 0,
    limit = SESSION_PROJECTION_PAGE_LIMIT,
  ): Promise<{
    session: SessionView;
    nodes: SessionNodeView[];
    nextNodeOffset?: number;
    branchableMessages: SessionUserMessageBoundaryView[];
    nextHistoryOffset?: number;
  }> {
    const result = await this.#connection.request({
      method: "session_tree_get",
      node_offset: nodeOffset,
      history_offset: historyOffset,
      limit,
    });
    if (result.type !== "session_tree") {
      throw new Error(`session_tree_get returned ${result.type}`);
    }
    this.#sessionInfo = result.session;
    return {
      session: result.session,
      nodes: result.nodes,
      nextNodeOffset: result.next_node_offset,
      branchableMessages: result.branchable_messages,
      nextHistoryOffset: result.next_history_offset,
    };
  }

  /** Renames metadata only. */
  async nameSession(name: string): Promise<SessionView> {
    const result = await this.#connection.request({ method: "session_name", name });
    if (result.type !== "session_changed") {
      throw new Error(`session_name returned ${result.type}`);
    }
    this.#sessionInfo = result.session;
    return result.session;
  }

  /** Creates a new empty Session and asks the caller to reattach. */
  async newSession(): Promise<SessionSwitch> {
    const result = await this.#connection.request({ method: "session_new" });
    return this.#sessionChanged(result, "session_new");
  }

  /** Selects a persisted Session/node and asks the caller to reattach. */
  async selectSession(
    sessionId: SessionId,
    nodeId?: SessionNodeId,
  ): Promise<SessionSwitch> {
    const result = await this.#connection.request({
      method: "session_select",
      session_id: sessionId,
      node_id: nodeId,
    });
    return this.#sessionChanged(result, "session_select");
  }

  /** Clones the exact committed current canonical head. */
  async cloneSession(): Promise<SessionSwitch> {
    const result = await this.#connection.request({ method: "session_clone" });
    return this.#sessionChanged(result, "session_clone");
  }

  /** Forks at an exact historical user-message boundary. */
  async forkSession(
    surfaceRevision: SurfaceRevision,
    messageId: string,
  ): Promise<SessionSwitch> {
    const result = await this.#connection.request({
      method: "session_fork",
      surface_revision: surfaceRevision,
      message_id: messageId,
    });
    return this.#sessionChanged(result, "session_fork");
  }

  /** Creates a branch node inside the active Session. */
  async branchTree(
    surfaceRevision: SurfaceRevision,
    messageId: string,
  ): Promise<SessionSwitch> {
    const result = await this.#connection.request({
      method: "session_tree_branch",
      surface_revision: surfaceRevision,
      message_id: messageId,
    });
    return this.#sessionChanged(result, "session_tree_branch");
  }

  #sessionChanged(
    result: RuntimeClientResult,
    method: string,
  ): SessionSwitch {
    if (result.type !== "session_changed") {
      throw new Error(`${method} returned ${result.type}`);
    }
    this.#sessionInfo = result.session;
    return {
      session: result.session,
      editorContent: result.editor_content,
      restartRequired: result.restart_required,
    };
  }

  #applyEvent(event: RuntimeClientProtocolEvent): void {
    if (this.#state === undefined) {
      return;
    }
    // An event at or before the installed cursor is already described by the
    // snapshot; folding it again would double-apply a fact.
    if (event.cursor <= this.#state.cursor) {
      return;
    }
    this.#state = reduce(this.#state, event);
    this.#publish();
  }

  #install(snapshot: RuntimeClientSnapshot, cursor: RuntimeClientCursor): void {
    this.#state = replaceFromSnapshot(
      snapshot,
      cursor,
      this.#state === undefined
        ? undefined
        : {
            notices: this.#state.notices,
            pendingSubmissions: this.#state.pendingSubmissions,
          },
    );
    this.#publish();
  }

  /** Applies a pure local transformation of transient client state. */
  updateState(update: (state: PresentationState) => PresentationState): void {
    if (this.#state === undefined) {
      return;
    }
    this.#state = update(this.#state);
    this.#publish();
  }

  #publish(): void {
    const state = this.#state;
    if (state === undefined) {
      return;
    }
    for (const listener of this.#listeners) {
      listener(state);
    }
  }
}

function settlementOf(
  state: PresentationState | undefined,
  attemptId: string,
): RuntimeClientOutcome | undefined {
  const attempt = state?.attempt;
  if (
    attempt === undefined ||
    attempt.attemptId !== attemptId ||
    attempt.phase.type !== "settled"
  ) {
    return undefined;
  }
  return attempt.phase.outcome;
}

/** Whether an error is the runtime asking for an authoritative repair. */
export function isResyncRequired(error: unknown): boolean {
  return (
    error instanceof RuntimeRequestError &&
    error.error.type === "resync_required"
  );
}

/** Whether an error ended the transport rather than answering a request. */
export function isConnectionClosed(error: unknown): boolean {
  return error instanceof ConnectionClosedError;
}
