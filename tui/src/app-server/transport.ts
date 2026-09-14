/**
 * The transport boundary of the App Server client.
 *
 * ```text
 *            AppServerClient          one protocol, one semantic client
 *                  |
 *          AppServerTransport         complete messages in, complete messages out
 *            /            \
 *   StdioTransport      WebSocketTransport
 *   JSONL over a         text frames to an
 *   TUI-owned child      externally managed server
 * ```
 *
 * A transport delivers **complete, untrusted decoded JSON values** and the
 * lifetime of the physical connection. That is all it is allowed to know. It
 * does not know what a Session is, what a Turn is, which requests are
 * side-effecting, how a response correlates to a request, or what any of it
 * means. Everything below this line is bytes and sockets; everything above it
 * is one protocol.
 *
 * # Termination
 *
 * Termination is a transport fact and never a semantic one. A closed transport
 * says the client can no longer observe or address the server. It says nothing
 * about whether a turn is running, whether an interaction is still pending, or
 * whether a mutation the client sent was accepted — and this client never lets
 * it pretend otherwise.
 */

/** Why a transport reached its terminal state. */
export type TransportCloseReason =
  | "input_eof"
  | "framing_error"
  | "protocol_error"
  | "process_exit"
  | "write_error"
  | "socket_error"
  | "handshake_failed"
  | "client_closed";

/**
 * A terminal transport failure.
 *
 * Deliberately not a protocol error: a protocol error is a well-formed answer
 * from a healthy server, and the connection stays usable after one.
 */
export class TransportClosedError extends Error {
  readonly reason: TransportCloseReason;

  constructor(reason: TransportCloseReason, message: string, cause?: unknown) {
    super(message, cause === undefined ? undefined : { cause });
    this.name = "TransportClosedError";
    this.reason = reason;
  }
}

/** Parsed JSON only: never a trusted App Server DTO until protocol decoding. */
export type TransportRecord = unknown;

export interface AppServerTransport {
  /** The terminal failure, once the transport has one. */
  readonly closed: TransportClosedError | undefined;

  /** A bounded description for diagnostics. Never a semantic fact. */
  describe(): string;

  /**
   * Writes one complete protocol message.
   *
   * Writes are serialized: messages reach the server in issue order even when
   * the peer applies backpressure mid-message.
   */
  send(message: unknown): Promise<void>;

  /** Subscribes to complete inbound messages. Returns an unsubscribe function. */
  onMessage(listener: (record: TransportRecord) => void): () => void;

  /** Subscribes to terminal failure. Fires immediately if already terminal. */
  onClose(listener: (error: TransportClosedError) => void): () => void;

  /**
   * Ends this transport from the client side.
   *
   * This is a local action on a socket or a pipe. It detaches; it does not
   * cancel, settle, unload, or shut anything down. Ending an owned child
   * process is a different operation with a different owner — see
   * `./host.ts`.
   */
  close(): void | Promise<void>;
}

/**
 * The parts of a transport that are identical for every transport.
 *
 * Listener bookkeeping, exactly-once terminal settlement and write ordering are
 * mechanics, not policy: duplicating them per transport would be two chances to
 * get "settle every waiter exactly once" wrong. What stays transport-specific is
 * precisely the byte mechanics — framing, sockets, pipes — which subclasses
 * supply through {@link writeMessage}.
 */
export abstract class BaseTransport implements AppServerTransport {
  readonly #messageListeners = new Set<(record: TransportRecord) => void>();
  readonly #closeListeners = new Set<(error: TransportClosedError) => void>();
  /** Serializes writes so messages reach the peer in issue order. */
  #writeChain: Promise<void> = Promise.resolve();
  #closed: TransportClosedError | undefined;

  get closed(): TransportClosedError | undefined {
    return this.#closed;
  }

  abstract describe(): string;

  /** Writes one already-encoded message. Transport-specific mechanics only. */
  protected abstract writeMessage(message: unknown): Promise<void>;

  /** Releases transport resources. Called exactly once, on termination. */
  protected abstract disposeTransport(): void;

  send(message: unknown): Promise<void> {
    if (this.#closed !== undefined) {
      return Promise.reject(this.#closed);
    }
    const next = this.#writeChain.then(async () => {
      if (this.#closed !== undefined) {
        throw this.#closed;
      }
      await this.writeMessage(message);
    });
    // The chain must survive a failed write, or every later write would be
    // rejected with a stale cause instead of the terminal one.
    this.#writeChain = next.catch((cause: unknown) => {
      if (cause instanceof TransportClosedError) {
        this.terminate(cause);
        return;
      }
      this.terminate(
        new TransportClosedError(
          "write_error",
          `writing a protocol message failed: ${describeCause(cause)}`,
          cause,
        ),
      );
    });
    return next;
  }

  onMessage(listener: (record: TransportRecord) => void): () => void {
    this.#messageListeners.add(listener);
    return () => this.#messageListeners.delete(listener);
  }

  onClose(listener: (error: TransportClosedError) => void): () => void {
    this.#closeListeners.add(listener);
    if (this.#closed !== undefined) {
      listener(this.#closed);
    }
    return () => this.#closeListeners.delete(listener);
  }

  close(): void {
    this.terminate(
      new TransportClosedError(
        "client_closed",
        "the client closed the App Server transport",
      ),
    );
  }

  /** Publishes one complete inbound message. */
  protected deliver(record: TransportRecord): void {
    if (this.#closed !== undefined) {
      return;
    }
    for (const listener of [...this.#messageListeners]) {
      listener(record);
    }
  }

  /** Settles the transport terminally. Idempotent by construction. */
  protected terminate(error: TransportClosedError): void {
    if (this.#closed !== undefined) {
      return;
    }
    this.#closed = error;
    this.disposeTransport();
    for (const listener of [...this.#closeListeners]) {
      listener(error);
    }
  }
}

export function describeCause(cause: unknown): string {
  return cause instanceof Error ? cause.message : String(cause);
}
