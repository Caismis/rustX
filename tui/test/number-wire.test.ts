/**
 * The exact `Number` wire representation of the Runtime Client protocol.
 *
 * The property under test is the one the protocol depends on:
 *
 * ```text
 * finiteNumberFromWire(finiteNumberToWire(x)) === x
 * ```
 *
 * for every finite binary64, with no dependence on JavaScript choosing a
 * particular decimal spelling for a `number`. The Rust side asserts the same
 * property over the same representative values in
 * `src/events/interaction.rs`, and the two meet on the shared fixture bytes
 * exercised by `protocol-questionnaire-number.test.ts`.
 */

import assert from "node:assert/strict";
import { describe, it } from "node:test";

import {
  FINITE_NUMBER_WIRE_CHARS,
  FiniteNumberWireError,
  finiteNumberDecimal,
  finiteNumberFromWire,
  finiteNumberToWire,
} from "../src/protocol/number.ts";

/**
 * The representative values every stage of the pipeline is proven against:
 * the zeros, small magnitudes, ordinary fractions, both sides of the `2^53`
 * precision frontier, `±2^63`, and the extremes of the finite range.
 */
const REPRESENTATIVE: Array<[label: string, value: number]> = [
  ["0", 0],
  ["-0", -0],
  ["1", 1],
  ["-1", -1],
  ["0.1", 0.1],
  ["1.5", 1.5],
  ["-2.75", -2.75],
  ["1e-300", 1e-300],
  ["2^53 - 1", 2 ** 53 - 1],
  ["2^53", 2 ** 53],
  ["2^53 + 2", 2 ** 53 + 2],
  ["2^54", 2 ** 54],
  ["2^63", 2 ** 63],
  ["-2^63", -(2 ** 63)],
  ["2^1000", 2 ** 1000],
  ["Number.MAX_VALUE", Number.MAX_VALUE],
  ["Number.MIN_VALUE", Number.MIN_VALUE],
  ["-Number.MAX_VALUE", -Number.MAX_VALUE],
];

describe("the canonical Number wire representation", () => {
  it("round-trips every representative finite binary64 exactly", () => {
    // The Rust side asserts the same property over the same values, and pins
    // these spellings too, so the two tables cannot drift apart silently.
    const pinned: Record<string, string> = {
      "2^53": "4340000000000000",
      "2^53 + 2": "4340000000000001",
      "2^54": "4350000000000000",
      "2^63": "43e0000000000000",
      "-2^63": "c3e0000000000000",
      "2^1000": "7e70000000000000",
      "Number.MAX_VALUE": "7fefffffffffffff",
      "Number.MIN_VALUE": "0000000000000001",
    };
    for (const [label, value] of REPRESENTATIVE) {
      const expected = pinned[label];
      if (expected !== undefined) assert.equal(finiteNumberToWire(value), expected, label);
    }

    for (const [label, value] of REPRESENTATIVE) {
      const wire = finiteNumberToWire(value);
      assert.equal(wire.length, FINITE_NUMBER_WIRE_CHARS, label);
      assert.match(wire, /^[0-9a-f]{16}$/, label);
      const decoded = finiteNumberFromWire(wire);
      // `Object.is` rather than `===`, so a `-0` that failed to canonicalize
      // would be a failure rather than silently equal to `0`.
      assert.ok(
        Object.is(decoded, value === 0 ? 0 : value),
        `${label}: ${wire} decoded to ${decoded}`,
      );
      // And the spelling is settled: re-encoding is byte-identical.
      assert.equal(finiteNumberToWire(decoded), wire, label);
    }
  });

  it("pins the wire spelling of the value JSON.stringify cannot preserve", () => {
    // The concrete failure this representation exists to close. `2^63` is an
    // exact binary64 — a power of two — but its shortest round-tripping
    // decimal is a different mathematical integer, so a JSON number cannot
    // carry binary64 identity across the protocol.
    const two63 = 2 ** 63;
    assert.equal(JSON.stringify(two63), "9223372036854776000");
    assert.notEqual(JSON.stringify(two63), "9223372036854775808");
    assert.equal(BigInt(two63).toString(), "9223372036854775808");

    assert.equal(finiteNumberToWire(two63), "43e0000000000000");
    assert.equal(finiteNumberFromWire("43e0000000000000"), two63);
    assert.equal(finiteNumberToWire(-two63), "c3e0000000000000");
    assert.equal(finiteNumberFromWire("c3e0000000000000"), -two63);
    // The wire form is transport-safe in a way the JSON number was not: it is
    // a string, so `JSON.stringify` cannot reformat it.
    assert.equal(
      JSON.stringify({ value: finiteNumberToWire(two63) }),
      '{"value":"43e0000000000000"}',
    );
  });

  it("canonicalizes negative zero rather than carrying two spellings", () => {
    // rustX has one semantic zero. `-0` encodes as `+0`, and the `-0` bit
    // pattern is refused on the wire the way `ExactInteger` would refuse "-0".
    assert.equal(finiteNumberToWire(-0), "0000000000000000");
    assert.equal(finiteNumberToWire(0), "0000000000000000");
    assert.ok(Object.is(finiteNumberFromWire("0000000000000000"), 0));
    assert.throws(
      () => finiteNumberFromWire("8000000000000000"),
      (error: unknown) =>
        error instanceof FiniteNumberWireError && /negative zero/.test(error.message),
    );
  });

  it("refuses every non-canonical spelling and every non-finite value", () => {
    for (const malformed of [
      "",
      "0",
      "43E0000000000000", // uppercase is a second spelling of one value
      "0x43e0000000000000", // no prefix
      "43e000000000000", // 15 digits
      "43e00000000000000", // 17 digits
      " 43e0000000000000",
      "43e000000000000g",
      "9223372036854775808", // the decimal, not the wire form
    ]) {
      assert.throws(
        () => finiteNumberFromWire(malformed),
        FiniteNumberWireError,
        `${JSON.stringify(malformed)} is not a canonical binary64 value`,
      );
    }
    for (const nonFinite of [
      "7ff0000000000000", // +Infinity
      "fff0000000000000", // -Infinity
      "7ff8000000000000", // NaN
      "7fffffffffffffff", // a signalling NaN payload
    ]) {
      assert.throws(
        () => finiteNumberFromWire(nonFinite),
        (error: unknown) =>
          error instanceof FiniteNumberWireError && /finite/.test(error.message),
        `${nonFinite} does not name a finite value`,
      );
    }
    for (const nonFinite of [Number.NaN, Infinity, -Infinity]) {
      assert.throws(() => finiteNumberToWire(nonFinite), FiniteNumberWireError);
    }
  });

  it("presents a whole number as the decimal a human can type back", () => {
    // The display seam, which is not the wire seam: a human never sees bits,
    // and must never be shown a bound this client would itself refuse.
    assert.equal(finiteNumberDecimal(2 ** 63), "9223372036854775808");
    assert.equal(finiteNumberDecimal(-(2 ** 63)), "-9223372036854775808");
    assert.equal(finiteNumberDecimal(2 ** 53), "9007199254740992");
    assert.equal(finiteNumberDecimal(0), "0");
    assert.equal(finiteNumberDecimal(-0), "0");
    assert.equal(finiteNumberDecimal(1500), "1500");
    // Fractions keep the shortest round-tripping spelling, which denotes the
    // same binary64.
    assert.equal(finiteNumberDecimal(0.1), "0.1");
    assert.equal(finiteNumberDecimal(-2.75), "-2.75");
    assert.equal(finiteNumberDecimal(1e-300), "1e-300");
  });
});
