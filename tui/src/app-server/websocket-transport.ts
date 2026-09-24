/**
 * App Server protocol messages over a WebSocket to an externally managed server.
 *
 * ```text
 * ws(s)://host:port/  +  subprotocols
 *     rustx.app-server.v20
 *     rustx-token.<dedicated transport token>
 * ```
 *
 * The admission contract is the server's (`src/app_server/transport/
 * websocket.rs`): path `/` with no query, both subprotocol offers present, and
 * the server selecting only `rustx.app-server.v20` in its response. Failed
 * admission is HTTP 401 and the credential is never echoed. This client
 * therefore invents no authentication of its own — it presents the dedicated
 * transport secret the trusted host gave it, exactly the way a browser can.
 *
 * That secret is a **transport** credential. It is not a provider key, not an
 * MCP secret, not a runtime credential, and it never enters Session state or
 * provider configuration: it is supplied once at connect time and is used for
 * nothing else.
 *
 * One complete text message is one protocol message. The library reassembles
 * fragments and validates UTF-8. This adapter checks the App Server payload
 * bound before JSON parsing.
 *
 * Disconnect is a transport fact. The server keeps running, accepted work keeps
 * executing, pending interactions stay pending, and this client simply stops
 * observing until it reconnects and re-reads authoritative state.
 */

import { JSONL_MAX_RECORD_BYTES } from "../protocol/jsonl.ts";
import {
  BaseTransport,
  TransportClosedError,
  describeCause,
} from "./transport.ts";

/** The only subprotocol the App Server selects. */
export const APP_SERVER_SUBPROTOCOL = "rustx.app-server.v20";

/** The credential offer prefix the server's handshake callback matches. */
export const TOKEN_SUBPROTOCOL_PREFIX = "rustx-token.";

export interface WebSocketTransportOptions {
  /** A `ws://` or `wss://` endpoint. The server requires path `/`. */
  endpoint: string;
  /** The dedicated transport token, supplied by the trusted host. */
  token: string;
  /**
   * How long the handshake may take before the attempt is abandoned. The
   * server drops an incomplete socket after its own bound; this is the client
   * side of the same finiteness.
   */
  handshakeTimeoutMs?: number;
}

/** The server's own handshake bound, mirrored so the client fails first. */
export const DEFAULT_HANDSHAKE_TIMEOUT_MS = 5_000;

export class WebSocketTransport extends BaseTransport {
  readonly #socket: WebSocket;
  readonly #endpoint: string;
  readonly #ended: Promise<void>;

  private constructor(socket: WebSocket, endpoint: string) {
    super();
    this.#socket = socket;
    this.#endpoint = endpoint;
    this.#ended = new Promise((resolve) => socket.addEventListener("close", () => resolve(), { once: true }));

    socket.addEventListener("message", (event: MessageEvent) => {
      this.#onMessage(event.data);
    });
    socket.addEventListener("close", () => {
      this.terminate(
        new TransportClosedError(
          "input_eof",
          "the App Server closed the WebSocket connection",
        ),
      );
    });
    socket.addEventListener("error", () => {
      // The DOM error event carries no cause; `close` follows with the real
      // lifecycle transition, and whichever arrives first settles once.
      this.terminate(
        new TransportClosedError(
          "socket_error",
          "the App Server WebSocket connection failed",
        ),
      );
    });
  }

  /**
   * Connects and completes the admission handshake.
   *
   * Resolves only once the server has selected the App Server subprotocol:
   * a socket that opened without it is not an App Server connection, and
   * proceeding would mean speaking a protocol the peer never agreed to.
   */
  static connect(options: WebSocketTransportOptions): Promise<WebSocketTransport> {
    const endpoint = options.endpoint;
    const timeoutMs = options.handshakeTimeoutMs ?? DEFAULT_HANDSHAKE_TIMEOUT_MS;
    let socket: WebSocket;
    try {
      socket = new WebSocket(endpoint, [
        APP_SERVER_SUBPROTOCOL,
        `${TOKEN_SUBPROTOCOL_PREFIX}${options.token}`,
      ]);
    } catch (cause) {
      return Promise.reject(
        new TransportClosedError(
          "handshake_failed",
          `the App Server endpoint ${endpoint} could not be opened: ${describeCause(cause)}`,
          cause,
        ),
      );
    }

    return new Promise<WebSocketTransport>((resolve, reject) => {
      let settled = false;
      const timer = setTimeout(() => {
        finish(() => {
          socket.close();
          reject(
            new TransportClosedError(
              "handshake_failed",
              `the App Server handshake with ${endpoint} did not complete within ${timeoutMs}ms`,
            ),
          );
        });
      }, timeoutMs);
      timer.unref?.();

      const finish = (act: () => void): void => {
        if (settled) return;
        settled = true;
        clearTimeout(timer);
        socket.removeEventListener("open", onOpen);
        socket.removeEventListener("error", onError);
        socket.removeEventListener("close", onClose);
        act();
      };

      const onOpen = (): void => {
        finish(() => {
          if (socket.protocol !== APP_SERVER_SUBPROTOCOL) {
            socket.close();
            reject(
              new TransportClosedError(
                "handshake_failed",
                `the peer at ${endpoint} did not select the ${APP_SERVER_SUBPROTOCOL} subprotocol`,
              ),
            );
            return;
          }
          resolve(new WebSocketTransport(socket, endpoint));
        });
      };
      const onError = (): void => {
        finish(() =>
          reject(
            new TransportClosedError(
              "handshake_failed",
              // A rejected credential is an HTTP 401 the browser-shaped API
              // does not expose, so the diagnostic names both possibilities
              // rather than guessing one.
              `the App Server at ${endpoint} refused the connection; check that it is running and that the transport token is current`,
            ),
          ),
        );
      };
      const onClose = (): void => {
        finish(() =>
          reject(
            new TransportClosedError(
              "handshake_failed",
              `the App Server at ${endpoint} closed the connection during admission`,
            ),
          ),
        );
      };

      socket.addEventListener("open", onOpen);
      socket.addEventListener("error", onError);
      socket.addEventListener("close", onClose);
    });
  }

  override async close(): Promise<void> {
    super.close();
    await this.#ended;
  }

  override describe(): string {
    return this.#endpoint;
  }

  protected override writeMessage(message: unknown): Promise<void> {
    this.#socket.send(JSON.stringify(message));
    return Promise.resolve();
  }

  protected override disposeTransport(): void {
    if (
      this.#socket.readyState === WebSocket.OPEN ||
      this.#socket.readyState === WebSocket.CONNECTING
    ) {
      this.#socket.close();
    }
  }

  #onMessage(data: unknown): void {
    if (typeof data !== "string") {
      // The server never sends binary, and it rejects binary from clients.
      this.terminate(
        new TransportClosedError(
          "protocol_error",
          "the App Server sent a non-text WebSocket message",
        ),
      );
      return;
    }
    if (Buffer.byteLength(data, "utf8") > JSONL_MAX_RECORD_BYTES) {
      this.terminate(new TransportClosedError("framing_error", "the App Server message exceeds the payload limit"));
      return;
    }
    let record: unknown;
    try {
      record = JSON.parse(data) as unknown;
    } catch (cause) {
      this.terminate(
        new TransportClosedError(
          "protocol_error",
          `the App Server sent a message that is not valid JSON: ${describeCause(cause)}`,
          cause,
        ),
      );
      return;
    }
    this.deliver(record);
  }
}
