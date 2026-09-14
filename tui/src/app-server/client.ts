/**
 * The one typed App Server client.
 *
 * ```text
 *   stdio JSONL ----\
 *                    >--- AppServerTransport ---> AppServerClient ---> Sessions
 *   WebSocket   ----/
 * ```
 *
 * Exactly one object owns every protocol-level client concern: `initialize` and
 * version negotiation, JSON-RPC id allocation, the pending-request map,
 * response correlation, notification dispatch by attachment target, and
 * terminal settlement. Both transports feed this same object, so there is no
 * "local client" and no "remote client" — there is one client and two ways for
 * its bytes to travel.
 *
 * # Correlation
 *
 * ```text
 * call(method, params) -> id allocated here -> transport.send
 *                                                    |
 * inbound message -> has an id? --yes--> settle that pending request
 *                            |
 *                           no --------> route the notification by target
 * ```
 *
 * Requests may be pipelined and responses may complete out of order; the
 * JSON-RPC id is the only correlation. Repeating an id would not deduplicate a
 * mutation, so ids are allocated once, monotonically, and never reused.
 *
 * # Lost responses
 *
 * > Losing a response does not prove that a side-effecting request was never
 * > accepted.
 *
 * This client therefore **never replays a request**. When a transport dies with
 * requests in flight, each pending request settles exactly once, and a request
 * that could have changed server state settles as {@link UncertainOutcomeError}
 * rather than as a failure — because "it did not happen" is a claim this client
 * is not entitled to make. Recovery is always the same: reconnect, reinitialize,
 * re-attach, read authoritative state, and repair the projection from it. It is
 * never to resend.
 */

import {
  describeRpcError,
  isFailure,
  isNotification,
  isResponse,
  sameTarget,
  type AttachmentTarget,
  type ClientIdentity,
  type MethodName,
  type MethodParams,
  type Notification,
  type PresentationCapabilities,
  type RequestId,
  type ResultOf,
  type ResultType,
  type RpcError,
  type ServerCapabilities,
} from "../protocol/app-server.ts";
import {
  TransportClosedError,
  type AppServerTransport,
  type TransportRecord,
} from "./transport.ts";

/** The protocol version this client speaks. Independent of every other version. */
export const APP_SERVER_PROTOCOL_VERSION = 1;

/** How this client identifies itself in `initialize`. */
export const CLIENT_IDENTITY: ClientIdentity = {
  name: "rustx-tui",
  version: "0.1.0",
};

/** What this terminal client can render. Never an authorization. */
export const PRESENTATION_CAPABILITIES: PresentationCapabilities = {
  images: false,
  questionnaires: true,
  reviews: true,
};

/**
 * A typed semantic failure returned by the App Server.
 *
 * Deliberately distinct from {@link TransportClosedError}: a protocol error is
 * a well-formed answer from a healthy server, and the connection stays usable
 * afterwards.
 */
export class AppServerRequestError extends Error {
  readonly error: RpcError;
  readonly method: MethodName;

  constructor(method: MethodName, error: RpcError) {
    super(describeRpcError(error));
    this.name = "AppServerRequestError";
    this.error = error;
    this.method = method;
  }

  /** The closed typed failure kind, when the server supplied one. */
  get kind(): string | undefined {
    return this.error.data?.kind;
  }
}

/**
 * A side-effecting request whose response was lost with the connection.
 *
 * The request may have been accepted, executed and committed. This client will
 * not resend it and will not report it as failed; the caller's only sound move
 * is to read authoritative state after reconnecting.
 */
export class UncertainOutcomeError extends Error {
  readonly method: MethodName;
  /** The transport failure that lost the response. */
  readonly transportFailure: TransportClosedError;

  constructor(method: MethodName, cause: TransportClosedError) {
    super(
      `the connection ended before ${method} was answered; whether the server accepted it is unknown`,
      { cause },
    );
    this.name = "UncertainOutcomeError";
    this.method = method;
    this.transportFailure = cause;
  }
}

/**
 * The methods whose outcome is unknown when their response is lost.
 *
 * Every method that can change server state is listed. A read whose response is
 * lost can simply be issued again after reconnecting — nothing happened — so
 * reads are deliberately absent, and the default for an unlisted method is the
 * safe one only because the list is exhaustive over the mutating vocabulary.
 */
export const SIDE_EFFECTING_METHODS: ReadonlySet<MethodName> = new Set<MethodName>([
  "session/create",
  "session/name",
  "session/fork",
  "session/branch",
  "session/delete",
  "session/recoverDeletion",
  "session/attach",
  "session/detach",
  "session/unload",
  "turn/start",
  "turn/steer",
  "turn/cancel",
  "interaction/respond",
  "interaction/cancel",
  "settings/replace",
  "settings/setModel",
  "settings/setApprovalMode",
  "settings/saveDefault",
  "resources/reload",
  "context/compact",
  "goal/control",
  "background/cancel",
  "subagent/cancel",
  "subagent/disposeWorkspace",
]);

interface PendingRequest {
  readonly method: MethodName;
  resolve: (result: unknown) => void;
  reject: (error: Error) => void;
}

type NotificationListener = (notification: Notification) => void;
type CloseListener = (error: TransportClosedError) => void;

export interface AppServerClientOptions {
  transport: AppServerTransport;
  identity?: ClientIdentity;
  presentation?: PresentationCapabilities;
}

export class AppServerClient {
  readonly #transport: AppServerTransport;
  readonly #pending = new Map<RequestId, PendingRequest>();
  readonly #notificationListeners = new Set<NotificationListener>();
  readonly #closeListeners = new Set<CloseListener>();
  #nextRequestId = 1;
  #capabilities: ServerCapabilities | undefined;
  #closed: TransportClosedError | undefined;

  private constructor(transport: AppServerTransport) {
    this.#transport = transport;
    transport.onMessage((record) => this.#dispatch(record));
    transport.onClose((error) => this.#settleTerminally(error));
  }

  /**
   * Negotiates the protocol version and returns a live client.
   *
   * A version the server does not support fails here, explicitly. There is no
   * downgrade: a client that guessed at an older vocabulary would be speaking a
   * protocol neither side agreed to.
   */
  static async initialize(
    options: AppServerClientOptions,
  ): Promise<AppServerClient> {
    const client = new AppServerClient(options.transport);
    const initialized = await client.call("initialize", {
      protocol_version: APP_SERVER_PROTOCOL_VERSION,
      client: options.identity ?? CLIENT_IDENTITY,
      presentation: options.presentation ?? PRESENTATION_CAPABILITIES,
    }, "initialized");
    if (initialized.protocol_version !== APP_SERVER_PROTOCOL_VERSION) {
      throw new Error(
        `the App Server negotiated protocol ${initialized.protocol_version}, this client speaks ${APP_SERVER_PROTOCOL_VERSION}`,
      );
    }
    client.#capabilities = initialized.capabilities;
    return client;
  }

  /** The server capabilities advertised at initialization. */
  get capabilities(): ServerCapabilities | undefined {
    return this.#capabilities;
  }

  /** The terminal failure, once the connection has one. */
  get closed(): TransportClosedError | undefined {
    return this.#closed;
  }

  /** How many requests are awaiting a correlated response. */
  get pendingCount(): number {
    return this.#pending.size;
  }

  /** A bounded transport description for diagnostics. */
  describeTransport(): string {
    return this.#transport.describe();
  }

  /**
   * Issues one request and resolves with its correlated, type-checked result.
   *
   * `expect` is the result discriminator this method is defined to return. A
   * different discriminator is a protocol violation rather than something to
   * interpret loosely, so it fails instead of being coerced.
   */
  async call<M extends MethodName, T extends ResultType>(
    method: M,
    params: MethodParams<M>,
    expect: T,
  ): Promise<ResultOf<T>> {
    const result = await this.#request(method, params);
    const typed = result as { type?: unknown };
    if (typed.type !== expect) {
      throw new Error(
        `${method} returned ${String(typed.type)} instead of ${expect}`,
      );
    }
    return result as ResultOf<T>;
  }

  /** Subscribes to server notifications. Returns an unsubscribe function. */
  onNotification(listener: NotificationListener): () => void {
    this.#notificationListeners.add(listener);
    return () => this.#notificationListeners.delete(listener);
  }

  /** Subscribes to terminal connection failure. */
  onClose(listener: CloseListener): () => void {
    this.#closeListeners.add(listener);
    if (this.#closed !== undefined) {
      listener(this.#closed);
    }
    return () => this.#closeListeners.delete(listener);
  }

  /**
   * Ends this client's transport.
   *
   * A client-side transport action and nothing more. It does not detach an
   * attachment on the server's behalf, cancel work, or stop a process — see
   * `./host.ts` for the operation that ends an owned child.
   */
  close(): void | Promise<void> {
    return this.#transport.close();
  }

  #request(method: MethodName, params: unknown): Promise<unknown> {
    if (this.#closed !== undefined) {
      // After termination a new request fails immediately rather than waiting
      // for a peer that will never answer. Nothing was sent, so nothing is
      // uncertain.
      return Promise.reject(this.#closed);
    }

    const id = this.#nextRequestId;
    this.#nextRequestId += 1;

    return new Promise<unknown>((resolve, reject) => {
      this.#pending.set(id, { method, resolve, reject });
      void this.#transport
        .send({ jsonrpc: "2.0", id, method, params })
        .catch(() => {
          // The transport already terminated and settled every pending
          // request, including this one, with the terminal cause.
        });
    });
  }

  #dispatch(record: TransportRecord): void {
    if (this.#closed !== undefined) {
      return;
    }

    if (isNotification(record)) {
      for (const listener of [...this.#notificationListeners]) {
        listener(record);
      }
      return;
    }

    if (!isResponse(record)) {
      this.#fail("a protocol message is neither a correlated response nor a notification");
      return;
    }

    if (record.id === null || record.id === undefined) {
      // A failure with no correlation is a protocol-level envelope error the
      // server could not attribute. It belongs to the connection, not to one
      // request, so guessing a victim would be worse than failing.
      const detail = isFailure(record)
        ? describeRpcError(record.error)
        : "an uncorrelated response";
      this.#fail(`the App Server reported an uncorrelated protocol error: ${detail}`);
      return;
    }

    const pending = this.#pending.get(record.id);
    if (pending === undefined) {
      // An unknown or duplicate response id means the peer is not speaking the
      // correlation contract. Guessing which request it answers would be worse
      // than failing.
      this.#fail(`the App Server answered unknown request id ${String(record.id)}`);
      return;
    }
    this.#pending.delete(record.id);

    if (isFailure(record)) {
      pending.reject(new AppServerRequestError(pending.method, record.error));
      return;
    }
    pending.resolve(record.result);
  }

  #fail(message: string): void {
    this.#settleTerminally(
      new TransportClosedError("protocol_error", message),
    );
    this.#transport.close();
  }

  #settleTerminally(error: TransportClosedError): void {
    if (this.#closed !== undefined) {
      return;
    }
    this.#closed = error;

    // Every pending request settles exactly once, explicitly. A request that
    // could have changed server state settles as *unknown*, never as failed:
    // the response was lost, which is not evidence that the mutation was not
    // accepted.
    const pending = [...this.#pending.values()];
    this.#pending.clear();
    for (const request of pending) {
      request.reject(
        SIDE_EFFECTING_METHODS.has(request.method)
          ? new UncertainOutcomeError(request.method, error)
          : error,
      );
    }

    for (const listener of [...this.#closeListeners]) {
      listener(error);
    }
  }
}

/** Whether an error ended the connection rather than answering a request. */
export function isConnectionClosed(error: unknown): boolean {
  return (
    error instanceof TransportClosedError || error instanceof UncertainOutcomeError
  );
}

/** Whether a side-effecting request's outcome is genuinely unknown. */
export function isUncertainOutcome(error: unknown): boolean {
  return error instanceof UncertainOutcomeError;
}

/** Whether the server asked for an authoritative projection repair. */
export function isResyncRequired(error: unknown): boolean {
  return (
    error instanceof AppServerRequestError && error.kind === "resync_required"
  );
}

/** Whether the addressed attachment or incarnation has been replaced. */
export function isStaleAttachment(error: unknown): boolean {
  return (
    error instanceof AppServerRequestError &&
    (error.kind === "stale_attachment" || error.kind === "stale_runtime")
  );
}

/** Whether another client already holds this Session's writable control. */
export function isControllerInUse(error: unknown): boolean {
  return (
    error instanceof AppServerRequestError && error.kind === "controller_in_use"
  );
}

/** Whether two targets address the same attachment in every identity domain. */
export { sameTarget };
export type { AttachmentTarget };
