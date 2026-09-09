/**
 * The one authoritative `Number` conversion seam of the Runtime Client
 * protocol.
 *
 * The runtime's canonical `Number` domain is the finite IEEE-754 binary64
 * (`FiniteNumber` in Rust) — the very domain a JavaScript `number` holds. The
 * three things this protocol keeps apart are easy to conflate:
 *
 * ```text
 * human decimal spelling      client-local presentation ("1.5e3")
 *   -> finite binary64        the semantic value        (1500.0)
 *   -> canonical wire text    an exact encoding of the *value*
 * ```
 *
 * The wire carries the **value**, never the spelling, and the spelling never
 * has to survive: `readNumberDraft` turns a human decimal into one binary64,
 * and everything after that point is about preserving *that* value exactly.
 *
 * # Why a JSON number could not do it
 *
 * `JSON.stringify` prints the shortest decimal that round-trips a `number`,
 * not the exact integer the binary64 denotes. The exact binary64 `2^63` is the
 * mathematical integer
 *
 * ```text
 * 9223372036854775808
 * ```
 *
 * and `JSON.stringify(2 ** 63)` emits
 *
 * ```text
 * 9223372036854776000
 * ```
 *
 * Those are different integers. They parse back to the same binary64, but a
 * reader that treats a JSON integer as an exact decimal integer — as the Rust
 * decoder must, to refuse a decimal binary64 cannot hold — sees a value it has
 * to reject, and a question whose only legal answer is `2^63` becomes
 * publishable and unanswerable. Binary64 identity must therefore not depend on
 * JavaScript's decimal rendering of a `number` at all.
 *
 * # The canonical wire form
 *
 * A `Number` bound and a `Number` answer both cross this protocol as the
 * value's own IEEE-754 bit pattern, written as exactly
 * {@link FINITE_NUMBER_WIRE_CHARS} lowercase hexadecimal digits, most
 * significant first:
 *
 * ```text
 * 2^63   -> "43e0000000000000"
 * -2^63  -> "c3e0000000000000"
 * 0.1    -> "3fb999999999999a"
 * ```
 *
 * It is exact (the value's own bits, so no decimal parser sits in the trust
 * path), canonical (one value has one spelling, byte for byte, in both
 * languages), and bounded (always 16 bytes). It is **internal to the
 * protocol**: a human always sees and edits ordinary decimals, and an MCP
 * server always receives an ordinary JSON number.
 *
 * # Negative zero
 *
 * rustX canonicalizes `-0` to `+0`. Every comparison the domain takes part in
 * — the runtime's `Eq`, its `PartialOrd`, and the authoritative range check —
 * already treats them as one value, so admitting both bit patterns would give
 * one semantic value two canonical spellings. {@link finiteNumberToWire}
 * normalizes, and {@link finiteNumberFromWire} refuses
 * `"8000000000000000"` as a non-canonical spelling of `0`, exactly as the
 * runtime does.
 */

import type { FiniteNumberWire } from "./types.ts";

/** The exact width of a canonical `Number` wire spelling. */
export const FINITE_NUMBER_WIRE_CHARS = 16;

const CANONICAL_WIRE = /^[0-9a-f]{16}$/;
const NEGATIVE_ZERO_WIRE = "8000000000000000";

/**
 * One reusable 8-byte view. The conversions are synchronous and never yield,
 * so the buffer cannot be observed between a write and its read.
 */
const BITS = new DataView(new ArrayBuffer(8));

/** A `Number` value that is not a legal member of the runtime's domain. */
export class FiniteNumberWireError extends Error {
  constructor(message: string) {
    super(message);
    this.name = "FiniteNumberWireError";
  }
}

/**
 * Encodes one finite binary64 as its canonical Runtime Client spelling.
 *
 * The caller is expected to have established the value already — this is the
 * encoding step, not the admissibility step, and it deliberately performs no
 * decimal parsing of its own.
 *
 * @throws {FiniteNumberWireError} when the value is `NaN` or infinite, neither
 * of which the runtime's domain can hold.
 */
export function finiteNumberToWire(value: number): FiniteNumberWire {
  if (!Number.isFinite(value)) {
    throw new FiniteNumberWireError(
      `${value} is not a finite number, so it is not a value this runtime can hold`,
    );
  }
  // `-0 === 0`, so this normalizes negative zero without a bit test.
  BITS.setFloat64(0, value === 0 ? 0 : value, false);
  return BITS.getBigUint64(0, false).toString(16).padStart(FINITE_NUMBER_WIRE_CHARS, "0");
}

/**
 * Decodes one canonical Runtime Client `Number` spelling.
 *
 * Reconstruction is exact by construction: the bits are the value. Every
 * `Number` bound and every decoded `Number` answer in this client goes through
 * here, so no second parser can disagree with this one.
 *
 * @throws {FiniteNumberWireError} when the text is not the canonical width and
 * alphabet, names `NaN` or an infinity, or spells the negative zero the domain
 * canonicalizes away.
 */
export function finiteNumberFromWire(wire: string): number {
  if (!CANONICAL_WIRE.test(wire)) {
    throw new FiniteNumberWireError(
      `"${wire}" is not a canonical binary64 value: write exactly ` +
        `${FINITE_NUMBER_WIRE_CHARS} lowercase hexadecimal digits of its IEEE-754 bit pattern`,
    );
  }
  if (wire === NEGATIVE_ZERO_WIRE) {
    throw new FiniteNumberWireError(
      `"${wire}" spells negative zero, which this domain canonicalizes to "${finiteNumberToWire(0)}"`,
    );
  }
  BITS.setBigUint64(0, BigInt(`0x${wire}`), false);
  const value = BITS.getFloat64(0, false);
  if (!Number.isFinite(value)) {
    throw new FiniteNumberWireError(
      `"${wire}" does not name a finite 64-bit binary floating-point number`,
    );
  }
  return value;
}

/**
 * The decimal a human reads for one finite binary64 — and can type back.
 *
 * `String(value)` prints the shortest decimal that *round-trips*, which is not
 * always the value's own decimal: `String(2 ** 63)` is `9223372036854776000`,
 * while the binary64 is the integer `9223372036854775808`. That matters here
 * because the client admits a *whole* decimal only when binary64 holds that
 * integer exactly, so the shortest spelling of a large whole number is a
 * decimal this very client would refuse — a bound the user is shown and cannot
 * enter.
 *
 * A whole number is therefore printed as its exact integer, which `BigInt`
 * takes from the binary64 losslessly. A fractional value keeps the shortest
 * round-tripping spelling, which denotes the nearest binary64 — the same value
 * — and is what a reader expects to see.
 */
export function finiteNumberDecimal(value: number): string {
  return Number.isInteger(value) ? BigInt(value).toString() : String(value);
}
