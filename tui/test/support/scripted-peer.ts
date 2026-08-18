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
 * Waits until a condition holds, bounded by an outer liveness deadline.
 *
 * The condition is re-checked as soon as the event loop can run it, so this
 * returns the moment the awaited fact is observable — it is never a fixed
 * sleep. The deadline exists only so a genuine hang fails the test instead of
 * stalling it.
 *
 * The previous implementation spun a fixed 10 000 `setImmediate` turns. That
 * was documented as "never waiting out a duration", but because a queued
 * immediate keeps the poll phase from blocking, the whole budget is worth
 * roughly 10 ms of wall clock. Conditions that depend on real external I/O
 * (the spawned runtime binary answering, a provider round trip) therefore had
 * a ~10 ms budget and flaked on a contended CI runner. Yielding with a timer
 * lets the poll phase actually wait for that I/O.
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
