import { readSourceRepairFields, type SourceRepairRead } from '../../../protocol/app-server/source-observation.ts';
import { materializeSettings } from '../../../protocol/app-server/settings-observation.ts';
import { readEditor } from '../../../protocol/app-server/editor.ts';
import { archiveDownloadUrl } from "../../../protocol/app-server/download.ts";
import { saveSessionArchive } from "./archive.ts";
/**
 * The App Server a TUI is talking to, and who owns its process.
 *
 * ```text
 * local self-hosted                  existing / remote
 *   rustx-tui spawns                   someone else runs
 *   rustx app-server --listen stdio    rustx app-server --listen ws://...
 *          |                                   |
 *   StdioTransport                      WebSocketTransport
 *          \                                   /
 *           \_________ AppServerClient _______/
 *                     one protocol, one semantics
 * ```
 *
 * The two constructors differ in exactly one way that matters — **who spawned
 * the process** — and that is the only thing ownership is ever derived from.
 * Not the Session id, not the endpoint address, not whether the host is
 * loopback, not the transport type. A TUI that spawned its App Server owns it
 * and ends it on exit; a TUI that connected to one someone else is running must
 * never stop it, and never stops anyone else's Sessions either.
 *
 * # Session multiplicity
 *
 * One host keeps many Sessions attached at once. Switching which Session the
 * terminal shows is a change of *focus* in the client: it attaches or reuses an
 * attachment, and it does not detach, quiesce, cancel, unload, restart or
 * replace anything else. A Session that is not on screen keeps executing in the
 * same App Server process.
 */

import {
  type SessionNodeId,
  type SessionPersistentState,
  type SessionSnapshot,
  type SessionSummaryView,
  type SessionNode,
  type SessionDeleteResult,
  type SessionId,
  type SurfaceRevision,
  type UserInputBlock,
  type Notification,
  type SourceTarget,
} from "../protocol/app-server.ts";
import type { SourceRepairObservation } from "../presentation/source-repairs.ts";
import { laterRepairSequence, retainSourceRepair, sourceRepairDecision, sourceRepairKey } from "../../../protocol/app-server/source-repair.ts";
import { AppServerClient } from "./client.ts";
import {
  AppServerChild,
  type AppServerChildOptions,
  type ChildExit,
} from "./child-process.ts";
import { AppServerSession } from "./session.ts";
import { SESSION_PROJECTION_PAGE_LIMIT } from "./session-readers.ts";
import { StdioTransport } from "./stdio-transport.ts";
import {
  WebSocketTransport,
  type WebSocketTransportOptions,
} from "./websocket-transport.ts";
import { TransportClosedError } from "./transport.ts";

/**
 * Who owns the App Server process.
 *
 * Established by construction, never inferred afterwards.
 */
export type ProcessOwnership =
  /** This TUI spawned the App Server and must end it on exit. */
  | "owned_child"
  /** Someone else runs the App Server; this TUI only ever disconnects. */
  | "external";

/** One durable Session transition the server committed. */
export interface SessionTransition {
  session: SessionSnapshot;
  /** Fork/tree content selected before publication; never canonical history. */
  editorContent?: UserInputBlock[];
  /** Present only when the transition committed before durability was certain. */
  durabilityDiagnostic?: string;
}

export interface LocalAppServerOptions
  extends Omit<AppServerChildOptions, "launch"> {
  launch: AppServerChildOptions["launch"];
  /** Startup cancellation belongs to the composition root, not transport semantics. */
  signal?: AbortSignal;
  /**
   * Grace for each escalation step of initialization-failure cleanup only:
   * when the handshake fails or startup is cancelled, the owned child is sent
   * SIGTERM and stdin EOF, then `waitOrTerminate` escalates after this grace.
   * Normal shutdown never uses it; it waits for the child's own settlement.
   */
  terminationGraceMs?: number;
}

/**
 * What an App Server host is composed of.
 *
 * Ownership is supplied here and never derived later: the two static
 * constructors below are the only places it is decided, and each decides it
 * from the one fact that settles it — whether this process spawned the server.
 */
export interface AppServerHostComposition {
  endpoint?: string;
  client: AppServerClient;
  ownership: ProcessOwnership;
  /** The owned child, present exactly when `ownership` is `owned_child`. */
  child?: AppServerChild;
}

/**
 * The Session a notification addresses, if it addresses one.
 *
 * Session notifications do not all address a Session through an attachment
 * target: configuration application is scoped by Session, and
 * `session/summaryInvalidated` (Issue #386) is addressed by Session identity
 * alone, because durable Session metadata belongs to the Session rather than
 * to whichever Conversation of it a client displays. A source-scoped
 * notification (`configuration/sourceRepairChanged`, Issue #464) addresses a
 * configuration source and no Session at all.
 */
function sessionIdOf(notification: Notification): string | undefined {
  switch (notification.method) {
    case "configuration/changed":
      return notification.params.application.scope;
    case "configuration/sourceRepairChanged":
      return undefined;
    case "session/ownershipRetired":
    case "session/summaryInvalidated":
      return notification.params.session_id;
    default:
      return notification.params.target.session_id;
  }
}

/** Observes a source's latest native repair receipt. Presentation only. */
export type SourceRepairListener = (observation: SourceRepairObservation) => void;

export class AppServerHost {
  readonly endpoint: string | undefined;
  readonly client: AppServerClient;
  readonly ownership: ProcessOwnership;
  readonly #child: AppServerChild | undefined;
  readonly #sessions = new Map<string, AppServerSession>();
  /**
   * The newest native repair observation of each configuration source this
   * connection observed, by source. Source-scoped and Session-independent:
   * kept here so a notice that arrives before any presentation subscribes is
   * still there to present. Notices and authoritative reads are ordered by
   * their native ledger sequence, never by arrival: an older one — a read
   * answered after a newer notice, or a notice produced before a fresher read
   * — changes nothing. Finite (`retainSourceRepair`): the User and 64 Workspaces,
   * the least recently observed Workspace going first.
   */
  #sourceRepairs = new Map<string, SourceRepairObservation>();
  /**
   * The highest native sequence at which this connection forgot a source.
   * Whatever a source absent from `#sourceRepairs` held was current at or
   * before it, so a notice of an absent source ordered at or after it is
   * never older than what was forgotten. One ordered before it may be the
   * forgotten history of an evicted source, or the first notice of a source
   * never seen: it is ambiguous, never applied, and the source is recovered
   * by an authoritative read instead. One value, however many sources ever
   * came and went.
   */
  #forgottenThrough = "0";
  /**
   * This connection's Workspace reads in flight, each with the highest
   * sequence at which its own source was forgotten while it was in flight.
   * Anything forgotten before a read was sent is older than its answer, since
   * native snapshots after receiving it; anything forgotten meanwhile is
   * compared exactly. Each leaves when its read settles.
   */
  readonly #reads = new Set<{ key: string; forgotten: string }>();
  /** Workspaces whose ambiguous notice awaits its recovery read; one entry
   * per source, finite (`retainSourceRepair`), read one at a time. */
  #recoveries = new Map<string, SourceTarget>();
  #recovering = false;
  readonly #sourceRepairListeners = new Set<SourceRepairListener>();
  readonly #unknownModelSelections = new Set<string>();
  #shutdown: Promise<ChildExit | undefined> | undefined;
  readonly #exports = new Set<AbortController>();

  constructor(composition: AppServerHostComposition) {
    const { client, ownership, child } = composition;
    this.endpoint = composition.endpoint;
    if ((ownership === "owned_child") !== (child !== undefined)) {
      throw new Error(
        "an owned App Server host has a child process and an external one does not",
      );
    }
    client.uploadEndpoint = composition.endpoint;
    this.client = client;
    this.ownership = ownership;
    this.#child = child;

    // One subscription routes every notification. A Session notification names
    // its full attachment target; each attachment decides whether that target
    // is its own, so a superseded incarnation cannot reach its replacement.
    // Source-scoped notices go to the host's source observations instead.
    client.onNotification((notification) => {
      if (notification.method === "configuration/sourceRepairChanged") {
        const { target, receipt, sequence } = notification.params;
        this.#observeSourceRepair({ target, receipt: receipt ?? undefined, sequence });
        return;
      }
      const sessionId = sessionIdOf(notification);
      const session = sessionId === undefined ? undefined : this.#sessions.get(sessionId);
      session?.applyNotification(notification);
    });
  }

  /**
   * Spawns and owns one App Server child, speaking the protocol over stdio.
   *
   * Exactly one child exists for the lifetime of the TUI. Nothing about Session
   * navigation spawns another.
   */
  static async spawnLocal(options: LocalAppServerOptions): Promise<AppServerHost> {
    options.signal?.throwIfAborted();
    const child = AppServerChild.spawn(options);
    const abort = () => { child.requestShutdown(); child.closeStdin(); };
    options.signal?.addEventListener("abort", abort, { once: true });
    const transport = new StdioTransport({
      input: child.stdout,
      output: child.stdin,
      label: `stdio ${child.command}`,
    });
    // A child that dies settles pending requests with the real process cause
    // rather than a bare EOF — and still only as a process fact.
    void child.wait().then((exit) => {
      transport.reportProcessExit(exit.code, exit.signal, exit.spawnError);
    });

    try {
      const client = await AppServerClient.initialize({ transport });
      options.signal?.throwIfAborted();
      return new AppServerHost({ client, ownership: "owned_child", child });
    } catch (error) {
      // The child is ours, so a failed handshake must not leave it running.
      const stderr = child.stderrTail().text.trim();
      child.requestShutdown();
      child.closeStdin();
      await child.waitOrTerminate(options.terminationGraceMs);
      throw new Error(
        `could not start the App Server: ${(error as Error).message}${stderr.length > 0 ? `\n${stderr}` : ""}`,
        { cause: error },
      );
    } finally {
      options.signal?.removeEventListener("abort", abort);
    }
  }

  /**
   * Connects to an App Server someone else is running.
   *
   * This TUI owns no process here. Disconnecting, exiting, or losing the
   * network never stops that server and never stops its Sessions.
   */
  static async connectRemote(
    options: WebSocketTransportOptions,
  ): Promise<AppServerHost> {
    const transport = await WebSocketTransport.connect(options);
    try {
      const client = await AppServerClient.initialize({ transport });
      return new AppServerHost({ client, ownership: "external", endpoint: options.endpoint });
    } catch (error) {
      // Only this client's socket is closed. The server keeps running.
      transport.close();
      throw error;
    }
  }

  /** Destination stays entirely on the TUI machine, including remote mode. */
  async exportSession(sessionId: string, destination: string, signal?: AbortSignal): Promise<string> {
    const abort = new AbortController();
    this.#exports.add(abort);
    const combined = signal ? AbortSignal.any([signal, abort.signal]) : abort.signal;
    try {
      return await saveSessionArchive(destination, async () => {
        const { download } = await this.client.call("session/exportPrepare", { session_id: sessionId }, "session_archive");
        combined.throwIfAborted();
        if (this.ownership === "external" && !this.endpoint) throw new Error("Missing remote App Server endpoint");
        return fetch(archiveDownloadUrl(download, this.endpoint), { signal: combined, redirect: "error" });
      }, combined);
    } finally { this.#exports.delete(abort); }
  }

  // -------------------------------------------------------------------------
  // Configuration source repair observations (Issue #464)
  // -------------------------------------------------------------------------

  /** The App Server authority this connection negotiated; read-only. */
  get authorityId(): string {
    return this.client.authorityId;
  }

  /** The latest receipt of every source observed on this connection. */
  sourceRepairs(): SourceRepairObservation[] {
    return [...this.#sourceRepairs.values()];
  }

  /** Subscribes to changed source repair observations. */
  onSourceRepair(listener: SourceRepairListener): () => void {
    this.#sourceRepairListeners.add(listener);
    return () => this.#sourceRepairListeners.delete(listener);
  }

  /**
   * Reads sources authoritatively and folds their repair receipts in.
   *
   * The recovery path after a notification gap or a reconnect: a read, never
   * a guess. `configuration/sourcesRead` is read-only on the server, and
   * answers for exactly the canonical target it was asked about. An answer
   * older than what this connection meanwhile observed of the source — and
   * then forgot — changes nothing; it is still returned to the caller as the
   * snapshot it is.
   */
  async readSourceRepairs(target: SourceTarget = { kind: "user" }): Promise<SourceRepairRead> {
    const read = target.kind === "workspace" ? { key: sourceRepairKey(target), forgotten: "0" } : undefined;
    if (read !== undefined) this.#reads.add(read);
    try {
      const client = this.client;
      const authorityId = this.authorityId;
      const current = () => this.client === client && this.authorityId === authorityId && !client.closed;
      const result = await client.call("configuration/sourcesRead", { target }, "source_settings");
      const projection = await readSourceRepairFields(result.projection, async (reference, offset) =>
        (await client.call("configuration/sourceValueRead", { target, reference, offset, max_bytes: 65_536 }, "source_value_chunk")).chunk, current);
      const sequence = projection.repair_sequence;
      // The User is never forgotten, so nothing it held can be newer.
      this.#observeSourceRepair({ target: { kind: "user" }, receipt: projection.user_repair ?? undefined, sequence }, "0");
      if (read !== undefined) this.#observeSourceRepair({ target, receipt: projection.workspace_repair ?? undefined, sequence }, read.forgotten);
      return projection;
    } finally {
      if (read !== undefined) this.#reads.delete(read);
    }
  }

  /**
   * Applies one native observation unless it is not newer than what this
   * connection holds, or may have held, of its source. A held source orders
   * it by sequence alone: a higher sequence replaces it, even with the same
   * receipt or with no receipt on either side. Two observations at one
   * sequence agree; one that disagrees is inconsistent, never newer, so the
   * held one stays and a notice's source is read again. An absent one may
   * have been forgotten through `forgotten` — for a read, its own bound; for
   * a notice, the connection's frontier — and an observation older than that
   * is never applied; an ambiguous notice is recovered by reading its source
   * again.
   */
  #observeSourceRepair(observation: SourceRepairObservation, forgotten?: string): void {
    const key = sourceRepairKey(observation.target);
    const decision = sourceRepairDecision(this.#sourceRepairs.get(key), observation, forgotten, this.#forgottenThrough);
    if (decision === "recover") this.#recover(observation.target);
    if (decision !== "apply") return;
    const { retained, forgotten: evicted } = retainSourceRepair(this.#sourceRepairs, key, observation);
    this.#sourceRepairs = new Map(retained);
    if (evicted !== undefined) this.#forget(evicted);
    for (const listener of [...this.#sourceRepairListeners]) listener(observation);
  }

  /** Records that this connection no longer holds `evicted`'s source. */
  #forget({ target, sequence }: SourceRepairObservation): void {
    this.#forgottenThrough = laterRepairSequence(this.#forgottenThrough, sequence);
    const key = sourceRepairKey(target);
    for (const read of this.#reads) {
      if (read.key === key) read.forgotten = laterRepairSequence(read.forgotten, sequence);
    }
  }

  /**
   * Reads `target` again because a notice of it was ambiguous or disagreed
   * with what this connection holds at the same sequence. Sequential
   * and bounded: one read in flight, one queued entry per source; a source
   * noticed again while its read is in flight is read once more after it,
   * since that read may predate the notice. A failed read proves nothing,
   * and nothing is read once the connection closed.
   */
  #recover(target: SourceTarget): void {
    this.#recoveries = new Map(retainSourceRepair(this.#recoveries, sourceRepairKey(target), target).retained);
    if (this.#recovering) return;
    this.#recovering = true;
    void (async () => {
      for (let next = this.#recoveries.entries().next().value; next !== undefined && this.client.closed === undefined;
        next = this.#recoveries.entries().next().value) {
        this.#recoveries.delete(next[0]);
        await this.readSourceRepairs(next[1]).catch(() => undefined);
      }
      this.#recovering = false;
    })();
  }

  /** Every Session currently attached through this connection. */
  get attached(): readonly AppServerSession[] {
    return [...this.#sessions.values()];
  }

  /** The bounded stderr tail of an owned child; empty for an external server. */
  stderrTail(): { text: string; truncatedBytes: number } {
    return this.#child?.stderrTail() ?? { text: "", truncatedBytes: 0 };
  }

  /** The owned child's exit, when it has one. */
  get childExit(): ChildExit | undefined {
    return this.#child?.exited;
  }

  /** A bounded description of where this client is connected. */
  describe(): string {
    return this.ownership === "owned_child"
      ? `owned App Server child (pid ${this.#child?.pid ?? "unknown"})`
      : `external App Server at ${this.client.describeTransport()}`;
  }

  // -------------------------------------------------------------------------
  // Attachment
  // -------------------------------------------------------------------------

  /**
   * Attaches to a Session, or returns the attachment this connection already
   * holds for it.
   *
   * Attaching to a second Session leaves the first attached and running. This
   * is the whole point: the App Server hosts many Sessions, and focus is a
   * client concept.
   */
  async attach(
    sessionId: SessionId,
    nodeId?: SessionNodeId,
    previous?: AppServerSession,
  ): Promise<AppServerSession> {
    const existing = this.#sessions.get(sessionId);
    if (existing !== undefined && !existing.released && !existing.serverClosed) {
      if (nodeId === undefined || nodeId === existing.nodeId) return existing;
      throw new Error("use branch switching to open a different Session node");
    }
    const session = await AppServerSession.attach(this.client, sessionId, nodeId, session => this.#sessions.set(sessionId, session), previous, this.#unknownModelSelections);
    this.#sessions.set(sessionId, session);
    session.onClosed(() => {
      // Residency ended for this attachment. Drop the route so a later attach
      // installs a fresh one rather than reusing a target the server retired.
      if (this.#sessions.get(sessionId) === session) {
        this.#sessions.delete(sessionId);
      }
    });
    return session;
  }

  /** Carry local unanswered-request evidence across reconnection, not model authority. */
  inheritModelSelectionUncertainty(previous: AppServerHost): void {
    for (const sessionId of previous.#unknownModelSelections) {
      this.#unknownModelSelections.add(sessionId);
    }
  }

  /** User-confirmed branch switch; the manager owns native retirement. */
  async openNode(sessionId: SessionId, nodeId: SessionNodeId): Promise<AppServerSession> {
    const existing = this.#sessions.get(sessionId);
    if (existing !== undefined && !existing.released && !existing.serverClosed) {
      await existing.switchNode(nodeId);
      this.#sessions.delete(sessionId);
    }
    return this.attach(sessionId, nodeId);
  }

  /** The attachment this connection holds for a Session, if any. */
  attachment(sessionId: SessionId): AppServerSession | undefined {
    return this.#sessions.get(sessionId);
  }

  /**
   * Releases one attachment.
   *
   * Detach is not unload and not cancellation: the Session stays loaded and
   * keeps executing. This exists for the cases where releasing control is the
   * product intent, not for ordinary focus changes.
   */
  async detach(sessionId: SessionId): Promise<void> {
    const session = this.#sessions.get(sessionId);
    if (session === undefined) {
      return;
    }
    this.#sessions.delete(sessionId);
    await session.detach();
  }

  // -------------------------------------------------------------------------
  // Durable Session catalog
  //
  // These address the durable controller and take no attachment target: a
  // Session can be listed, read, renamed, forked or deleted without being
  // loaded at all.
  // -------------------------------------------------------------------------

  async listSessions(
    query?: string,
    offset = 0,
    limit = SESSION_PROJECTION_PAGE_LIMIT,
  ): Promise<{ sessions: SessionSummaryView[]; nextOffset?: number }> {
    const page = await this.client.call(
      "session/list",
      {
        query: query === undefined || query.length === 0 ? null : query,
        offset,
        limit,
      },
      "sessions",
    );
    return {
      sessions: page.sessions,
      nextOffset: page.next_offset ?? undefined,
    };
  }

  async readSession(sessionId: SessionId): Promise<SessionSnapshot> {
    const read = await this.client.call(
      "session/read",
      { session_id: sessionId },
      "session",
    );
    return read.session;
  }

  async createSession(settings: SessionPersistentState): Promise<SessionTransition> {
    return transitionOf(this.client,
      await this.client.call(
        "session/create",
        { settings },
        "session_transition",
      ),
    );
  }

  async renameSession(
    sessionId: SessionId,
    name: string,
  ): Promise<SessionSnapshot> {
    const renamed = await this.client.call(
      "session/name",
      { session_id: sessionId, name },
      "session",
    );
    return renamed.session;
  }

  async sessionTree(
    sessionId: SessionId,
    offset = 0,
    limit = SESSION_PROJECTION_PAGE_LIMIT,
  ): Promise<{ nodes: SessionNode[]; nextOffset?: number }> {
    const tree = await this.client.call(
      "session/tree",
      { session_id: sessionId, offset, limit },
      "tree",
    );
    return { nodes: tree.nodes, nextOffset: tree.next_offset ?? undefined };
  }

  /**
   * Copies a lineage at an exact revision into an independent Session.
   *
   * Without a boundary this clones the revision; with one it forks at that
   * exact historical user message.
   */
  async forkSession(
    sessionId: SessionId,
    surfaceRevision: SurfaceRevision,
    boundary?: import("../../../protocol/app-server/v57.ts").CanonicalContentLocator,
    nodeId?: SessionNodeId,
  ): Promise<SessionTransition> {
    return transitionOf(this.client,
      await this.client.call(
        "session/fork",
        {
          session_id: sessionId,
          node_id: nodeId ?? null,
          surface_revision: surfaceRevision,
          side: "before",
          boundary: boundary ?? null,
        },
        "session_transition",
      ),
    );
  }

  /** Creates a branch node inside the same Session graph. */
  async branchSession(
    sessionId: SessionId,
    nodeId: SessionNodeId,
    surfaceRevision: SurfaceRevision,
    boundary: import("../../../protocol/app-server/v57.ts").CanonicalContentLocator,
  ): Promise<SessionTransition> {
    return transitionOf(this.client,
      await this.client.call(
        "session/branch",
        {
          session_id: sessionId,
          node_id: nodeId,
          surface_revision: surfaceRevision,
          side: "before",
          boundary,
        },
        "session_transition",
      ),
    );
  }

  async previewSessionDeletion(
    sessionId: SessionId,
  ): Promise<SessionDeleteResult> {
    const previewed = await this.client.call(
      "session/deletePreview",
      { session_id: sessionId },
      "deletion",
    );
    return previewed.result;
  }

  async deleteSession(
    sessionId: SessionId,
    expectedTargetRevision: string,
  ): Promise<SessionDeleteResult> {
    this.client.setSessionDeleting(sessionId, true);
    const deleted = await this.client.call(
      "session/delete",
      {
        session_id: sessionId,
        expected_target_revision: expectedTargetRevision,
      },
      "deletion",
    );
    if (deleted.result.status === 'stale' || deleted.result.status === 'blocked') this.client.setSessionDeleting(sessionId, false);
    return deleted.result;
  }

  async recoverSessionDeletion(
    sessionId: SessionId,
  ): Promise<SessionDeleteResult> {
    const recovered = await this.client.call(
      "session/recoverDeletion",
      { session_id: sessionId },
      "deletion",
    );
    return recovered.result;
  }

  async readSettings(
    sessionId: SessionId,
  ): Promise<{ revision: string; settings: SessionPersistentState }> {
    const read = await this.client.call(
      "session/settings",
      { session_id: sessionId },
      "settings",
    );
    const settings = await materializeSettings(read.projection, async(reference,offset)=>(await this.client.call('session/settingsValueRead',{session_id:sessionId,reference,offset,max_bytes:65_536},'settings_value_chunk')).chunk, ()=>true);
    return { revision: read.projection.revision, settings };
  }

  // -------------------------------------------------------------------------
  // Shutdown
  // -------------------------------------------------------------------------

  /**
   * Ends this TUI's relationship with the App Server, according to ownership.
   *
   * For an **owned child** this is a process shutdown: the client closes the
   * child's stdin and sends SIGTERM, then waits for server-owned drain and exit. Work in flight may be
   * lost — because the process the TUI owns is intentionally ending, not
   * because a transport detach is execution authority.
   *
   * For an **external server** this is a disconnect and nothing more. The
   * server keeps running, every Session stays loaded, accepted turns keep
   * executing and pending interactions stay pending. Another client — or this
   * one, later — attaches and reads authoritative state.
   */
  shutdown(): Promise<ChildExit | undefined> {
    // Publish the shared promise before closing transports can call listeners.
    return this.#shutdown ??= Promise.resolve().then(() => this.#settle());
  }

  async #settle(): Promise<ChildExit | undefined> {
    for (const abort of this.#exports) abort.abort(new Error("TUI disconnected during export"));
    this.#sessions.clear();
    if (this.ownership === "external" || this.#child === undefined) {
      await this.client.close();
      return undefined;
    }
    this.#child.requestShutdown();
    this.#child.closeStdin();
    const exit = await this.#child.wait();
    await this.client.close();
    return exit;
  }
}

async function transitionOf(client: AppServerClient, result: {
  session: SessionSnapshot;
  editor?: import('../../../protocol/app-server/v57.ts').EditorLocator | null;
  durability_diagnostic?: string | null;
}): Promise<SessionTransition> {
  return {
    session: result.session,
    editorContent: result.editor ? await readEditor(result.editor,
      async (block_index, offset) => (await client.call('session/editorRead', { locator: result.editor!, block_index, offset, max_bytes: 65_536 }, 'editor_page')).page,
      () => true) : undefined,
    durabilityDiagnostic: result.durability_diagnostic ?? undefined,
  };
}

export { TransportClosedError };
