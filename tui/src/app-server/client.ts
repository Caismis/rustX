import { transferUpload } from '../../../protocol/app-server/upload.ts';
import { requestEffect, requestLane, type MethodEffect, type RequestLane } from '../../../protocol/app-server/method-policy.ts';
import type { Request1 } from '../../../protocol/app-server/v57.ts';
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

import { retainSourceRepair, sourceRepairKey } from "../../../protocol/app-server/source-repair.ts";
import { decodeProtocolMessage } from "../protocol/decoder.ts";

import {
  describeRpcError,
  isFailure,
  isNotification,
  isResponse,
  type AttachmentTarget,
  type ClientIdentity,
  type MethodName,
  type MethodParams,
  type MethodResult,
  type Notification,
  type PresentationCapabilities,
  type AppServerRequestId,
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
export const APP_SERVER_PROTOCOL_VERSION = 57;

/** Notifications retained before the first subscription; level-triggered
 * owners re-announce or reread anything beyond this. */
const UNROUTED_NOTIFICATIONS = 64;

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
 * not resend it and will not report it as failed. Readback after reconnecting
 * restores the current projection, but does not establish this request's outcome:
 * native work can survive its waiter and commit after the readback.
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

/** The requests a client may cancel by id on its own connection. */
export type DeliveryMethod = "delivery/read" | "delivery/locate";

interface PendingRequest {
  readonly method: MethodName;
  readonly expect: ResultType;
  readonly effect: MethodEffect;
  resolve: (result: MethodResult) => void;
  reject: (error: Error) => void;
}

/** At most sixteen requests cross one connection: the App Server's
 * `IN_FLIGHT_REQUESTS`, which ends a connection that sends a seventeenth.
 * This client counts a request until its response arrives, which is never
 * earlier than the server frees its slot, so staying within the lanes below
 * never trips that limit. Observation and admission cannot occupy the
 * capacity reserved for lifecycle control and inspection. Rejection is
 * local, before id allocation/send; no ordinary lane queues or replays work.
 *
 * `cancel` is one slot only `delivery/cancel` may use, so no number of
 * ordinary requests can stop a delivery's cancellation from reaching the
 * server. Cancellations wait for it in order, at most one per in-flight
 * delivery request; the server answers each at once, without awaiting work. */
const REQUEST_CAPACITY = { observation: 8, rpc: 6, control: 2 } as const satisfies Record<RequestLane, number>;

type NotificationListener = (notification: Notification) => void;
type CloseListener = (error: TransportClosedError) => void;

export interface AppServerClientOptions {
  transport: AppServerTransport;
  identity?: ClientIdentity;
  presentation?: PresentationCapabilities;
}

export class AppServerClient {
  uploadCarrier = transferUpload;
  uploadEndpoint?: string;
  readonly #transport: AppServerTransport;
  readonly #pending = new Map<AppServerRequestId, PendingRequest>();
  /** Aborted delivery requests whose cancellation awaits the cancel slot, in
   * abort order. An entry leaves when its cancel is sent or its request
   * settles, so this never outgrows the in-flight delivery requests. */
  readonly #cancels = new Set<AppServerRequestId>();
  readonly #notificationListeners = new Set<NotificationListener>();
  /**
   * Notifications that arrived before anything subscribed, replayed to the
   * first subscriber. A server may notify right after `initialize` answers,
   * before the composition root has built the host that routes notifications;
   * that gap must not lose a level-triggered notice. Finite, and retired by
   * the first subscription.
   *
   * Source repair observations are kept apart, coalesced to the latest one
   * per source — a withdrawal included — so no amount of unrelated traffic
   * crowds them out. The server advances its per-source delivery cursor as
   * it sends one and never sends it again, so a dropped observation would be
   * lost for good; the latest per source is all a subscriber needs. Bounded
   * by `retainSourceRepair`: the least recently observed Workspace goes beyond 64,
   * as the native ledger forgets it; the User never does. A source dropped
   * here was never observed by a subscriber, so nothing it held can go stale.
   */
  #unrouted: { notifications: Notification[]; sourceRepairs: Map<string, Notification> } | undefined = {
    notifications: [],
    sourceRepairs: new Map(),
  };
  readonly #closeListeners = new Set<CloseListener>();
  #nextAppServerRequestId = 1;
  #capabilities: ServerCapabilities | undefined;
  #authorityId = "";
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
    try {
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
      if (!initialized.authority_id) throw new Error("The App Server supplied no process authority identity.");
      client.#capabilities = initialized.capabilities;
      client.#authorityId = initialized.authority_id;
    } catch (error) {
      await client.close();
      throw error;
    }
    return client;
  }

  /** Process authority namespaces Runtime incarnations across restarts. */
  get authorityId(): string { return this.#authorityId; }

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
  readonly #deletingSessions = new Set<string>();
  setSessionDeleting(id: string, deleting: boolean): void {
    if (deleting) this.#deletingSessions.add(id); else this.#deletingSessions.delete(id);
  }

  async call<M extends Exclude<MethodName, "delivery/cancel">, T extends ResultType>(
    method: M,
    params: MethodParams<M>,
    expect: T,
  ): Promise<ResultOf<T>> {
    if ('target' in params && 'session_id' in params.target && this.#deletingSessions.has(params.target.session_id) && method !== 'session/detach') {
      throw new Error('Session deletion has disabled controls. Verify its outcome before continuing.');
    }
    const result = await this.#request(method, params, expect);
    return result as ResultOf<T>;
  }

  /**
   * Issues one delivery request whose cancellation `signal` owns.
   *
   * Aborting sends `delivery/cancel` for exactly this request id, once,
   * through the reserved cancel slot: immediately, or after the
   * cancellations aborted before it, unless this request settles first. The
   * request still settles exactly once, with the server's terminal outcome:
   * its result when publication committed first, otherwise
   * `delivery_cancelled`. Its correlation entry stays until then, so the late
   * response is never an unknown id. A signal aborted before sending sends
   * nothing. A healthy server answers every cancel; one that refuses a
   * cancel breaks this contract and ends the connection, which settles the
   * request with that terminal failure.
   */
  async observe(params: MethodParams<'agent/observe'>, signal: AbortSignal): Promise<ResultOf<'agent_observe'>> {
    return await this.#request('agent/observe', params, 'agent_observe', signal) as ResultOf<'agent_observe'>;
  }

  async callDelivery<M extends DeliveryMethod, T extends ResultType>(
    method: M,
    params: MethodParams<M>,
    expect: T,
    signal: AbortSignal,
  ): Promise<ResultOf<T>> {
    const result = await this.#request(method, params, expect, signal);
    return result as ResultOf<T>;
  }

  /** Subscribes to server notifications. Returns an unsubscribe function. */
  onNotification(listener: NotificationListener): () => void {
    this.#notificationListeners.add(listener);
    // The first subscriber drains the buffer before anything is dispatched
    // to it directly: a notification arriving during the replay — a newer
    // observation of a source already buffered included — joins the buffer
    // and is replayed after everything older, so the handoff never delivers
    // an older observation after a newer one, and never drops or repeats one.
    const unrouted = this.#unrouted;
    if (unrouted !== undefined) {
      for (;;) {
        const next = unrouted.notifications.shift() ?? first(unrouted.sourceRepairs);
        if (next === undefined) break;
        listener(next);
      }
      this.#unrouted = undefined;
    }
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

  #request(method: MethodName, params: unknown, expect: ResultType, signal?: AbortSignal): Promise<MethodResult> {
    if (this.#closed !== undefined) {
      // After termination a new request fails immediately rather than waiting
      // for a peer that will never answer. Nothing was sent, so nothing is
      // uncertain.
      return Promise.reject(this.#closed);
    }
    if (signal?.aborted) return Promise.reject(signal.reason);

    const lane = requestLane(method);
    if (this.#occupied(lane) >= REQUEST_CAPACITY[lane]) {
      return Promise.reject(new Error(`Client ${lane} capacity reached. Request not sent; inspect current operations before retrying.`));
    }

    const id = this.#nextAppServerRequestId;
    this.#nextAppServerRequestId += 1;

    return new Promise<MethodResult>((resolve, reject) => {
      const cancel = () => {
        if (!this.#pending.has(id)) return;
        this.#cancels.add(id);
        this.#sendCancel();
      };
      const release = () => {
        signal?.removeEventListener("abort", cancel);
        this.#cancels.delete(id);
        // A settled cancel frees the slot for the next waiting one.
        if (requestLane(method) === "control") this.#sendCancel();
      };
      this.#pending.set(id, {
        method,
        expect,
        effect: requestEffect({ method, params } as Request1),
        resolve: (result) => { release(); resolve(result); },
        reject: (error) => { release(); reject(error); },
      });
      signal?.addEventListener("abort", cancel, { once: true });
      void this.#transport
        .send({ jsonrpc: "2.0", id, method, params })
        .catch(() => {
          // The transport already terminated and settled every pending
          // request, including this one, with the terminal cause.
        });
    });
  }

  #occupied(lane: keyof typeof REQUEST_CAPACITY): number {
    return [...this.#pending.values()].filter(request => requestLane(request.method) === lane).length;
  }

  /** Sends the oldest waiting cancellation when the cancel slot is free. Its
   * answer is not the outcome — the delivery request's own terminal response
   * is — but a refusal means the server dropped a cancellation it owed. */
  #sendCancel(): void {
    if (this.#closed !== undefined || this.#occupied("control") >= REQUEST_CAPACITY.control) return;
    const [id] = this.#cancels;
    if (id === undefined) return;
    this.#cancels.delete(id);
    const observing = this.#pending.get(id)?.method === 'agent/observe';
    this.#request(observing ? 'agent/observeCancel' : 'delivery/cancel', { request_id: id }, observing ? 'agent_observe_cancelled' : 'delivery_cancel').catch((error: unknown) => {
      if (error instanceof AppServerRequestError) {
        this.#fail(`the App Server refused delivery/cancel for request ${String(id)}: ${error.message}`);
      }
      // Otherwise the connection already ended, settling the request too.
    });
  }

  #dispatch(untrusted: TransportRecord): void {
    if (this.#closed !== undefined) {
      return;
    }

    const record = decodeProtocolMessage(untrusted);
    if (record === undefined) {
      this.#fail("invalid App Server v57 protocol message");
      return;
    }

    if (isNotification(record)) {
      if (this.#unrouted !== undefined) {
        if (record.method === "configuration/sourceRepairChanged") {
          this.#unrouted.sourceRepairs = new Map(retainSourceRepair(this.#unrouted.sourceRepairs, sourceRepairKey(record.params.target), record).retained);
        } else if (this.#unrouted.notifications.length < UNROUTED_NOTIFICATIONS) {
          this.#unrouted.notifications.push(record);
        }
        return;
      }
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
    if (!isFailure(record) && record.result.type !== pending.expect) {
      this.#fail(`${pending.method} returned ${record.result.type} instead of ${pending.expect}`);
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
        request.effect === "side_effecting"
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

/** Whether a delivery request answered that its cancellation won. */
export function isDeliveryCancelled(error: unknown): boolean {
  return error instanceof AppServerRequestError && error.kind === "delivery_cancelled";
}

/** Whether a side-effecting request's outcome is genuinely unknown. */
export function isUncertainOutcome(error: unknown): boolean {
  return error instanceof UncertainOutcomeError;
}

/** Removes and returns the oldest entry of `entries`, if any. */
function first<V>(entries: Map<string, V>): V | undefined {
  for (const [key, value] of entries) {
    entries.delete(key);
    return value;
  }
  return undefined;
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
export type { AttachmentTarget };
