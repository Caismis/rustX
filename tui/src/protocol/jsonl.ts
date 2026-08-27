/**
 * The strict JSONL framing of the Runtime Client stdio transport.
 *
 * This mirrors the Rust transport contract (`src/runtime_client/transport/
 * stdio.rs`) byte for byte:
 *
 * ```text
 * one JSON object per LF-delimited record
 * ```
 *
 * - `\n` (LF) is the sole record delimiter. One physical LF terminates one
 *   record, so an escaped `\n` inside a JSON string stays inside its record
 *   and multiline pretty-printed JSON is not a valid record.
 * - CRLF input is accepted by removing exactly one `\r` immediately before the
 *   terminating LF. No other whitespace is touched.
 * - {@link JSONL_MAX_RECORD_BYTES} bounds one record's JSON payload in both
 *   directions, excluding the terminating LF and including a trailing CR when
 *   CRLF was used.
 * - Memory is bounded: a record that reaches the limit before its LF fails
 *   immediately. The decoder never keeps buffering past the bound and never
 *   discards through a later LF.
 *
 * Bounds are counted in **encoded bytes**, never in JavaScript string length —
 * a `String.length` bound would let a multi-byte record cross the real limit.
 * The decoder therefore works on `Buffer` and never on a convenience line API
 * that could accumulate an unterminated line of arbitrary length.
 *
 * Any complete in-bound-size record that does not parse as JSON is a
 * transport failure, exactly as it is in Rust. This module decides framing
 * only; it never interprets protocol semantics.
 */

/** The v5 record limit, identical to `STDIO_JSONL_MAX_RECORD_BYTES`. */
export const JSONL_MAX_RECORD_BYTES = 8 * 1024 * 1024;

const LF = 0x0a;
const CR = 0x0d;

/** Why a JSONL stream violated its framing contract. */
export type JsonlFramingErrorKind =
  | "record_too_large"
  | "truncated_record"
  | "malformed_record"
  | "invalid_utf8";

/**
 * A framing failure of one JSONL record.
 *
 * Every kind is transport-fatal and semantically inert: the offending record
 * never reaches a semantic consumer.
 */
export class JsonlFramingError extends Error {
  readonly kind: JsonlFramingErrorKind;
  readonly limit?: number;
  readonly bytes?: number;

  constructor(
    kind: JsonlFramingErrorKind,
    message: string,
    detail?: { limit?: number; bytes?: number },
  ) {
    super(message);
    this.name = "JsonlFramingError";
    this.kind = kind;
    this.limit = detail?.limit;
    this.bytes = detail?.bytes;
  }
}

/**
 * Encodes one value as a JSONL record, bound-checked in encoded bytes.
 *
 * An oversized record is never truncated and never split across records; the
 * caller is expected to treat the failure as terminal.
 *
 * @throws {JsonlFramingError} when the encoded payload exceeds the limit.
 */
export function encodeRecord(
  value: unknown,
  maxRecordBytes: number = JSONL_MAX_RECORD_BYTES,
): Buffer {
  const payload = Buffer.from(JSON.stringify(value), "utf8");
  if (payload.byteLength > maxRecordBytes) {
    throw new JsonlFramingError(
      "record_too_large",
      `an outbound record of ${payload.byteLength} bytes exceeds the ${maxRecordBytes}-byte record limit`,
      { limit: maxRecordBytes, bytes: payload.byteLength },
    );
  }
  return Buffer.concat([payload, Buffer.from([LF])]);
}

/**
 * A bounded, incremental JSONL record decoder.
 *
 * Feed it arbitrary chunks — records may be split across chunks, several
 * records may arrive in one chunk, and a chunk may split a multi-byte
 * character or a CRLF pair. The decoder holds at most one partial record, and
 * that record is itself bounded.
 */
export class JsonlDecoder {
  readonly #maxRecordBytes: number;
  #buffer: Buffer = Buffer.alloc(0);
  #failed = false;

  constructor(maxRecordBytes: number = JSONL_MAX_RECORD_BYTES) {
    this.#maxRecordBytes = maxRecordBytes;
  }

  /** The bytes currently buffered for the in-progress record. */
  get bufferedBytes(): number {
    return this.#buffer.byteLength;
  }

  /**
   * Consumes one chunk and returns every complete record it terminated.
   *
   * @throws {JsonlFramingError} on an oversized, malformed, or non-UTF-8
   * record. The decoder is terminal after any such failure.
   */
  push(chunk: Buffer): unknown[] {
    this.#assertUsable();
    // Concatenating only up to the bound would corrupt a record that is
    // legitimately near the limit, so append first and check the *unterminated*
    // prefix below; the appended chunk itself is a fixed-size read.
    this.#buffer =
      this.#buffer.byteLength === 0
        ? chunk
        : Buffer.concat([this.#buffer, chunk]);

    const records: unknown[] = [];
    let searchFrom = 0;
    for (;;) {
      const lf = this.#buffer.indexOf(LF, searchFrom);
      if (lf === -1) {
        break;
      }
      const payload = this.#buffer.subarray(0, lf);
      this.#buffer = this.#buffer.subarray(lf + 1);
      searchFrom = 0;
      records.push(this.#decodeRecord(payload));
    }

    // No LF in what remains: the partial record must still respect the bound.
    if (this.#buffer.byteLength > this.#maxRecordBytes) {
      this.#failed = true;
      throw new JsonlFramingError(
        "record_too_large",
        `an inbound record exceeded the ${this.#maxRecordBytes}-byte record limit`,
        { limit: this.#maxRecordBytes, bytes: this.#buffer.byteLength },
      );
    }
    return records;
  }

  /**
   * Asserts that the stream ended at a record boundary.
   *
   * @throws {JsonlFramingError} when a partial record was still buffered.
   */
  finish(): void {
    this.#assertUsable();
    if (this.#buffer.byteLength > 0) {
      const bytes = this.#buffer.byteLength;
      this.#failed = true;
      throw new JsonlFramingError(
        "truncated_record",
        `the input stream ended with a ${bytes}-byte record that has no terminating newline`,
        { bytes },
      );
    }
  }

  #assertUsable(): void {
    if (this.#failed) {
      throw new JsonlFramingError(
        "malformed_record",
        "the decoder already failed; the transport is terminal",
      );
    }
  }

  #decodeRecord(raw: Buffer): unknown {
    // Remove exactly one CR immediately before the terminating LF.
    const payload =
      raw.byteLength > 0 && raw[raw.byteLength - 1] === CR
        ? raw.subarray(0, raw.byteLength - 1)
        : raw;

    // The bound applies to the record as framed, CR included.
    if (raw.byteLength > this.#maxRecordBytes) {
      this.#failed = true;
      throw new JsonlFramingError(
        "record_too_large",
        `an inbound record exceeded the ${this.#maxRecordBytes}-byte record limit`,
        { limit: this.#maxRecordBytes, bytes: raw.byteLength },
      );
    }

    const text = payload.toString("utf8");
    // `toString` is lossy: it substitutes U+FFFD rather than failing, so an
    // invalid sequence is detected by re-encoding and comparing lengths.
    if (Buffer.byteLength(text, "utf8") !== payload.byteLength) {
      this.#failed = true;
      throw new JsonlFramingError(
        "invalid_utf8",
        "an inbound record is not valid UTF-8",
        { bytes: payload.byteLength },
      );
    }

    try {
      return JSON.parse(text) as unknown;
    } catch (cause) {
      this.#failed = true;
      throw new JsonlFramingError(
        "malformed_record",
        `an inbound record is not valid JSON: ${(cause as Error).message}`,
        { bytes: payload.byteLength },
      );
    }
  }
}
