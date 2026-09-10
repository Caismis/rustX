/**
 * The cross-language `Number` wire contract, proven on the real bytes.
 *
 * The fixtures under `tests/fixtures/runtime-client/` are the shared contract
 * between this client and the Rust runtime. Two of them pin the `Number`
 * domain, and `tests/scripted/runtime_client/protocol.rs` asserts the other
 * half of each:
 *
 * ```text
 * questionnaire-number-v24.json           Rust writes  -> this client reads
 * questionnaire-number-response-v24.jsonl this client writes -> Rust reads
 * ```
 *
 * The chain this file proves end to end, with no re-implementation of either
 * side's encoder:
 *
 * ```text
 * Rust publishes a Number question, minimum = maximum = 2^63
 *   -> the canonical binary64 wire form reaches the TUI
 *   -> QuestionnaireOverlay accepts the draft "9223372036854775808"
 *   -> readNumberDraft settles one exact binary64 (2^63)
 *   -> finiteNumberToWire encodes it exactly
 *   -> the real encodeRecord / JSON.stringify writes the JSONL bytes
 *   -> Rust decodes those very bytes back to FiniteNumber(2^63)
 * ```
 *
 * `2^63` is the case that matters: it is an exact binary64 (a power of two),
 * but `JSON.stringify(2 ** 63)` prints `9223372036854776000`, a *different*
 * mathematical integer. A raw JSON number therefore cannot carry binary64
 * identity across this boundary, which is why the domain has a wire form of
 * its own.
 */

import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { describe, it } from "node:test";

import { encodeRecord } from "../src/protocol/jsonl.ts";
import { finiteNumberFromWire, finiteNumberToWire } from "../src/protocol/number.ts";
import type {
  InteractionRef,
  InteractionRequester,
  QuestionnaireResponse,
  QuestionnaireSpecification,
} from "../src/protocol/types.ts";
import { QuestionnaireOverlay } from "../src/ui/components/questionnaire.ts";
import { plainText } from "../src/ui/theme.ts";

const REQUEST_FIXTURE = new URL(
  "../../tests/fixtures/runtime-client/questionnaire-number-v24.json",
  import.meta.url,
);
const RESPONSE_FIXTURE = new URL(
  "../../tests/fixtures/runtime-client/questionnaire-number-response-v24.jsonl",
  import.meta.url,
);

/** The one value this question admits, and the decimal a human types for it. */
const TWO_POW_63 = 2 ** 63;
const TWO_POW_63_DECIMAL = "9223372036854775808";

type NumberRequestFixture = {
  id: string;
  conversation_id: string;
  kind: {
    type: "questionnaire";
    requester: InteractionRequester;
    questionnaire: QuestionnaireSpecification;
  };
};

function overlayFor(
  request: NumberRequestFixture,
  onSubmit: (response: QuestionnaireResponse) => void = () => {},
): QuestionnaireOverlay {
  return new QuestionnaireOverlay({
    interactionId: request.id,
    questionnaire: request.kind.questionnaire,
    requester: request.kind.requester,
    onSubmit,
    onDecline: () => assert.fail("the questionnaire is submitted, never declined"),
    onInterrupt: () => assert.fail("the questionnaire never cancels the attempt"),
  });
}

function publishedQuestion(): NumberRequestFixture {
  return JSON.parse(readFileSync(REQUEST_FIXTURE, "utf8")) as NumberRequestFixture;
}

function answer(draft: string): QuestionnaireResponse | undefined {
  let submitted: QuestionnaireResponse | undefined;
  const overlay = overlayFor(publishedQuestion(), (response) => {
    submitted = response;
  });
  for (const scalar of draft) overlay.handleInput(scalar);
  // Walk to the review tab and submit, exactly as a user does.
  overlay.handleInput("\t");
  overlay.handleInput("\r");
  return submitted;
}

describe("the cross-language Number wire contract", () => {
  it("reads the published 2^63 bound out of the shared request fixture", () => {
    const request = publishedQuestion();
    const declared = request.kind.questionnaire.questions[0]?.answer;
    assert.equal(declared?.type, "number");
    if (declared?.type !== "number") throw new Error("not a number question");

    // The bound crosses as canonical binary64 text and reconstructs exactly.
    assert.equal(declared.minimum, "43e0000000000000");
    assert.equal(declared.maximum, "43e0000000000000");
    assert.equal(finiteNumberFromWire(declared.minimum!), TWO_POW_63);
    assert.equal(finiteNumberFromWire(declared.maximum!), TWO_POW_63);
    // Nothing rounded on the way: the bound is the exact integer 2^63, which
    // is not the decimal JSON.stringify would have printed for it.
    assert.equal(BigInt(finiteNumberFromWire(declared.minimum!)).toString(), TWO_POW_63_DECIMAL);
    assert.equal(JSON.stringify(TWO_POW_63), "9223372036854776000");
  });

  it("writes the exact JSONL bytes the Rust runtime decodes", () => {
    const submitted = answer(TWO_POW_63_DECIMAL);
    assert.deepEqual(submitted, {
      type: "submitted",
      value: {
        answers: [{
          question_index: 0,
          answer: { type: "number", value: { value: "43e0000000000000" } },
        }],
      },
    });

    // The real client request, framed by the real transport encoder. Nothing
    // here re-implements the wire: `encodeRecord` is the same function the
    // connection writes through, and `JSON.stringify` runs inside it.
    const request = publishedQuestion();
    const interaction: InteractionRef = {
      conversation_id: request.conversation_id,
      interaction_id: request.id,
    };
    const record = encodeRecord({
      method: "interaction_respond",
      interaction,
      response: { type: "questionnaire", response: submitted },
      id: 1,
    });

    const expected = readFileSync(RESPONSE_FIXTURE);
    assert.deepEqual(
      record,
      expected,
      "the bytes this client writes drifted from the fixture the Rust runtime " +
        "decodes in tests/scripted/runtime_client/protocol.rs",
    );
    // Stated directly: the record carries the value as text, so JSON.stringify
    // had no `number` to re-spell, and the lossy decimal appears nowhere.
    const bytes = record.toString("utf8");
    assert.ok(bytes.includes('"value":"43e0000000000000"'), bytes);
    assert.ok(!bytes.includes("9223372036854776000"), bytes);
    assert.equal(record.at(-1), 0x0a);
  });

  it("admits only the exact binary64 the bound names", () => {
    // decode(encode(x)) === x, on the value this question is pinned to.
    assert.equal(finiteNumberFromWire(finiteNumberToWire(TWO_POW_63)), TWO_POW_63);

    // The neighbouring integers both *round* to 2^63, so a client that
    // converted before checking would have found them in range. Admissibility
    // to the domain is settled first, so neither can be submitted.
    for (const inadmissible of ["9223372036854775807", "9223372036854775809"]) {
      assert.equal(Number(inadmissible), TWO_POW_63, `${inadmissible} rounds into range`);
      assert.equal(answer(inadmissible), undefined, `${inadmissible} never submits`);
    }
    // And the previous representable value is exact but out of range.
    assert.equal(answer("9223372036854774784"), undefined);

    // The human is shown the decimal they can type, never the bit pattern.
    const rendered = plainText(overlayFor(publishedQuestion()).render(80).join("\n"));
    assert.match(rendered, /Number between 9223372036854775808 and 9223372036854775808/);
    assert.doesNotMatch(rendered, /43e0000000000000/);
  });
});
