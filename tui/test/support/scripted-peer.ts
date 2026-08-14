/**
 * A scripted Runtime Client protocol peer for deterministic transport tests.
 *
 * The peer is a pair of in-memory streams plus a record log. A test drives it
 * by writing exact bytes or exact protocol records, so ordering is decided by
 * the script rather than by timing. Nothing here sleeps, and no test proves an
 * ordering with a timer.
 *
 * It is a *test double for the runtime's transport*, never a runtime: it holds
 * no conversation state and answers only what a test tells it to answer.
 */

import { PassThrough } from "node:stream";

import { encodeRecord } from "../../src/protocol/jsonl.ts";
import type {
  RuntimeClientError,
  RuntimeClientEvent,
  RuntimeClientRequest,
  RuntimeClientResult,
} from "../../src/protocol/types.ts";

export class ScriptedPeer {
  /** Bytes rustX would write; the client reads this. */
  readonly runtimeOutput = new PassThrough();
  /** Bytes the client writes; the peer reads this. */
  readonly clientOutput = new PassThrough();

  readonly #requests: RuntimeClientRequest[] = [];
  readonly #waiters: Array<() => void> = [];
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
        this.#requests.push(JSON.parse(record) as RuntimeClientRequest);
      }
      const waiters = this.#waiters.splice(0);
      for (const waiter of waiters) {
        waiter();
      }
    });
  }

  /** Every request the client has written so far, in write order. */
  get requests(): readonly RuntimeClientRequest[] {
    return this.#requests;
  }

  /**
   * Resolves once at least `count` requests have been written.
   *
   * This is a data barrier, not a delay: the promise settles on the write
   * itself, so a test never guesses how long a write takes.
   */
  async awaitRequests(count: number): Promise<readonly RuntimeClientRequest[]> {
    while (this.#requests.length < count) {
      await new Promise<void>((resolve) => this.#waiters.push(resolve));
    }
    return this.#requests;
  }

  /** Writes one correlated success response. */
  respond(id: number, result: RuntimeClientResult): void {
    this.writeRecord({ id, result });
  }

  /** Writes one correlated typed protocol error. */
  respondError(id: number, error: RuntimeClientError): void {
    this.writeRecord({ id, error });
  }

  /** Writes one protocol event notification at a cursor. */
  emit(cursor: number, event: RuntimeClientEvent): void {
    this.writeRecord({ cursor, event });
  }

  /** Writes one already-shaped protocol record. */
  writeRecord(record: unknown): void {
    this.runtimeOutput.write(encodeRecord(record));
  }

  /** Writes exact bytes, for framing-level tests. */
  writeRaw(bytes: string | Buffer): void {
    this.runtimeOutput.write(
      typeof bytes === "string" ? Buffer.from(bytes, "utf8") : bytes,
    );
  }

  /** Ends the runtime output stream: transport EOF. */
  endOutput(): void {
    this.runtimeOutput.end();
  }
}

/**
 * Yields to the event loop until a condition holds.
 *
 * Used only to let already-queued stream callbacks run — never to wait out a
 * duration. The loop is bounded so a genuine hang fails the test instead of
 * stalling it.
 */
export async function until(
  condition: () => boolean,
  what = "condition",
): Promise<void> {
  for (let turn = 0; turn < 10_000; turn += 1) {
    if (condition()) {
      return;
    }
    await new Promise<void>((resolve) => setImmediate(resolve));
  }
  throw new Error(`${what} never became true`);
}
