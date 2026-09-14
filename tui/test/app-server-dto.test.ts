/**
 * The TUI consumes the generated #288 DTOs, not a hand-maintained wire mirror.
 *
 * `protocol/app-server/fixtures.json` is written by Rust from the authoritative
 * App Server DTOs. This suite feeds those exact bytes through the client's real
 * classification path, so the contract under test is "the client understands
 * what Rust actually serializes" rather than "two transcriptions of the
 * protocol agree with each other".
 *
 * Runtime decoding uses the same generated schema as the TypeScript DTOs.
 */

import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { describe, it } from "node:test";

import { decodeProtocolMessage } from "../src/protocol/decoder.ts";

import {
  compareExact,
  describeRpcError,
  isFailure,
  isNotification,
  isResponse,
  isSuccess,
} from "../src/protocol/app-server.ts";

const FIXTURE_URL = new URL(
  "../../protocol/app-server/fixtures.json",
  import.meta.url,
);
const encoded = readFileSync(FIXTURE_URL, "utf8");
const fixtures = (JSON.parse(encoded) as unknown[]).map((record) => {
  const decoded = decodeProtocolMessage(record);
  assert.ok(decoded, "every Rust fixture satisfies the generated runtime contract");
  return decoded;
});

describe("Rust-produced App Server fixtures", () => {
  it("classifies every message as a request, a response, or a notification", () => {
    assert.ok(fixtures.length > 0, "the generated fixture set is not empty");
    for (const message of fixtures) {
      const record = message;
      const request = "method" in record && "id" in record;
      assert.ok(
        request || isResponse(record) || isNotification(record),
        `unclassified fixture: ${JSON.stringify(message)}`,
      );
      // The classes are mutually exclusive: a response is never routed as a
      // notification, and a notification never settles a pending request.
      assert.ok(!(isResponse(record) && isNotification(record)));
    }
  });

  it("separates a correlated result from a correlated failure", () => {
    const responses = fixtures.filter(isResponse);
    assert.ok(responses.length > 0);
    for (const response of responses) {
      assert.notEqual(
        isFailure(response),
        isSuccess(response),
        "a response carries exactly one of result or error",
      );
      if (isFailure(response)) {
        // Every typed failure renders without throwing, and never degrades to
        // a bare numeric code.
        const described = describeRpcError(response.error);
        assert.equal(typeof described, "string");
        assert.ok(described.length > 0);
      }
    }
  });

  it("carries exact u64 domains as canonical decimal text above 2^53", () => {
    // 9007199254740993 is 2^53 + 1: the first integer a JSON number cannot
    // represent. Finding it only inside quotes is the whole point of the
    // exact domains.
    assert.ok(
      encoded.includes('"9007199254740993"'),
      "an exact domain crosses the wire as text",
    );
    assert.ok(
      !/[^"]9007199254740993/.test(encoded),
      "no exact domain crosses the wire as a JSON number",
    );
  });

  it("orders exact domains numerically, never lexicographically", () => {
    assert.equal(compareExact("9", "10"), -1);
    assert.equal(compareExact("9007199254740993", "9007199254740992"), 1);
    assert.equal(compareExact("42", "42"), 0);
  });

  it("preserves an attachment target's four distinct identity domains", () => {
    const target = fixtures
      .map((message) => message as { params?: { target?: Record<string, string> } })
      .map((message) => message.params?.target)
      .find((candidate) => candidate !== undefined);
    assert.ok(target, "the fixture set addresses an attachment");
    // Session, Conversation, runtime incarnation and attachment are separate
    // domains. A client that collapsed any two of them would let a stale
    // incarnation's events reach its replacement.
    assert.deepEqual(Object.keys(target).sort(), [
      "attachment_id",
      "conversation_id",
      "runtime_incarnation",
      "session_id",
    ]);
  });
});
