/**
 * The single owner of the Runtime Client transport.
 *
 * Exactly one object owns JSONL framing, request-id allocation, the pending
 * RPC map, response correlation, event delivery, protocol decoding, ordered
 * writes, EOF, and terminal connection failure. No UI component writes a
 * JSONL record, no slash command allocates a request id, and no second module
 * keeps its own correlation map.
 *
 * ```text
 * request(body) -> id allocated here -> encode -> ordered write
 *                                                     |
 * stdout bytes -> bounded JSONL decode -> record ------+
 *                                          |
 *                            has an id? --yes--> settle that pending request
 *                                          |
 *                                         no -> deliver as an event
 * ```
 *
 * # Settlement
 *
 * Every pending request settles exactly once. EOF, a malformed protocol
 * record, a framing error, an unexpected process exit, and any terminal
 * failure explicitly reject every request still pending, and every request
 * issued afterwards fails immediately. A request never hangs because the peer
 * went away.
 *
 * Transport loss is never cancellation. A closed connection says nothing
 * about attempts, tool executions, or background work; it only means this
 * client can no longer observe them.
 */

import type { Readable, Writable } from "node:stream";

import { JsonlDecoder, JsonlFramingError, encodeRecord } from "../protocol/jsonl.ts";
import {
  describeProtocolError,
  isEventLikeRecord,
  isProtocolEvent,
  type RequestId,
  type RuntimeClientError,
  type RuntimeClientProtocolEvent,
  type RuntimeClientRequest,
  type RuntimeClientRequestBody,
  type RuntimeClientResponse,
  type RuntimeClientResult,
} from "../protocol/types.ts";

/** Why a connection reached its terminal state. */
export type ConnectionClosedReason =
  | "input_eof"
  | "framing_error"
  | "protocol_error"
  | "process_exit"
  | "write_error"
  | "client_closed";

/** A terminal transport failure. Never a semantic protocol error. */
export class ConnectionClosedError extends Error {
  readonly reason: ConnectionClosedReason;

  constructor(reason: ConnectionClosedReason, message: string, cause?: unknown) {
    super(message, cause === undefined ? undefined : { cause });
    this.name = "ConnectionClosedError";
    this.reason = reason;
  }
}

/**
 * A typed semantic failure returned by the runtime.
 *
 * Deliberately distinct from {@link ConnectionClosedError}: a protocol error
 * is a well-formed answer from a healthy runtime, and the connection stays
 * usable afterwards.
 */
export class RuntimeRequestError extends Error {
  readonly error: RuntimeClientError;

  constructor(error: RuntimeClientError) {
    super(describeProtocolError(error));
    this.name = "RuntimeRequestError";
    this.error = error;
  }
}

export interface RuntimeClientConnectionOptions {
  input: Readable;
  output: Writable;
  maxRecordBytes?: number;
}

interface PendingRequest {
  resolve: (result: RuntimeClientResult) => void;
  reject: (error: Error) => void;
  method: string;
}

type EventListener = (event: RuntimeClientProtocolEvent) => void;
type CloseListener = (error: ConnectionClosedError) => void;

export class RuntimeClientConnection {
  readonly #input: Readable;
  readonly #output: Writable;
  readonly #decoder: JsonlDecoder;
  readonly #maxRecordBytes: number | undefined;

  #nextRequestId: RequestId = 1;
  readonly #pending = new Map<RequestId, PendingRequest>();
  readonly #eventListeners = new Set<EventListener>();
  readonly #closeListeners = new Set<CloseListener>();

  /** Serializes writes so records reach the peer in issue order. */
  #writeChain: Promise<void> = Promise.resolve();
  #closed: ConnectionClosedError | undefined;

  constructor(options: RuntimeClientConnectionOptions) {
    this.#input = options.input;
    this.#output = options.output;
    this.#maxRecordBytes = options.maxRecordBytes;
    this.#decoder = new JsonlDecoder(options.maxRecordBytes);

    this.#input.on("data", (chunk: Buffer) => this.#onChunk(chunk));
    this.#input.on("end", () => this.#onInputEnd());
    this.#input.on("error", (cause) =>
      this.#close(
        new ConnectionClosedError(
          "framing_error",
          `reading the runtime transport failed: ${(cause as Error).message}`,
          cause,
        ),
      ),
    );
    this.#output.on("error", (cause) =>
      this.#close(
        new ConnectionClosedError(
          "write_error",
          `writing the runtime transport failed: ${(cause as Error).message}`,
          cause,
        ),
      ),
    );
  }

  /** The terminal failure, once the connection has one. */
  get closed(): ConnectionClosedError | undefined {
    return this.#closed;
  }

  /** How many requests are awaiting a correlated response. */
  get pendingCount(): number {
    return this.#pending.size;
  }

  /** Subscribes to protocol events. Returns an unsubscribe function. */
  onEvent(listener: EventListener): () => void {
    this.#eventListeners.add(listener);
    return () => this.#eventListeners.delete(listener);
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
   * Issues one request and resolves with its correlated result.
   *
   * The id is allocated here and nowhere else. A typed protocol error rejects
   * with {@link RuntimeRequestError}; a transport failure rejects with
   * {@link ConnectionClosedError}.
   */
  request(body: RuntimeClientRequestBody): Promise<RuntimeClientResult> {
    if (this.#closed !== undefined) {
      // After termination a new request fails immediately rather than
      // waiting for a peer that will never answer.
      return Promise.reject(this.#closed);
    }

    const id = this.#nextRequestId;
    this.#nextRequestId += 1;
    const request = { ...body, id } as RuntimeClientRequest;

    return new Promise<RuntimeClientResult>((resolve, reject) => {
      this.#pending.set(id, { resolve, reject, method: body.method });
      this.#enqueueWrite(request).catch((cause: unknown) => {
        // The write path already closed the connection and rejected every
        // pending request, including this one.
        void cause;
      });
    });
  }

  /**
   * Closes the connection from this side.
   *
   * This is a client-side transport action. It ends observation; it does not
   * cancel, settle, detach, or shut anything down.
   */
  close(): void {
    this.#close(
      new ConnectionClosedError(
        "client_closed",
        "the client closed the runtime transport",
      ),
    );
  }

  /**
   * Reports that the runtime process exited.
   *
   * The process owner calls this so pending requests fail with the real
   * cause instead of a bare EOF.
   */
  reportProcessExit(
    code: number | null,
    signal: string | null,
    spawnError?: string,
  ): void {
    this.#close(
      new ConnectionClosedError(
        "process_exit",
        spawnError === undefined
          ? `the runtime process exited (code ${code ?? "none"}, signal ${signal ?? "none"})`
          : `the runtime process could not be started: ${spawnError}`,
      ),
    );
  }

  #enqueueWrite(request: RuntimeClientRequest): Promise<void> {
    // Chaining keeps record order equal to issue order even when the peer
    // applies backpressure mid-record.
    const next = this.#writeChain.then(async () => {
      if (this.#closed !== undefined) {
        throw this.#closed;
      }
      const record = encodeRecord(request, this.#maxRecordBytes);
      await new Promise<void>((resolve, reject) => {
        this.#output.write(record, (cause) =>
          cause ? reject(cause) : resolve(),
        );
      });
    });

    this.#writeChain = next.catch((cause: unknown) => {
      if (cause instanceof ConnectionClosedError) {
        this.#close(cause);
        return;
      }
      this.#close(
        new ConnectionClosedError(
          cause instanceof JsonlFramingError ? "framing_error" : "write_error",
          `writing a protocol record failed: ${(cause as Error).message}`,
          cause,
        ),
      );
    });

    return next;
  }

  #onChunk(chunk: Buffer): void {
    if (this.#closed !== undefined) {
      return;
    }
    let records: unknown[];
    try {
      records = this.#decoder.push(chunk);
    } catch (cause) {
      this.#close(
        new ConnectionClosedError(
          "framing_error",
          `the runtime transport violated its framing contract: ${(cause as Error).message}`,
          cause,
        ),
      );
      return;
    }
    for (const record of records) {
      if (!this.#dispatch(record)) {
        return;
      }
    }
  }

  #onInputEnd(): void {
    try {
      this.#decoder.finish();
    } catch (cause) {
      this.#close(
        new ConnectionClosedError(
          "framing_error",
          `the runtime transport ended mid-record: ${(cause as Error).message}`,
          cause,
        ),
      );
      return;
    }
    this.#close(
      new ConnectionClosedError(
        "input_eof",
        "the runtime closed its transport output stream",
      ),
    );
  }

  /** Routes one decoded record. Returns false once the connection ended. */
  #dispatch(record: unknown): boolean {
    if (typeof record !== "object" || record === null) {
      this.#close(
        new ConnectionClosedError(
          "protocol_error",
          "a protocol record is not a JSON object",
        ),
      );
      return false;
    }

    if (isProtocolEvent(record)) {
      for (const listener of this.#eventListeners) {
        listener(record);
      }
      return true;
    }

    if (isEventLikeRecord(record)) {
      this.#close(
        new ConnectionClosedError(
          "protocol_error",
          "the runtime sent an unknown or malformed Runtime Client Protocol v5 event",
        ),
      );
      return false;
    }

    const response = record as RuntimeClientResponse;
    if (typeof response.id !== "number") {
      this.#close(
        new ConnectionClosedError(
          "protocol_error",
          "a protocol record is neither a correlated response nor an event",
        ),
      );
      return false;
    }

    const pending = this.#pending.get(response.id);
    if (pending === undefined) {
      // An unknown or duplicate response id means the peer is not speaking
      // the correlation contract. Guessing which request it answers would be
      // worse than failing.
      this.#close(
        new ConnectionClosedError(
          "protocol_error",
          `the runtime answered unknown request id ${response.id}`,
        ),
      );
      return false;
    }
    this.#pending.delete(response.id);

    if (response.error !== undefined) {
      pending.reject(new RuntimeRequestError(response.error));
      return true;
    }
    if (response.result === undefined) {
      this.#close(
        new ConnectionClosedError(
          "protocol_error",
          `the response to request ${response.id} carries neither result nor error`,
        ),
      );
      return false;
    }
    pending.resolve(response.result);
    return true;
  }

  #close(error: ConnectionClosedError): void {
    if (this.#closed !== undefined) {
      return;
    }
    this.#closed = error;

    // Every pending request settles exactly once, explicitly.
    const pending = [...this.#pending.values()];
    this.#pending.clear();
    for (const request of pending) {
      request.reject(error);
    }

    for (const listener of this.#closeListeners) {
      listener(error);
    }
  }
}
