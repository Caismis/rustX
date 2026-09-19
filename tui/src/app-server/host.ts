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
  type MessageId,
  type UserInputBlock,
} from "../protocol/app-server.ts";
import { AppServerClient } from "./client.ts";
import {
  AppServerChild,
  type AppServerChildOptions,
  type ChildExit,
} from "./child-process.ts";
import { AppServerSession, SESSION_PROJECTION_PAGE_LIMIT } from "./session.ts";
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
  /** How long the owned child gets to exit after the shutdown sequence. */
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
  client: AppServerClient;
  ownership: ProcessOwnership;
  /** The owned child, present exactly when `ownership` is `owned_child`. */
  child?: AppServerChild;
  terminationGraceMs?: number;
}

export class AppServerHost {
  readonly client: AppServerClient;
  readonly ownership: ProcessOwnership;
  readonly #child: AppServerChild | undefined;
  readonly #sessions = new Map<string, AppServerSession>();
  #shutdown: Promise<ChildExit | undefined> | undefined;

  constructor(composition: AppServerHostComposition) {
    const { client, ownership, child } = composition;
    if ((ownership === "owned_child") !== (child !== undefined)) {
      throw new Error(
        "an owned App Server host has a child process and an external one does not",
      );
    }
    this.client = client;
    this.ownership = ownership;
    this.#child = child;

    // One subscription routes every notification. A notification names its full
    // attachment target; each attachment decides whether that target is its
    // own, so a superseded incarnation cannot reach its replacement.
    client.onNotification((notification) => {
      const session = this.#sessions.get(notification.params.target.session_id);
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
      return new AppServerHost({
        client,
        ownership: "owned_child",
        child,
        terminationGraceMs: options.terminationGraceMs,
      });
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
      return new AppServerHost({ client, ownership: "external" });
    } catch (error) {
      // Only this client's socket is closed. The server keeps running.
      transport.close();
      throw error;
    }
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
  ): Promise<AppServerSession> {
    const existing = this.#sessions.get(sessionId);
    if (existing !== undefined && !existing.released && !existing.serverClosed) {
      if (nodeId === undefined || nodeId === existing.nodeId) return existing;
      throw new Error("use branch switching to open a different Session node");
    }
    const session = await AppServerSession.attach(this.client, sessionId, nodeId);
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
    return transitionOf(
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
    boundary?: MessageId,
    nodeId?: SessionNodeId,
  ): Promise<SessionTransition> {
    return transitionOf(
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
    boundary: MessageId,
  ): Promise<SessionTransition> {
    return transitionOf(
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
      "settings/read",
      { session_id: sessionId },
      "settings",
    );
    return { revision: read.revision, settings: read.settings };
  }

  async replaceSettings(
    sessionId: SessionId,
    expectedRevision: string,
    settings: SessionPersistentState,
  ): Promise<string> {
    const replaced = await this.client.call(
      "settings/replace",
      {
        session_id: sessionId,
        expected_revision: expectedRevision,
        settings,
      },
      "settings_replaced",
    );
    return replaced.revision;
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

function transitionOf(result: {
  session: SessionSnapshot;
  editor_content?: UserInputBlock[] | null;
  durability_diagnostic?: string | null;
}): SessionTransition {
  return {
    session: result.session,
    editorContent: result.editor_content ?? undefined,
    durabilityDiagnostic: result.durability_diagnostic ?? undefined,
  };
}

export { TransportClosedError };
