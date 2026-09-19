/**
 * One attached Session: its authoritative projection and its typed operations.
 *
 * ```text
 * session/attach -> target + authoritative snapshot + cursor + subscription
 *                -> install projection
 *                -> fold session/event notifications
 * ```
 *
 * Attach is one cut: the server drains pending observations, captures the
 * snapshot and cursor, admits the controller and registers its subscription
 * under a single projection lock. A client that read the snapshot and then
 * issued its own `session/subscribe` would be re-registering over a
 * registration it already has — and would be asking for exactly the gap that
 * single cut exists to prevent. `session/subscribe` therefore appears in one
 * place only: repairing after a resync, against the new snapshot's cursor.
 *
 * This owner sequences snapshot installation, subscription and resync repair
 * for exactly one Session. It owns no agent semantics: it starts nothing,
 * settles nothing, and interprets no model, tool or capability value. The
 * server is authoritative; everything held here is a projection that one fresh
 * snapshot rebuilds completely. Ordinary settlement reads refresh those facts
 * over the existing subscription, preserving safely joined history and local UI.
 *
 * # Identity and fencing
 *
 * An {@link AttachmentTarget} names four distinct domains at once: the Session,
 * the Conversation, the runtime incarnation, and this attachment. They are not
 * aliases of each other, and neither is the projection cursor or a JSON-RPC
 * request id. Every notification repeats the full target, and this object folds
 * an event only when all four match — so an event addressed to a previous
 * incarnation of the same Session cannot reach a replacement projection.
 *
 * Responses are fenced the same way, by epoch: an authoritative read issued
 * against one attachment can still be in flight when the attachment is replaced
 * or released, and installing its result afterwards would overwrite current
 * truth with stale truth. The epoch check makes that impossible without any
 * reliance on timing.
 *
 * # Resync
 *
 * `session/resyncRequired` means the incremental projection can no longer be
 * trusted. The response is never to guess at the gap:
 *
 * ```text
 * incremental state no longer trusted
 *   -> session/snapshot
 *   -> replace the projection wholesale
 *   -> session/subscribe after the new cursor
 * ```
 *
 * Nothing is replayed from what the UI thought happened.
 */

import {
  compareExact,
  sameTarget,
  type AttachmentTarget,
  type CapabilityView,
  type GoalControl,
  type GoalView,
  type InteractionRef,
  type InteractionResponse,
  type ModelCatalogView,
  type Notification,
  type RuntimeClientBackgroundExecution,
  type RuntimeClientContextView,
  type RuntimeClientCursor,
  type RuntimeClientSnapshot,
  type RuntimeClientSubagent,
  type RuntimeClientSubagentWorkspaceDisposalOutcome,
  type RuntimeClientTranscriptCursor,
  type RuntimeClientTranscriptPage,
  type SessionModelConfig,
  type SessionModelView,
  type SessionNodeId,
  type SessionUserMessageBoundary,
  type SubagentId,
  type SurfaceRevision,
  type ToolExecutionId,
  type UserInputBlock,
} from "../protocol/app-server.ts";
import {
  mergeTranscriptPage,
  refreshFromSnapshot,
  reduce,
  replaceFromSnapshot,
} from "../presentation/projection.ts";
import type { PresentationState } from "../presentation/state.ts";
import { AppServerClient, isResyncRequired } from "./client.ts";

/** Native owner bound for one bounded page. */
export const SESSION_PROJECTION_PAGE_LIMIT = 32;
export const TRANSCRIPT_PROJECTION_PAGE_LIMIT = 32;

type StateListener = (state: PresentationState) => void;
type SnapshotListener = () => void;
type ClosedListener = () => void;

/** One bounded page of historical fork/branch boundaries. */
export interface BoundaryPage {
  /** The exact committed head the page was selected against. */
  surfaceRevision: SurfaceRevision;
  boundaries: SessionUserMessageBoundary[];
  nextOffset?: number;
}

export class AppServerSession {
  readonly nodeId: SessionNodeId | undefined;
  readonly #client: AppServerClient;
  #target: AttachmentTarget;
  #state: PresentationState;
  readonly #listeners = new Set<StateListener>();
  readonly #snapshotListeners = new Set<SnapshotListener>();
  readonly #closedListeners = new Set<ClosedListener>();
  #resyncCount = 0;
  /** Advanced whenever authoritative ownership of this projection changes. */
  #epoch = 0;
  #released = false;
  #serverClosed = false;
  /** Serializes repairs so two resyncs cannot interleave their installs. */
  #repair: Promise<void> = Promise.resolve();
  #refresh: Promise<void> | undefined;
  #refreshRequested = false;

  private constructor(
    client: AppServerClient,
    target: AttachmentTarget,
    state: PresentationState,
    nodeId?: SessionNodeId,
  ) {
    this.nodeId = nodeId;
    this.#client = client;
    this.#target = target;
    this.#state = state;
    client.onClose(() => {
      this.#released = true;
      this.#epoch += 1;
    });
  }

  /**
   * Attaches to a durable Session and installs its authoritative projection.
   *
   * Attach loads or reuses the runtime and acquires this connection's external
   * control of it. It is not a process operation: attaching to a second Session
   * never replaces the first, and both stay live in the same App Server.
   */
  static async attach(
    client: AppServerClient,
    sessionId: string,
    nodeId?: SessionNodeId,
  ): Promise<AppServerSession> {
    const attached = await client.call(
      "session/attach",
      { session_id: sessionId, node_id: nodeId ?? null },
      "attached",
    );
    return new AppServerSession(
      client,
      attached.target,
      replaceFromSnapshot(attached.snapshot, attached.cursor),
      nodeId,
    );
  }

  /** The full four-domain identity of this attachment. */
  get target(): AttachmentTarget {
    return this.#target;
  }

  get sessionId(): string {
    return this.#target.session_id;
  }

  /** The current presentation state. Always a projection, never authority. */
  get state(): PresentationState {
    return this.#state;
  }

  /** How many authoritative repairs this attachment has performed. */
  get resyncCount(): number {
    return this.#resyncCount;
  }

  /** Whether the server reported this attachment closed. */
  get serverClosed(): boolean {
    return this.#serverClosed;
  }

  /** Whether this client has released the attachment. */
  get released(): boolean {
    return this.#released;
  }

  /** Subscribes to presentation state changes. */
  onState(listener: StateListener): () => void {
    this.#listeners.add(listener);
    return () => this.#listeners.delete(listener);
  }

  /**
   * Subscribes to authoritative projection replacement, including resync.
   *
   * The controlling TUI uses this signal to invalidate attachment-local
   * presentation leases. It carries no runtime semantics.
   */
  onSnapshot(listener: SnapshotListener): () => void {
    this.#snapshotListeners.add(listener);
    return () => this.#snapshotListeners.delete(listener);
  }

  /** Subscribes to the server reporting this attachment closed. */
  onClosed(listener: ClosedListener): () => void {
    this.#closedListeners.add(listener);
    return () => this.#closedListeners.delete(listener);
  }

  // -------------------------------------------------------------------------
  // Notification routing
  // -------------------------------------------------------------------------

  /**
   * Folds one routed notification, if it addresses exactly this attachment.
   *
   * Returns whether the notification belonged here. A notification for another
   * Session, or for a superseded incarnation or attachment of this Session, is
   * declined rather than applied: a stale target must never mutate the
   * projection that replaced it.
   */
  applyNotification(notification: Notification): boolean {
    if (this.#released || this.#serverClosed || !sameTarget(notification.params.target, this.#target)) {
      return false;
    }
    switch (notification.method) {
      case "session/event":
        this.#applyEvent(notification.params.cursor, notification.params.event);
        return true;
      case "session/resyncRequired":
        // The bounded replay window has moved past this client's cursor.
        // Repair authoritatively; never interpolate the gap.
        this.#enqueueRepair();
        return true;
      case "session/closed":
        this.#serverClosed = true;
        this.#epoch += 1;
        // Residency ended. That is a statement about this attachment's
        // observability, not a runtime outcome: nothing here fabricates a
        // settled attempt, an answered interaction, or a completed tool.
        for (const listener of [...this.#closedListeners]) {
          listener();
        }
        return true;
      default: {
        const exhaustive: never = notification;
        void exhaustive;
        return false;
      }
    }
  }

  // -------------------------------------------------------------------------
  // Turn and interaction
  // -------------------------------------------------------------------------

  /** Submits one inbound message. Acceptance, never completion. */
  async submitInbound(
    content: UserInputBlock[],
  ): Promise<{ messageId: string; sequence: string }> {
    const accepted = await this.#client.call(
      "turn/start",
      { target: this.#target, content },
      "inbound_accepted",
    );
    return {
      messageId: accepted.message_id,
      sequence: accepted.inbound_sequence,
    };
  }

  /** Steering and queue admission stay native; neither retains a client queue. */
  async steer(content: UserInputBlock[]): Promise<void> {
    await this.#client.call("turn/steer", { target: this.#target, content }, "inbound_accepted");
  }

  async editPending(expected: import("../protocol/app-server.ts").MethodParams<"inbound/edit">["expected"], text: string): Promise<void> {
    const result = await this.#client.call("inbound/edit", { target: this.#target, expected, text }, "inbound_mutation");
    if (result.outcome.status !== "applied") throw new Error(`Pending edit: ${result.outcome.status}. Not retried.`);
  }

  async removePending(expected: import("../protocol/app-server.ts").MethodParams<"inbound/remove">["expected"]): Promise<void> {
    const result = await this.#client.call("inbound/remove", { target: this.#target, expected }, "inbound_mutation");
    if (result.outcome.status !== "applied") throw new Error(`Pending removal: ${result.outcome.status}. Not retried.`);
  }

  async upload(name: string, bytes: Uint8Array): Promise<UserInputBlock[]> {
    const result = await this.#client.call("session/upload", {
      target: this.#target, files: [{ name, data: Buffer.from(bytes).toString("base64") }],
    }, "session_uploaded");
    return result.files.map(({ receipt }) => ({ type: "upload", ...receipt }));
  }

  async permissionSources() {
    return (await this.#client.call("configuration/sourcesRead", { session_id: this.sessionId }, "source_settings")).projection;
  }

  async writePermission(revision: string, mode: import("../protocol/app-server.ts").ApprovalMode) {
    await this.#client.call("configuration/sourceWrite", {
      session_id: this.sessionId, expected_revision: revision,
      mutation: { kind: "config", scope: "workspace", mutation: { unit: "approval", authored: mode } },
    }, "source_settings");
    return this.permissionSources();
  }

  async publishPermissions() {
    await this.#client.call("configuration/reload", { target: this.#target }, "configuration_reloaded");
    await this.resync();
    return this.permissionSources();
  }

  /** Requests cancellation of the current attempt. Acceptance, not settlement. */
  async cancelCurrentAttempt(): Promise<string> {
    const accepted = await this.#client.call(
      "turn/cancel",
      { target: this.#target },
      "cancellation_accepted",
    );
    return accepted.attempt_id;
  }

  /** Answers one runtime-owned interaction. */
  async respondInteraction(
    interaction: InteractionRef,
    response: InteractionResponse,
  ): Promise<void> {
    await this.#client.call(
      "interaction/respond",
      { target: this.#target, interaction, response },
      "interaction_settled",
    );
  }

  /** Cancels one pending interaction through its originating coordinator. */
  async cancelInteraction(interaction: InteractionRef): Promise<void> {
    await this.#client.call(
      "interaction/cancel",
      { target: this.#target, interaction },
      "interaction_settled",
    );
  }

  // -------------------------------------------------------------------------
  // Authoritative reads
  // -------------------------------------------------------------------------

  /**
   * Reads one bounded durable transcript page by its exclusive older boundary.
   * Callers pass the previous page's `next_cursor` unchanged.
   */
  async transcriptPage(
    beforeCursor?: RuntimeClientTranscriptCursor,
    limit = TRANSCRIPT_PROJECTION_PAGE_LIMIT,
  ): Promise<RuntimeClientTranscriptPage> {
    const page = await this.#client.call(
      "session/transcript",
      { target: this.#target, before: beforeCursor ?? null, limit },
      "transcript",
    );
    return page.page;
  }

  /** Loads the next older page without changing the live event cursor. */
  async loadOlderTranscript(
    limit = TRANSCRIPT_PROJECTION_PAGE_LIMIT,
  ): Promise<boolean> {
    const beforeCursor = this.#state.transcriptNextCursor;
    if (beforeCursor === undefined || beforeCursor === null) {
      return false;
    }
    const epoch = this.#epoch;
    const page = await this.transcriptPage(beforeCursor, limit);
    if (epoch !== this.#epoch) {
      // The projection was authoritatively replaced while this page was in
      // flight. Merging it now would splice history into a state it does not
      // describe.
      return false;
    }
    this.#state = mergeTranscriptPage(this.#state, page);
    this.#publish();
    return (page.entries ?? []).length > 0;
  }

  /** Reads one bounded page of historical fork/branch boundaries. */
  async boundaries(
    offset = 0,
    limit = SESSION_PROJECTION_PAGE_LIMIT,
  ): Promise<BoundaryPage> {
    const page = await this.#client.call(
      "session/boundaries",
      { target: this.#target, offset, limit },
      "boundaries",
    );
    return {
      surfaceRevision: page.surface_revision,
      boundaries: page.boundaries,
      nextOffset: page.next_offset ?? undefined,
    };
  }

  /** Reads the active capability projection. */
  async capabilities(): Promise<CapabilityView> {
    const read = await this.#client.call(
      "resources/read",
      { target: this.#target },
      "capabilities",
    );
    return read.capabilities;
  }

  // -------------------------------------------------------------------------
  // Maintenance and settings
  // -------------------------------------------------------------------------

  /** Runs one manual idle compaction to its durable terminal result. */
  async compactContext(): Promise<RuntimeClientContextView> {
    const compacted = await this.#client.call(
      "context/compact",
      { target: this.#target },
      "context",
    );
    return compacted.context;
  }

  /** Reads or mutates the root Goal through its native owner. */
  async goal(control: GoalControl): Promise<GoalView> {
    const result = await this.#client.call(
      "goal/control",
      { target: this.#target, control },
      "goal",
    );
    return result.view;
  }

  /** Atomically reloads resources for future admitted attempts. */
  async reloadConfiguration(): Promise<{
    resourceRevision: string;
    capabilityRevision: string;
  }> {
    const reloaded = await this.#client.call(
      "configuration/reload",
      { target: this.#target },
      "configuration_reloaded",
    );
    return {
      resourceRevision: reloaded.resource_revision,
      capabilityRevision: reloaded.capability_revision,
    };
  }

  /** The safe public catalog. This is why the client never reads rustx.toml. */
  async modelCatalog(): Promise<ModelCatalogView> {
    const models = await this.#client.call(
      "settings/models",
      { target: this.#target },
      "models",
    );
    return models.catalog;
  }

  async modelGet(): Promise<SessionModelView> {
    const model = await this.#client.call(
      "settings/model",
      { target: this.#target },
      "model",
    );
    return model.model;
  }

  /**
   * Replaces the authoritative session model configuration.
   *
   * A whole-state replacement, never a patch: callers send back the complete
   * configuration they read. The update affects future admissions only; an
   * already-admitted attempt keeps the model it froze.
   */
  async modelSet(config: SessionModelConfig): Promise<SessionModelView> {
    const model = await this.#client.call(
      "settings/setModel",
      { target: this.#target, config },
      "model",
    );
    return model.model;
  }

  /** One native read of the published immutable configuration. */
  async configuration(): Promise<import("../protocol/app-server.ts").EffectiveConfiguration> {
    const result = await this.#client.call("configuration/effective", { target: this.#target }, "effective_configuration");
    return result.projection;
  }

  // -------------------------------------------------------------------------
  // Background and subagents
  // -------------------------------------------------------------------------

  /**
   * Requests cancellation of one background execution.
   *
   * The returned registry snapshot is *acceptance*. The terminal fact arrives
   * later on the event stream, and only the runtime decides it.
   */
  async cancelBackground(
    executionId: ToolExecutionId,
  ): Promise<RuntimeClientBackgroundExecution> {
    const accepted = await this.#client.call(
      "background/cancel",
      { target: this.#target, execution_id: executionId },
      "background",
    );
    return accepted.execution;
  }

  async backgroundStatus(
    executionId: ToolExecutionId,
  ): Promise<RuntimeClientBackgroundExecution> {
    const status = await this.#client.call(
      "background/status",
      { target: this.#target, execution_id: executionId },
      "background",
    );
    return status.execution;
  }

  async subagentStatus(subagentId: SubagentId): Promise<RuntimeClientSubagent> {
    const status = await this.#client.call(
      "subagent/status",
      { target: this.#target, subagent_id: subagentId },
      "subagent",
    );
    return status.subagent;
  }

  async cancelSubagent(subagentId: SubagentId): Promise<RuntimeClientSubagent> {
    const accepted = await this.#client.call(
      "subagent/cancel",
      { target: this.#target, subagent_id: subagentId },
      "subagent",
    );
    return accepted.subagent;
  }

  /** Disposes one retained subagent workspace through the runtime authority. */
  async disposeSubagent(subagentId: SubagentId): Promise<{
    subagent: RuntimeClientSubagent;
    outcome: RuntimeClientSubagentWorkspaceDisposalOutcome;
  }> {
    const disposed = await this.#client.call(
      "subagent/disposeWorkspace",
      { target: this.#target, subagent_id: subagentId },
      "workspace_disposed",
    );
    return { subagent: disposed.subagent, outcome: disposed.outcome };
  }

  // -------------------------------------------------------------------------
  // Repair and release
  // -------------------------------------------------------------------------

  /**
   * Takes a fresh authoritative snapshot and replaces the projection.
   *
   * The snapshot and the re-subscription are two requests, so the cursor is
   * what joins them: the new registration starts exactly where the snapshot
   * ends, and any observation in between is either already described by the
   * snapshot or arrives after that cursor.
   */
  async resync(): Promise<void> {
    if (this.#released || this.#serverClosed) return;
    const epoch = ++this.#epoch;
    const snapshot = await this.#client.call(
      "session/snapshot",
      { target: this.#target },
      "snapshot",
    );
    if (epoch !== this.#epoch) {
      return;
    }
    this.#resyncCount += 1;
    this.#install(snapshot.snapshot, snapshot.cursor);
    await this.#subscribe(snapshot.cursor);
  }

  /**
   * Releases this attachment.
   *
   * Detach removes exactly this connection's control relationship. It does not
   * cancel a turn, settle an interaction, unload the runtime, or shut anything
   * down, and the Session keeps running in the App Server afterwards.
   */
  async detach(): Promise<void> {
    if (this.#released) {
      return;
    }
    this.#released = true;
    this.#epoch += 1;
    await this.#client.call(
      "session/detach",
      { target: this.#target },
      "detached",
    );
  }

  /** Switch the durable branch; native retirement remains manager-owned. */
  async switchNode(nodeId: string): Promise<void> {
    this.#released = true;
    this.#epoch += 1;
    await this.#client.call("session/switchNode", { target: this.#target, node_id: nodeId }, "session");
  }

  /** Applies a pure local transformation of transient client state. */
  updateState(update: (state: PresentationState) => PresentationState): void {
    this.#state = update(this.#state);
    this.#publish();
  }

  // -------------------------------------------------------------------------
  // Internals
  // -------------------------------------------------------------------------

  #enqueueRepair(): void {
    // Fence reads immediately, even before the serialized repair starts.
    this.#epoch += 1;
    this.#repair = this.#repair.then(
      () => (this.#released ? undefined : this.resync()),
      () => undefined,
    );
    void this.#repair.catch(() => {});
  }

  /** Read facts over the existing subscription; never replace presentation ownership. */
  #enqueueRefresh(): void {
    this.#refreshRequested = true;
    if (this.#refresh) return;
    this.#refresh = this.#refreshLive().finally(() => {
      this.#refresh = undefined;
      if (this.#refreshRequested && !this.#released && !this.#serverClosed) this.#enqueueRefresh();
    });
    void this.#refresh.catch(() => {});
  }

  async #refreshLive(): Promise<void> {
    while (this.#refreshRequested && !this.#released && !this.#serverClosed) {
      this.#refreshRequested = false;
      const epoch = this.#epoch;
      const target = this.#target;
      const fresh = await this.#client.call("session/snapshot", { target }, "snapshot");
      if (epoch !== this.#epoch || !sameTarget(target, this.#target)) return;
      // Events may advance while the read is in flight. Never roll them back.
      // A subsequent read crosses that exact cursor without replaying any action.
      if (compareExact(fresh.cursor, this.#state.cursor) < 0) {
        this.#refreshRequested = true;
        continue;
      }
      this.#state = refreshFromSnapshot(this.#state, fresh.snapshot, fresh.cursor);
      this.#publish();
    }
  }

  async #subscribe(afterCursor: RuntimeClientCursor): Promise<void> {
    try {
      await this.#client.call(
        "session/subscribe",
        { target: this.#target, after_cursor: afterCursor },
        "subscribed",
      );
    } catch (error) {
      if (isResyncRequired(error)) {
        // The cursor fell out of the bounded replay window between the
        // snapshot and the subscription. Repair authoritatively.
        await this.resync();
        return;
      }
      throw error;
    }
  }

  #applyEvent(
    cursor: RuntimeClientCursor,
    event: Parameters<typeof reduce>[1]["event"],
  ): void {
    // An event at or before the installed cursor is already described by the
    // snapshot; folding it again would double-apply a fact. Cursors are exact
    // u64 decimal text, so this is a numeric comparison and never a string one.
    if (compareExact(cursor, this.#state.cursor) <= 0) {
      return;
    }
    this.#state = reduce(this.#state, { cursor, event });
    this.#publish();
    // Completion/statistics are native read facts, not fields to derive from events.
    if (event.type === "attempt_settled") this.#enqueueRefresh();
  }

  #install(snapshot: RuntimeClientSnapshot, cursor: RuntimeClientCursor): void {
    this.#epoch += 1;
    this.#state = replaceFromSnapshot(snapshot, cursor);
    for (const listener of [...this.#snapshotListeners]) {
      listener();
    }
    this.#publish();
  }

  #publish(): void {
    for (const listener of [...this.#listeners]) {
      listener(this.#state);
    }
  }
}
