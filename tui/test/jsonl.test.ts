/**
 * The JSONL framing contract, byte for byte.
 *
 * These cases mirror the Rust transport suite: chunk splits, several records
 * per chunk, CRLF, escaped newlines inside JSON strings, the exact bound, the
 * bound exceeded, malformed JSON, invalid UTF-8, and truncation at EOF.
 * Bounds are asserted in encoded bytes, never in JavaScript string length.
 */

import assert from "node:assert/strict";
import { describe, it } from "node:test";

import {
  JSONL_MAX_RECORD_BYTES,
  JsonlDecoder,
  JsonlFramingError,
  encodeRecord,
} from "../src/protocol/jsonl.ts";

function bytes(text: string): Buffer {
  return Buffer.from(text, "utf8");
}

describe("JSONL framing", () => {
  it("decodes one record terminated by LF", () => {
    const decoder = new JsonlDecoder();
    const records = decoder.push(bytes('{"a":1}\n'));
    assert.deepEqual(records, [{ a: 1 }]);
    decoder.finish();
  });

  it("decodes several records arriving in one chunk", () => {
    const decoder = new JsonlDecoder();
    const records = decoder.push(bytes('{"a":1}\n{"b":2}\n{"c":3}\n'));
    assert.deepEqual(records, [{ a: 1 }, { b: 2 }, { c: 3 }]);
  });

  it("reassembles a record split across arbitrary chunk boundaries", () => {
    const source = '{"message":"hello world"}\n';
    for (let split = 1; split < source.length; split += 1) {
      const decoder = new JsonlDecoder();
      const first = decoder.push(bytes(source.slice(0, split)));
      const second = decoder.push(bytes(source.slice(split)));
      assert.deepEqual(
        [...first, ...second],
        [{ message: "hello world" }],
        `split at ${split}`,
      );
      decoder.finish();
    }
  });

  it("reassembles a record split inside a multi-byte character", () => {
    // The snowman is three UTF-8 bytes; the split lands in the middle of it.
    const record = bytes('{"text":"☃"}\n');
    const boundary = record.indexOf(0xe2) + 1;
    const decoder = new JsonlDecoder();
    const first = decoder.push(record.subarray(0, boundary));
    const second = decoder.push(record.subarray(boundary));
    assert.deepEqual([...first, ...second], [{ text: "☃" }]);
  });

  it("accepts CRLF by removing exactly one CR before the LF", () => {
    const decoder = new JsonlDecoder();
    assert.deepEqual(decoder.push(bytes('{"a":1}\r\n')), [{ a: 1 }]);
    // A CR that is not immediately before the LF is ordinary JSON whitespace
    // and is left to the JSON parser, not to a second whitespace grammar.
    assert.deepEqual(decoder.push(bytes('{"a":\r 2}\r\n')), [{ a: 2 }]);
  });

  it("accepts a CRLF pair split across chunks", () => {
    const decoder = new JsonlDecoder();
    assert.deepEqual(decoder.push(bytes('{"a":1}\r')), []);
    assert.deepEqual(decoder.push(bytes("\n")), [{ a: 1 }]);
  });

  it("keeps an escaped newline inside the record as JSON content", () => {
    const decoder = new JsonlDecoder();
    const records = decoder.push(bytes('{"text":"first\\nsecond"}\n'));
    assert.deepEqual(records, [{ text: "first\nsecond" }]);
    assert.equal((records[0] as { text: string }).text, "first\nsecond");
  });

  it("accepts a record of exactly the limit and rejects one byte more", () => {
    const limit = 64;
    // `{"t":"<pad>"}` — the payload is exactly `limit` encoded bytes.
    const overhead = '{"t":""}'.length;
    const exact = `{"t":"${"x".repeat(limit - overhead)}"}`;
    assert.equal(Buffer.byteLength(exact, "utf8"), limit);

    const decoder = new JsonlDecoder(limit);
    assert.equal(decoder.push(bytes(`${exact}\n`)).length, 1);

    const oversized = `{"t":"${"x".repeat(limit - overhead + 1)}"}`;
    const strict = new JsonlDecoder(limit);
    assert.throws(
      () => strict.push(bytes(`${oversized}\n`)),
      (error: unknown) =>
        error instanceof JsonlFramingError && error.kind === "record_too_large",
    );
  });

  it("counts encoded bytes, not JavaScript string length", () => {
    // Twenty snowmen are 20 UTF-16 code units but 60 UTF-8 bytes.
    const record = `{"t":"${"☃".repeat(20)}"}`;
    assert.ok(record.length < Buffer.byteLength(record, "utf8"));
    const decoder = new JsonlDecoder(Buffer.byteLength(record, "utf8") - 1);
    assert.throws(
      () => decoder.push(bytes(`${record}\n`)),
      (error: unknown) =>
        error instanceof JsonlFramingError && error.kind === "record_too_large",
    );
  });

  it("fails an unterminated record as soon as it passes the bound", () => {
    const decoder = new JsonlDecoder(32);
    // No LF has arrived: the decoder must not keep buffering.
    assert.throws(
      () => decoder.push(bytes("x".repeat(33))),
      (error: unknown) =>
        error instanceof JsonlFramingError && error.kind === "record_too_large",
    );
  });

  it("rejects malformed JSON as a transport failure", () => {
    for (const malformed of ["not json", "{", "", "   ", '{"a":}']) {
      const decoder = new JsonlDecoder();
      assert.throws(
        () => decoder.push(bytes(`${malformed}\n`)),
        (error: unknown) =>
          error instanceof JsonlFramingError &&
          error.kind === "malformed_record",
        `expected ${JSON.stringify(malformed)} to be transport-fatal`,
      );
    }
  });

  it("rejects invalid UTF-8 rather than substituting replacement characters", () => {
    const decoder = new JsonlDecoder();
    // 0xff is never valid UTF-8.
    const record = Buffer.concat([
      bytes('{"t":"'),
      Buffer.from([0xff]),
      bytes('"}\n'),
    ]);
    assert.throws(
      () => decoder.push(record),
      (error: unknown) =>
        error instanceof JsonlFramingError && error.kind === "invalid_utf8",
    );
  });

  it("rejects a stream that ends mid-record", () => {
    const decoder = new JsonlDecoder();
    assert.deepEqual(decoder.push(bytes('{"a":1}')), []);
    assert.throws(
      () => decoder.finish(),
      (error: unknown) =>
        error instanceof JsonlFramingError && error.kind === "truncated_record",
    );
  });

  it("accepts a stream that ends exactly at a record boundary", () => {
    const decoder = new JsonlDecoder();
    decoder.push(bytes('{"a":1}\n'));
    assert.doesNotThrow(() => decoder.finish());
  });

  it("stays terminal after a framing failure", () => {
    const decoder = new JsonlDecoder();
    assert.throws(() => decoder.push(bytes("nope\n")));
    assert.throws(() => decoder.push(bytes('{"a":1}\n')));
  });

  it("encodes one LF-terminated record and refuses an oversized one", () => {
    assert.equal(encodeRecord({ a: 1 }).toString("utf8"), '{"a":1}\n');
    assert.throws(
      () => encodeRecord({ t: "x".repeat(100) }, 32),
      (error: unknown) =>
        error instanceof JsonlFramingError && error.kind === "record_too_large",
    );
  });

  it("round-trips an encoded record through the decoder", () => {
    const value = { method: "snapshot_get", id: 7, note: "a\nb\r\nc☃" };
    const decoder = new JsonlDecoder();
    assert.deepEqual(decoder.push(encodeRecord(value)), [value]);
  });

  it("shares the v2 record limit with the Rust transport", () => {
    assert.equal(JSONL_MAX_RECORD_BYTES, 8 * 1024 * 1024);
  });
});
