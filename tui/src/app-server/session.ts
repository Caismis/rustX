import { uploadOperationId } from '../../../protocol/app-server/upload.ts';
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
 * snapshot rebuilds completely. Ordinary events, including native read-domain
 * cuts at settlement, preserve safely joined history and local UI without rereads.
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
  type ConfigurationApplication,
  type AvailableConfiguration,
  type AttachmentTarget,
  type CapabilityView,
  type GoalControl,
  type GoalView,
  type InteractionRef,
  type InteractionResponse,
  type ModelCatalogView,
  type Notification,
  type RuntimeClientJob,
  type RuntimeClientContextView,
  type RuntimeClientCursor,
  type RuntimeClientSnapshot,
  type RuntimeClientAgent,
  type RuntimeClientTranscriptCursor,
  type RuntimeClientTranscriptPage,
  type SessionFileReference,
  type SessionModelConfig,
  type SessionModelView,
  type SessionNodeId,
  type SessionUserMessageBoundary,
  type SubagentId,
  type AgentId,
  type SurfaceRevision,
  type ToolExecutionId,
  type UserInputBlock,
} from "../protocol/app-server.ts";
import {
  mergeTranscriptPage,
  reduce,
  replaceFromSnapshot,
} from "../presentation/projection.ts";
import type { PresentationState } from "../presentation/state.ts";
import {
  pageDeliveries,
  type DeliveryLocation,
  type DeliveryPage,
  type DeliveryRecord,
} from "../presentation/deliveries.ts";
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
  #configuration?: ConfigurationApplication;
  get application(): ConfigurationApplication | undefined { return this.#configuration; }
  readonly #listeners = new Set<StateListener>();
  readonly #snapshotListeners = new Set<SnapshotListener>();
  readonly #closedListeners = new Set<ClosedListener>();
  #resyncCount = 0;
  /** Advanced whenever authoritative ownership of this projection changes. */
  #epoch = 0;
  #released = false;
  #serverClosed = false;
  /** Serializes repairs so two resyncs cannot interleave their installs. */
  #repair: Promise<void> | undefined;
  #acquiring = false;

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
    const session = new AppServerSession(client, attached.target,
      replaceFromSnapshot(attached.snapshot, attached.cursor), nodeId);
    if (attached.configuration) session.#installConfiguration(attached.configuration);
    return session;
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
    if (notification.method === "configuration/changed") {
      const application = notification.params.application;
      if (this.#released || application.scope !== this.sessionId) return false;
      this.#installConfiguration(application);
      return true;
    }
    if (notification.method === "session/summaryInvalidated" || notification.method === "session/ownershipRetired") {
      // Both explicit catalog_changed values are valid. The TUI holds
      // no live catalog/summary cache: `/resume` reads the catalog afresh every
      // time it opens, so there is nothing here to repair. The notification is
      // accepted and declined — never folded into the conversation projection,
      // and never treated as invalid protocol input.
      return false;
    }
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

  get uploadPolicy() { return this.#client.capabilities?.upload_policy; }
  async upload(name: string, bytes: Uint8Array): Promise<UserInputBlock[]> {
    const operation = uploadOperationId();
    const epoch = this.#epoch, target = this.#target;
    const current = () => !this.#released && !this.#serverClosed && epoch === this.#epoch && sameTarget(target, this.#target);
    const check = () => { if (!current()) throw new Error(`Upload authority changed; reconcile operation ${operation}`); };
    check();
    const policy = this.uploadPolicy;
    if (!policy) throw new Error('Upload policy unavailable');
    const { transfer } = await this.#client.call("session/uploadPrepare", {
      target, operation_id: operation, files: [{ name, size: bytes.byteLength }],
    }, "upload_prepared");
    check();
    try { await this.#client.uploadCarrier(transfer, [new Blob([Uint8Array.from(bytes)])], policy, this.#client.uploadEndpoint); } catch { /* Repair the exact operation; never resend bytes. */ }
    check();
    const { outcome } = await this.#client.call("session/uploadStatus", { target, operation_id: operation }, "upload_status");
    check();
    if (outcome.state !== 'ready') throw new Error(`Upload ${operation}: ${outcome.state}. No bytes replayed.`);
    return outcome.files.map(({ receipt }) => ({ type: "upload", ...receipt }));
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
      { target: this.#target, at: beforeCursor ? { type: 'older', before: beforeCursor } : { type: 'latest' }, limit },
      "transcript_window",
    );
    return page.window.page;
  }

  /**
   * Whether this connection's transport was granted delivery access: the
   * owned stdio child, or the separate remote delivery credential.
   */
  get deliveryAccess(): boolean {
    return this.#client.capabilities?.delivery_access === true;
  }

  /** One bounded page of committed deliveries, by the same transcript paging. */
  async deliveryPage(before?: RuntimeClientTranscriptCursor): Promise<DeliveryPage> {
    return pageDeliveries(await this.transcriptPage(before));
  }

  /**
   * Original bytes (≤ 512 KiB, base64) of one committed delivery, through
   * this attachment. Native authority resolves the address every time; no
   * model request, Agent, or Tool execution starts. Aborting `signal`
   * cancels this exact native request on the server; the call then settles
   * with the server's terminal outcome (bytes only if publication won).
   */
  async readDelivery(
    record: Pick<DeliveryRecord, "messageId" | "index">,
    signal: AbortSignal = new AbortController().signal,
  ): Promise<{ file: SessionFileReference; data: string }> {
    const { file, data } = await this.#client.callDelivery(
      "delivery/read",
      { target: this.#target, message_id: record.messageId, delivery_index: record.index },
      "session_file_bytes",
      signal,
    );
    return { file, data };
  }

  /**
   * The verified server-side path and leaf identity of one delivery.
   * Aborting `signal` cancels the native request (see `readDelivery`).
   */
  async locateDelivery(
    record: Pick<DeliveryRecord, "messageId" | "index">,
    signal: AbortSignal = new AbortController().signal,
  ): Promise<DeliveryLocation> {
    const { file, path, device, inode } = await this.#client.callDelivery(
      "delivery/locate",
      { target: this.#target, message_id: record.messageId, delivery_index: record.index },
      "session_file_location",
      signal,
    );
    return { file, path, device, inode };
  }

  /** A read is valid only in the exact parent attachment/presentation epoch. */
  async agentTranscriptPage(
    agentId: AgentId,
    before?: RuntimeClientTranscriptCursor,
  ): Promise<RuntimeClientTranscriptPage | undefined> {
    if (this.#released || this.#serverClosed) return undefined;
    const epoch = this.#epoch;
    const target = this.#target;
    try {
      const result = await this.#client.call("agent/transcript", {
        target, agent_id: agentId, at: before == null ? { type: "latest" } : { type: "older", before, cut: null },
        limit: TRANSCRIPT_PROJECTION_PAGE_LIMIT,
      }, "transcript_window");
      if (epoch !== this.#epoch || !sameTarget(target, this.#target)) return undefined;
      return result.window.page;
    } catch (error) {
      if (epoch !== this.#epoch || !sameTarget(target, this.#target)) return undefined;
      throw error;
    }
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
    if (epoch !== this.#epoch || this.#state.transcriptNextCursor !== beforeCursor) {
      // A page belongs to the exact boundary requested. A live refresh can
      // replace that window without replacing attachment ownership. Neither
      // that stale page nor its next cursor may be spliced into the new window.
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
      { target: this.#target, request_id: globalThis.crypto.randomUUID() },
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

  #installConfiguration(application: ConfigurationApplication): void {
    if (this.#configuration && compareExact(application.version, this.#configuration.version) < 0) return;
    this.#configuration = application;
    this.#publish();
  }

  async readConfiguration() {
    const result = await this.#client.call("session/configuration", { session_id: this.sessionId }, "session_configuration");
    if (result.application) this.#installConfiguration(result.application);
    return result.application;
  }

  async adoptConfiguration(candidate: AvailableConfiguration): Promise<ConfigurationApplication> {
    const { application } = await this.#client.call("session/adoptConfiguration", {
      session_id: this.sessionId, candidate: candidate.identity, expected_binding: candidate.expected_binding,
    }, "configuration_application");
    // Acknowledgement is not a new observation. The caller rereads native authority.
    return application;
  }

  /** The safe public catalog. This is why the client never reads rustx.toml. */
  async modelCatalog(): Promise<ModelCatalogView> {
    const models = await this.#client.call(
      "session/models",
      { target: this.#target },
      "models",
    );
    return models.catalog;
  }

  async modelGet(): Promise<SessionModelView> {
    const model = await this.#client.call(
      "session/model",
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
      "session/setModel",
      { target: this.#target, config },
      "model",
    );
    return model.model;
  }

  /** One native read of the published immutable configuration. */
  async configuration(): Promise<import("../protocol/app-server.ts").EffectiveConfiguration> {
    const result = await this.#client.call("session/effectiveConfiguration", { target: this.#target }, "effective_configuration");
    return result.projection;
  }

  // -------------------------------------------------------------------------
  // Finite Jobs and durable Agents
  // -------------------------------------------------------------------------

  /**
   * Cancels one finite Job through the runtime physical-settlement contract.
   */
  async cancelJob(jobId: ToolExecutionId): Promise<RuntimeClientJob> {
    const result = await this.#client.call("job/cancel", { target: this.#target, job_id: jobId }, "job");
    return result.job;
  }

  async jobStatus(jobId: ToolExecutionId): Promise<RuntimeClientJob> {
    const result = await this.#client.call("job/status", { target: this.#target, job_id: jobId }, "job");
    return result.job;
  }

  async listJobs() {
    return this.#client.call("job/list", { target: this.#target }, "jobs");
  }

  async waitJob(jobId: ToolExecutionId): Promise<RuntimeClientJob> {
    const result = await this.#client.call("job/wait", { target: this.#target, job_id: jobId }, "job");
    return result.job;
  }

  async agentStatus(agentId: AgentId): Promise<RuntimeClientAgent> {
    const result = await this.#client.call("agent/status", { target: this.#target, agent_id: agentId }, "agent");
    return result.agent;
  }

  async sendMessage(agentId: AgentId, message: string, attachments: import("../../../protocol/app-server/v42.js").UploadReceipt[] = []) {
    return this.#client.call("agent/sendMessage", { target: this.#target, agent_id: agentId, message, attachments }, "agent_message");
  }

  async waitAgent(agentId: AgentId) {
    return this.#client.call("agent/wait", { target: this.#target, agent_id: agentId }, "agent_wait");
  }

  async interruptAgent(agentId: AgentId) {
    return this.#client.call("agent/interrupt", { target: this.#target, agent_id: agentId }, "agent_wait");
  }

  /** Disposes one retained subagent workspace through the runtime authority. */
  async disposeSubagent(subagentId: SubagentId) {
    const disposed = await this.#client.call(
      "subagent/disposeWorkspace",
      { target: this.#target, subagent_id: subagentId },
      "workspace_disposed",
    );
    return { subagent_id: disposed.subagent_id, workspace: disposed.workspace, outcome: disposed.outcome };
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
  resync(): Promise<void> {
    if (this.#repair) return this.#repair;
    if (this.#released || this.#serverClosed) return Promise.resolve();
    this.#acquiring = true;
    let epoch = ++this.#epoch;
    const target = this.#target;
    const work = (async () => {
      for (let attempt = 0; attempt < 3; attempt++) {
        this.#acquiring = true;
        const snapshot = await this.#client.call("session/snapshot", { target }, "snapshot");
        if (epoch !== this.#epoch || this.#released || this.#serverClosed) return;
        this.#resyncCount += 1;
        this.#install(snapshot.snapshot, snapshot.cursor);
        epoch = this.#epoch;
        this.#acquiring = false;
        try {
          await this.#client.call("session/subscribe", { target, after_cursor: snapshot.cursor }, "subscribed");
          return;
        } catch (error) {
          if (!isResyncRequired(error) || attempt === 2) throw error;
        }
      }
    })().catch(error => { this.#acquiring = true; throw error; }).finally(() => {
      if (this.#repair === work) this.#repair = undefined;
    });
    this.#repair = work;
    return work;
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
    void this.resync().catch(() => {});
  }

  #applyEvent(
    cursor: RuntimeClientCursor,
    event: Parameters<typeof reduce>[1]["event"],
  ): void {
    // An event at or before the installed cursor is already described by the
    // snapshot; folding it again would double-apply a fact. Cursors are exact
    // u64 decimal text, so this is a numeric comparison and never a string one.
    if (this.#acquiring || compareExact(cursor, this.#state.cursor) <= 0) {
      return;
    }
    if (BigInt(cursor) !== BigInt(this.#state.cursor) + 1n) { this.#enqueueRepair(); return; }
    this.#state = reduce(this.#state, { cursor, event });
    this.#publish();

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
