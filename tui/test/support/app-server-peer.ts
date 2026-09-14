/**
 * Scripted App Server peers for the deterministic client suites.
 *
 * Two doubles, at two different boundaries:
 *
 * ```text
 * FakeTransport        an AppServerTransport, with no bytes at all
 *                      -> proves client semantics: correlation, routing,
 *                         fencing, settlement, no-replay
 *
 * ScriptedAppServer    real JSONL over real pipes
 *                      -> proves the stdio transport: framing, EOF, stderr
 * ```
 *
 * A test drives either one by writing exact protocol messages, so ordering is
 * decided by the script rather than by timing. Nothing here sleeps, and no
 * test in this package proves an ordering with a timer.
 *
 * Neither double is a runtime. Neither holds conversation state, and neither
 * decides any semantics: they answer exactly what a test tells them to answer.
 */

import { PassThrough } from "node:stream";

import { encodeRecord } from "../../src/protocol/jsonl.ts";
import {
  BaseTransport,
  TransportClosedError,
  type TransportCloseReason,
} from "../../src/app-server/transport.ts";
import type {
  AttachmentTarget,
  MethodName,
  MethodParams,
  MethodResult,
  Notification,
  Request,
  RequestId,
  RpcError,
  RuntimeClientCursor,
  RuntimeClientEvent,
} from "../../src/protocol/app-server.ts";

/** One request the client wrote, already decoded. */
export type ObservedRequest = Request;

/** The shared "what did the client ask for" bookkeeping of both doubles. */
class RequestLog {
  readonly #requests: ObservedRequest[] = [];
  readonly #waiters: Array<() => void> = [];

  get requests(): readonly ObservedRequest[] {
    return this.#requests;
  }

  /** Every request for one method, in write order. */
  matching(method: string): readonly ObservedRequest[] {
    return this.#requests.filter((request) => request.method === method);
  }

  /** How many times the client asked for one method. */
  count(method: string): number {
    return this.matching(method).length;
  }

  record(request: ObservedRequest): void {
    this.#requests.push(request);
    for (const waiter of this.#waiters.splice(0)) {
      waiter();
    }
  }

  /**
   * Resolves once at least `count` requests have been written.
   *
   * This is a data barrier, not a delay: the promise settles on the write
   * itself, so a test never guesses how long a write takes.
   */
  async awaitRequests(count: number): Promise<readonly ObservedRequest[]> {
    while (this.#requests.length < count) {
      await new Promise<void>((resolve) => this.#waiters.push(resolve));
    }
    return this.#requests;
  }

  /** Resolves once the client has issued `count` requests for one method. */
  async awaitMethod(
    method: string,
    count = 1,
  ): Promise<readonly ObservedRequest[]> {
    while (this.count(method) < count) {
      await new Promise<void>((resolve) => this.#waiters.push(resolve));
    }
    return this.matching(method);
  }
}

/**
 * An `AppServerTransport` with no bytes.
 *
 * Everything below the typed client is removed, so a test that exercises
 * correlation, routing or settlement cannot accidentally be testing framing.
 */
export class FakeTransport extends BaseTransport {
  readonly log = new RequestLog();
  #disposed = false;

  override describe(): string {
    return "fake";
  }

  /** How many requests for one method the client has written. */
  transportCount(method: string): number {
    return this.log.count(method);
  }

  /** Whether the transport released its resources. */
  get disposed(): boolean {
    return this.#disposed;
  }

  protected override writeMessage(message: unknown): Promise<void> {
    this.log.record(message as ObservedRequest);
    return Promise.resolve();
  }

  protected override disposeTransport(): void {
    this.#disposed = true;
  }

  /** Delivers one protocol message to the client. */
  override deliver(record: object): void {
    super.deliver(record);
  }

  /** Answers one request with a typed result. */
  respond(id: RequestId, result: MethodResult): void {
    this.deliver({ jsonrpc: "2.0", id, result });
  }

  /** Answers one request with a typed protocol failure. */
  respondError(id: RequestId, error: RpcError): void {
    this.deliver({ jsonrpc: "2.0", id, error });
  }

  /** Publishes one routed observation. */
  emit(
    target: AttachmentTarget,
    cursor: RuntimeClientCursor,
    event: RuntimeClientEvent,
  ): void {
    this.deliver(notification("session/event", { target, cursor, event }));
  }

  /** Publishes one routed notification of any method. */
  notify(notificationMessage: Notification): void {
    this.deliver(notificationMessage);
  }

  /** Ends the transport as a terminal failure of the given kind. */
  fail(reason: TransportCloseReason, message = "scripted transport failure"): void {
    this.terminate(new TransportClosedError(reason, message));
  }
}

/**
 * The typed `params` of one observed request.
 *
 * `Request` is a union over the whole method vocabulary, so reading a
 * method-specific field needs the discriminator. Asserting it here keeps every
 * call site honest about which method it is inspecting.
 */
export function paramsOf<M extends MethodName>(
  request: ObservedRequest,
  method: M,
): MethodParams<M> {
  if (request.method !== method) {
    throw new Error(`expected a ${method} request, observed ${request.method}`);
  }
  return request.params as MethodParams<M>;
}

/** Builds one well-formed notification envelope. */
export function notification<M extends Notification["method"]>(
  method: M,
  params: Extract<Notification, { method: M }>["params"],
): Notification {
  return { jsonrpc: "2.0", method, params } as Notification;
}

/**
 * A scripted App Server speaking real JSONL over real pipes.
 *
 * This is the double for the *transport* boundary: framing, record splitting,
 * EOF and stderr separation are all genuinely exercised.
 */
export class ScriptedAppServer {
  /** Bytes the App Server would write; the client reads this. */
  readonly serverOutput = new PassThrough();
  /** Bytes the client writes; the peer reads this. */
  readonly clientOutput = new PassThrough();
  readonly log = new RequestLog();
  #pendingBytes = "";

  constructor() {
    this.clientOutput.on("data", (chunk: Buffer) => {
      this.#pendingBytes += chunk.toString("utf8");
      for (;;) {
        const lf = this.#pendingBytes.indexOf("\n");
        if (lf === -1) {
          break;
        }
        const record = this.#pendingBytes.slice(0, lf);
        this.#pendingBytes = this.#pendingBytes.slice(lf + 1);
        this.log.record(JSON.parse(record) as ObservedRequest);
      }
    });
  }

  get requests(): readonly ObservedRequest[] {
    return this.log.requests;
  }

  awaitRequests(count: number): Promise<readonly ObservedRequest[]> {
    return this.log.awaitRequests(count);
  }

  awaitMethod(method: string, count = 1): Promise<readonly ObservedRequest[]> {
    return this.log.awaitMethod(method, count);
  }

  /** Writes one correlated success response. */
  respond(id: RequestId, result: MethodResult): void {
    this.writeRecord({ jsonrpc: "2.0", id, result });
  }

  /** Writes one correlated typed protocol error. */
  respondError(id: RequestId, error: RpcError): void {
    this.writeRecord({ jsonrpc: "2.0", id, error });
  }

  /** Writes one routed observation. */
  emit(
    target: AttachmentTarget,
    cursor: RuntimeClientCursor,
    event: RuntimeClientEvent,
  ): void {
    this.writeRecord(notification("session/event", { target, cursor, event }));
  }

  /** Writes one already-shaped protocol record. */
  writeRecord(record: unknown): void {
    this.serverOutput.write(encodeRecord(record));
  }

  /** Writes exact bytes, for framing-level tests. */
  writeRaw(bytes: string | Buffer): void {
    this.serverOutput.write(
      typeof bytes === "string" ? Buffer.from(bytes, "utf8") : bytes,
    );
  }

  /** Ends the server output stream: transport EOF. */
  endOutput(): void {
    this.serverOutput.end();
  }
}

/**
 * Waits until a condition holds, bounded by an outer liveness deadline.
 *
 * The condition is re-checked as soon as the event loop can run it, so this
 * returns the moment the awaited fact is observable — it is never a fixed
 * sleep. The deadline exists only so a genuine hang fails the test instead of
 * stalling it.
 */
export async function until(
  condition: () => boolean,
  what = "condition",
  timeoutMs = 30_000,
): Promise<void> {
  const deadline = Date.now() + timeoutMs;
  while (!condition()) {
    if (Date.now() >= deadline) {
      throw new Error(`${what} never became true within ${timeoutMs}ms`);
    }
    await new Promise<void>((resolve) => {
      setTimeout(resolve, 1);
    });
  }
}

/** Yields once, so a microtask-scheduled continuation can run. */
export function tick(): Promise<void> {
  return new Promise<void>((resolve) => setImmediate(resolve));
}
