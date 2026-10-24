/**
 * The one authoritative numeric input and conversion seam of the Runtime
 * Client protocol, shared by every client (Issue #487).
 *
 * Pure and browser-safe: no I/O, schema loading or Node built-ins. Clients
 * keep their own presentation and translation of the machine-readable
 * reasons below; the reading, the exactness rule and the wire encoding exist
 * once, here.
 *
 * # `Number`
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

import type { AnswerSpecification, ExactInteger, FiniteNumber as FiniteNumberWire, QuestionnaireAnswer } from "./v57.ts";

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

/** The inclusive bounds of the runtime's canonical `Integer` domain. */
const I64_MIN = -(2n ** 63n);
const I64_MAX = 2n ** 63n - 1n;

/**
 * One `Number` draft's decimal syntax, split into the parts its exact value is
 * built from: integer digits, fraction digits, and a signed exponent.
 *
 * The accepted spellings are `123`, `-123`, `1.5`, `.5`, `1.`, `1e3`, `1.5e3`,
 * `1E-2`, `-2.5e+4`. A leading `+`, a bare `.`, and a bare exponent are not
 * numbers; the digit check below refuses the two the pattern alone would let
 * through.
 */
const NUMBER_DRAFT = /^-?(\d*)(?:\.(\d*))?(?:[eE]([+-]?)(\d+))?$/;

/** `2^53`: the first magnitude at which binary64 stops holding every whole number. */
const EXACT_WHOLE_FRONTIER = 2n ** 53n;

/** What one `Number` draft denotes, or why it denotes nothing submittable. */
export type NumberDraftReading =
  /** The finite binary64 value the draft denotes exactly. */
  | { readonly kind: "value"; readonly value: number }
  /** Not the syntax of a number. */
  | { readonly kind: "malformed" }
  /** Beyond the finite binary64 range. */
  | { readonly kind: "not-finite" }
  /** A whole number binary64 cannot hold exactly. */
  | { readonly kind: "inexact" };

/**
 * Reads one `Number` draft as the value the runtime would receive.
 *
 * This is the single point where a human's decimal spelling becomes a
 * JavaScript `number`, and it is deliberately taken **on the original string**:
 * `Number(...)` rounds, and rounding is precisely the information this has to
 * inspect. Asking whether the draft is exact after `Number(...)` has already
 * run can only ever confirm that a rounded value is equal to itself.
 *
 * The runtime's `Number` domain is the finite binary64 (`FiniteNumber`), and a
 * fractional decimal *means* the nearest binary64 — that is what a JSON number
 * means everywhere, and where the MCP server's own parse lands. A **whole**
 * number is different: `9007199254740993` names one specific integer, and
 * binary64 cannot hold it, so `FiniteNumber` refuses it rather than rounding it
 * to `9007199254740992`. That rule is about the *value*, so it cannot be evaded
 * by respelling: `9007199254740993.0`, `9.007199254740993e15` and
 * `90071992547409930e-1` all denote the same refused integer, and this reads
 * every one of them out of the decimal itself rather than out of the rounded
 * `Number`.
 *
 * Nothing here widens the domain: no arbitrary-precision value crosses the
 * wire. The exact arithmetic exists only to decide whether one lossy
 * conversion is faithful, and the value submitted is the ordinary binary64.
 */
export function readNumberDraft(draft: string): NumberDraftReading {
  const spelling = draft.trim();
  const match = NUMBER_DRAFT.exec(spelling);
  if (match === null) return { kind: "malformed" };
  const [, integerDigits = "", fractionDigits = "", exponentSign, exponentDigits] = match;
  // `.` and `e3` match the pattern but spell no digits, so they are not numbers.
  if (integerDigits.length + fractionDigits.length === 0) return { kind: "malformed" };

  const value = Number(spelling);
  if (!Number.isFinite(value)) return { kind: "not-finite" };

  const magnitude = exponentDigits === undefined ? 0n : BigInt(exponentDigits);
  const exponent = exponentSign === "-" ? -magnitude : magnitude;
  const whole = decimalWholeValue(integerDigits, fractionDigits, exponent);
  if (whole !== undefined && !isExactBinary64Whole(whole)) return { kind: "inexact" };
  return { kind: "value", value };
}

/**
 * The exact integer a well-formed decimal spelling denotes, or `undefined`
 * when it denotes a fractional value.
 *
 * The digits are one integer scaled by a power of ten: `1.5e3` is `15` scaled
 * by `10^2`. A negative scale is cancelled one digit at a time, which both
 * decides wholeness and terminates immediately on the first non-zero digit
 * below the point — so an absurd exponent like `1e-999999999` costs one step
 * rather than a `10^999999999`.
 *
 * The sign is dropped: binary64 exactness is symmetric, so `-9007199254740993`
 * and `9007199254740993` are decided identically.
 *
 * The caller has already proven the draft finite. That bounds a non-zero
 * `scale` at 308, because the value is at least `10^scale` and a finite
 * binary64 is below `2^1024`; zero is returned before `scale` is used at all.
 */
function decimalWholeValue(
  integerDigits: string,
  fractionDigits: string,
  exponent: bigint,
): bigint | undefined {
  let significand = BigInt(`${integerDigits}${fractionDigits}`);
  // Every spelling of zero is whole, and no exponent can change that — which
  // is also what keeps `0e999999999` from reaching the exponentiation below.
  if (significand === 0n) return 0n;
  let scale = exponent - BigInt(fractionDigits.length);
  while (scale < 0n) {
    if (significand % 10n !== 0n) return undefined;
    significand /= 10n;
    scale += 1n;
  }
  return significand * 10n ** scale;
}

/**
 * Whether binary64 holds this whole number exactly.
 *
 * Every whole number is an odd number times a power of two, and binary64's
 * significand is 53 bits, so it is exact when that odd part fits in 53 bits.
 * This is the same rule the runtime applies in `is_exact_binary64_whole`,
 * stated over `BigInt` instead of `u64` — the client is therefore never looser
 * than the boundary it is about to submit to.
 */
function isExactBinary64Whole(value: bigint): boolean {
  let odd = value < 0n ? -value : value;
  if (odd === 0n) return true;
  while (odd % 2n === 0n) odd >>= 1n;
  return odd < EXACT_WHOLE_FRONTIER;
}

/** What one `Integer` draft denotes, or why it denotes nothing submittable. */
export type IntegerDraftReading =
  /** The canonical decimal text the runtime parses, and its exact value. */
  | { readonly kind: "value"; readonly value: bigint; readonly wire: ExactInteger }
  /** Not the canonical decimal syntax of a whole number. */
  | { readonly kind: "malformed" }
  /** A whole number outside the runtime's 64-bit domain. */
  | { readonly kind: "out-of-range" };

/**
 * Reads one `Integer` draft exactly as `ExactInteger::parse` will.
 *
 * Never `Number(...)`: the runtime's Integer domain is the exact i64, and a
 * JavaScript number would silently round every value above 2^53 — the whole
 * reason this field crosses the wire as decimal text. The draft is compared
 * as a `BigInt` and submitted as the canonical digits the runtime accepts: an
 * optional `-` and digits, no leading zero, no `-0`. Surrounding whitespace is
 * input editing, not part of the number, and is never submitted.
 */
export function readIntegerDraft(draft: string): IntegerDraftReading {
  const spelling = draft.trim();
  if (!/^(?:0|-?[1-9][0-9]*)$/.test(spelling)) return { kind: "malformed" };
  const value = BigInt(spelling);
  if (value < I64_MIN || value > I64_MAX) return { kind: "out-of-range" };
  return { kind: "value", value, wire: spelling };
}

/** Why one `Number` or `Integer` answer draft is not submittable. A declared
 * bound is carried as the decimal a human reads and can type back. */
export type ScalarAnswerRejection =
  | { readonly reason: "malformed" | "not-finite" | "inexact" | "out-of-range" }
  | { readonly reason: "below-minimum" | "above-maximum"; readonly bound: string };

/** One scalar answer draft read against its declared shape. */
export type ScalarAnswerReading =
  | { readonly kind: "answer"; readonly answer: Extract<QuestionnaireAnswer, { type: "number" | "integer" }> }
  | ({ readonly kind: "rejected" } & ScalarAnswerRejection);

type NumericAnswerSpecification = Extract<AnswerSpecification, { type: "number" | "integer" }>;

/**
 * Reads one `Number` or `Integer` draft into the exact wire answer, or the
 * machine-readable reason it is refused.
 *
 * Syntax, then finiteness, then admissibility to the canonical domain, and
 * only then the declared bounds: a value is never compared against
 * `minimum`/`maximum` before it is established that it is the value the user
 * actually entered. This is UX only — the runtime validates the very same
 * facts again and stays authoritative.
 */
export function readScalarAnswer(answer: NumericAnswerSpecification, draft: string): ScalarAnswerReading {
  if (answer.type === "integer") {
    const reading = readIntegerDraft(draft);
    if (reading.kind !== "value") return { kind: "rejected", reason: reading.kind };
    if (answer.minimum != null && reading.value < BigInt(answer.minimum)) return { kind: "rejected", reason: "below-minimum", bound: answer.minimum };
    if (answer.maximum != null && reading.value > BigInt(answer.maximum)) return { kind: "rejected", reason: "above-maximum", bound: answer.maximum };
    return { kind: "answer", answer: { type: "integer", value: { value: reading.wire } } };
  }
  const reading = readNumberDraft(draft);
  if (reading.kind !== "value") return { kind: "rejected", reason: reading.kind };
  // The bounds are canonical binary64 text, so they are decoded through the
  // one seam that reconstructs a `number` exactly and then compared in the
  // domain both sides belong to. Comparing the wire spellings as strings
  // would be a lexical comparison of bit patterns, which is not the numeric
  // order.
  if (answer.minimum != null && reading.value < finiteNumberFromWire(answer.minimum)) return { kind: "rejected", reason: "below-minimum", bound: finiteNumberDecimal(finiteNumberFromWire(answer.minimum)) };
  if (answer.maximum != null && reading.value > finiteNumberFromWire(answer.maximum)) return { kind: "rejected", reason: "above-maximum", bound: finiteNumberDecimal(finiteNumberFromWire(answer.maximum)) };
  return { kind: "answer", answer: { type: "number", value: { value: finiteNumberToWire(reading.value) } } };
}
