/**
 * App Server protocol messages over a pipe pair, framed as JSONL.
 *
 * ```text
 * child stdout --> bounded JSONL decode --> one complete message
 * one message  --> encode + LF          --> child stdin
 * child stderr --> bounded tail (never decoded)
 * ```
 *
 * This is the ordinary local path: the TUI owns the child, so the simplest
 * process-local transport is the right one. No loopback socket is opened to
 * claim protocol unification — the protocol is already unified above this
 * layer, and a socket would only add an address, a port and an admission
 * secret to a pipe that already exists.
 *
 * Pipe loss is a transport fact. EOF and a broken pipe terminate this transport
 * and nothing else: they do not cancel a turn, settle an interaction, unload a
 * runtime, or license the client to invent a terminal runtime state.
 */

import type { Readable, Writable } from "node:stream";

import { JsonlDecoder, JsonlFramingError, encodeRecord } from "../protocol/jsonl.ts";
import {
  BaseTransport,
  TransportClosedError,
  describeCause,
} from "./transport.ts";

export interface StdioTransportOptions {
  /** The protocol output stream of the peer. */
  input: Readable;
  /** The protocol input stream of the peer. */
  output: Writable;
  maxRecordBytes?: number;
  /** A bounded label for diagnostics. */
  label?: string;
}

export class StdioTransport extends BaseTransport {
  readonly #input: Readable;
  readonly #output: Writable;
  readonly #decoder: JsonlDecoder;
  readonly #maxRecordBytes: number | undefined;
  readonly #label: string;

  constructor(options: StdioTransportOptions) {
    super();
    this.#input = options.input;
    this.#output = options.output;
    this.#maxRecordBytes = options.maxRecordBytes;
    this.#decoder = new JsonlDecoder(options.maxRecordBytes);
    this.#label = options.label ?? "stdio";

    this.#input.on("data", (chunk: Buffer) => this.#onChunk(chunk));
    this.#input.on("end", () => this.#onInputEnd());
    this.#input.on("error", (cause) =>
      this.terminate(
        new TransportClosedError(
          "framing_error",
          `reading the App Server transport failed: ${describeCause(cause)}`,
          cause,
        ),
      ),
    );
    this.#output.on("error", (cause) =>
      this.terminate(
        new TransportClosedError(
          "write_error",
          `writing the App Server transport failed: ${describeCause(cause)}`,
          cause,
        ),
      ),
    );
  }

  override describe(): string {
    return this.#label;
  }

  /**
   * Reports that the peer process exited.
   *
   * The process owner calls this so pending requests settle with the real
   * cause instead of a bare EOF. It remains a transport/process fact: it never
   * becomes a runtime outcome.
   */
  reportProcessExit(
    code: number | null,
    signal: string | null,
    spawnError?: string,
  ): void {
    this.terminate(
      new TransportClosedError(
        "process_exit",
        spawnError === undefined
          ? `the App Server process exited (code ${code ?? "none"}, signal ${signal ?? "none"})`
          : `the App Server process could not be started: ${spawnError}`,
      ),
    );
  }

  protected override writeMessage(message: unknown): Promise<void> {
    let record: Buffer;
    try {
      record = encodeRecord(message, this.#maxRecordBytes);
    } catch (cause) {
      return Promise.reject(
        new TransportClosedError(
          cause instanceof JsonlFramingError ? "framing_error" : "write_error",
          `encoding a protocol message failed: ${describeCause(cause)}`,
          cause,
        ),
      );
    }
    return new Promise<void>((resolve, reject) => {
      this.#output.write(record, (cause) =>
        cause ? reject(cause) : resolve(),
      );
    });
  }

  protected override disposeTransport(): void {
    this.#input.removeAllListeners("data");
  }

  #onChunk(chunk: Buffer): void {
    if (this.closed !== undefined) {
      return;
    }
    let records: unknown[];
    try {
      records = this.#decoder.push(chunk);
    } catch (cause) {
      this.terminate(
        new TransportClosedError(
          "framing_error",
          `the App Server transport violated its framing contract: ${describeCause(cause)}`,
          cause,
        ),
      );
      return;
    }
    for (const record of records) {
      if (typeof record !== "object" || record === null) {
        this.terminate(
          new TransportClosedError(
            "protocol_error",
            "a protocol record is not a JSON object",
          ),
        );
        return;
      }
      this.deliver(record);
      if (this.closed !== undefined) {
        return;
      }
    }
  }

  #onInputEnd(): void {
    try {
      this.#decoder.finish();
    } catch (cause) {
      this.terminate(
        new TransportClosedError(
          "framing_error",
          `the App Server transport ended mid-record: ${describeCause(cause)}`,
          cause,
        ),
      );
      return;
    }
    this.terminate(
      new TransportClosedError(
        "input_eof",
        "the App Server closed its transport output stream",
      ),
    );
  }
}
