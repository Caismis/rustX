import assert from "node:assert/strict";
import { describe, it } from "node:test";

import { visibleWidth } from "@earendil-works/pi-tui";

import { QuestionnaireOverlay } from "../src/ui/components/questionnaire.ts";
import type {
  InteractionRequester,
  QuestionnaireResponse,
  QuestionnaireSpecification,
} from "../src/protocol/types.ts";
import { plainText } from "../src/ui/theme.ts";

function questionnaire(): QuestionnaireSpecification {
  return {
    questions: [
      {
        question: "Which visual direction should I use?",
        header: "Visual style",
        answer: {
          type: "single_choice",
          options: [
          {
            label: "Swiss / Klein blue",
            description: "Information-first typography with strong hierarchy.",
            preview: "## Swiss preview\n\nBlue hierarchy.",
          },
          {
            label: "Electronic magazine",
            description: "A warmer editorial composition with serif typography.",
          },
        ],
          allow_custom: true,
        },
      },
      {
        question: "Which elements should be enabled?",
        header: "Elements",
        answer: {
          type: "multi_choice",
          options: [
          { label: "Charts", description: "Show quantitative charts." },
          { label: "Comments", description: "Show reviewer comments." },
        ],
          min_selected: 1,
          max_selected: 2,
          allow_custom: true,
        },
      },
    ],
  };
}

/** The native `ask_user` requester: built-in, never labelled as MCP. */
const NATIVE_REQUESTER: InteractionRequester = {
  tool_id: "tool-ask-user",
  tool_name: "ask_user",
  origin: "builtin",
};

/** An MCP-served tool's requester, carrying its canonical server identity. */
const MCP_REQUESTER: InteractionRequester = {
  tool_id: "mcp:github:create_issue",
  tool_name: "create_issue",
  origin: { mcp: { server_id: "github" } },
};

function overlay(
  onSubmit: (response: QuestionnaireResponse) => void,
  onDecline: () => void = () => {},
  onInterrupt: () => void = () => {},
): QuestionnaireOverlay {
  return new QuestionnaireOverlay({
    interactionId: "attempt-1-interaction-questionnaire-1",
    questionnaire: questionnaire(),
    requester: NATIVE_REQUESTER,
    onSubmit,
    onDecline,
    onInterrupt,
  });
}

function singleQuestionnaire(
  question: QuestionnaireSpecification["questions"][number] = questionnaire().questions[0]!,
): QuestionnaireSpecification {
  return { questions: [question] };
}

function singleOverlay(
  specification: QuestionnaireSpecification,
  onSubmit: (response: QuestionnaireResponse) => void,
): QuestionnaireOverlay {
  return new QuestionnaireOverlay({
    interactionId: "interaction-single",
    questionnaire: specification,
    requester: NATIVE_REQUESTER,
    onSubmit,
    onDecline: () => {},
    onInterrupt: () => {},
  });
}

function focusCustom(view: QuestionnaireOverlay, optionCount: number): void {
  for (let index = 0; index < optionCount; index += 1) {
    view.handleInput("\u001b[B");
  }
}

function submitSingle(view: QuestionnaireOverlay): void {
  view.handleInput("\t");
  view.handleInput("\r");
}

function assertBounded(lines: string[], width: number, height: number): void {
  assert.ok(lines.length <= height, `expected at most ${height} lines, got ${lines.length}`);
  assert.ok(
    lines.every((line) => visibleWidth(line) <= width),
    "every rendered line must fit the requested display width",
  );
}

type PreviewPage = {
  lines: string[];
  numbers: number[];
  first: number;
  last: number;
  total: number;
};

function previewPage(view: QuestionnaireOverlay, width: number): PreviewPage {
  const lines = view.render(width).map(plainText);
  const status = lines.find((line) => line.includes("lines "));
  const match = status?.match(/lines (\d+)-(\d+) of (\d+)/);
  assert.ok(match, "the visible preview must expose its bounded line range");
  const numbers = lines.flatMap((line) =>
    [...line.matchAll(/preview-(\d{3})/g)].map((found) => Number(found[1])),
  );
  assert.ok(numbers.length > 0, "the visible page must contain preview content");
  const first = Number(match[1]);
  const last = Number(match[2]);
  const total = Number(match[3]);
  assert.equal(first, numbers[0]! + 1, "preview status must start at the first visible line");
  assert.equal(last, numbers[numbers.length - 1]! + 1, "preview status must end at the last visible line");
  assert.equal(last - first + 1, numbers.length, "preview content must be contiguous");
  for (const [index, number] of numbers.entries()) {
    assert.equal(number, first - 1 + index, "preview lines must not have gaps");
  }
  return { lines, numbers, first, last, total };
}

function assertFocusedOption(page: PreviewPage, label: string): void {
  const focused = page.lines.filter((line) => line.includes("›"));
  assert.equal(focused.length, 1, "exactly one focus marker must remain visible");
  assert.ok(focused[0]!.includes(label), `focused row should identify ${label}`);
}

describe("QuestionnaireOverlay", () => {
  it("preserves a single selection while switching tabs and submits in question order", () => {
    let submitted: QuestionnaireResponse | undefined;
    let changes = 0;
    const view = new QuestionnaireOverlay({
      interactionId: "interaction-1",
      questionnaire: questionnaire(),
      requester: NATIVE_REQUESTER,
      onSubmit: (response) => {
        submitted = response;
      },
      onDecline: () => {},
      onInterrupt: () => {},
      onChange: () => {
        changes += 1;
      },
    });

    view.handleInput("\r");
    view.handleInput("\t");
    view.handleInput("\u001b[Z");
    assert.match(view.render(80).join("\n"), /●/);
    view.handleInput("\t");
    view.handleInput("\t");
    view.handleInput("\r");

    assert.deepEqual(submitted, {
      type: "submitted",
      value: {
        answers: [{
          question_index: 0,
          answer: { type: "option", value: { option_index: 0 } },
        }],
      },
    });
    assert.ok(changes >= 3);
  });

  it("supports authored-order multi-select and partial submission", () => {
    let submitted: QuestionnaireResponse | undefined;
    const view = overlay((response) => {
      submitted = response;
    });

    view.handleInput("\t");
    view.handleInput(" ");
    view.handleInput("\u001b[B");
    view.handleInput(" ");
    view.handleInput("\t");
    view.handleInput("\r");

    assert.deepEqual(submitted, {
      type: "submitted",
      value: {
        answers: [{
          question_index: 1,
          answer: {
            type: "options",
            value: { option_indices: [0, 1] },
          },
        }],
      },
    });
  });

  it("supports bounded custom answers and renders previews at narrow and wide widths", () => {
    let submitted: QuestionnaireResponse | undefined;
    const view = overlay((response) => {
      submitted = response;
    });
    const narrow = view.render(56).join("\n");
    const wide = view.render(120).join("\n");

    view.handleInput("\u001b[B");
    view.handleInput("\u001b[B");
    view.handleInput("x");
    view.handleInput(" ");
    view.handleInput("y");
    view.handleInput("\r");
    view.handleInput("\t");
    view.handleInput("\t");
    view.handleInput("\r");

    assert.deepEqual(submitted, {
      type: "submitted",
      value: {
        answers: [{
          question_index: 0,
          answer: { type: "custom", value: { answer: "x y" } },
        }],
      },
    });

    assert.match(narrow, /Swiss preview/);
    assert.match(wide, /Swiss preview/);
    assert.match(narrow, /Type something\./);
  });

  it("keeps decline and attempt cancellation as separate keyboard actions", () => {
    let declined = 0;
    let cancelled = 0;
    const view = overlay(
      () => assert.fail("decline must not submit a questionnaire response"),
      () => {
        declined += 1;
      },
      () => {
        cancelled += 1;
      },
    );

    view.handleInput("\u001b");
    assert.equal(declined, 1);
    assert.equal(cancelled, 0);

    const second = overlay(
      () => assert.fail("interrupt must not submit a questionnaire response"),
      () => assert.fail("interrupt must not decline a questionnaire"),
      () => {
        cancelled += 1;
      },
    );
    second.handleInput("\u0003");
    assert.equal(cancelled, 1);
  });

  it("preserves complete and chunked bracketed paste through tab switches", () => {
    let submitted: QuestionnaireResponse | undefined;
    const view = overlay((response) => {
      submitted = response;
    });

    focusCustom(view, 2);
    view.handleInput("\u001b[200~pasted text\u001b[201~");
    view.handleInput("\t");
    view.handleInput("\u001b[Z");
    view.handleInput("\t");
    view.handleInput("\t");
    view.handleInput("\r");

    assert.deepEqual(submitted, {
      type: "submitted",
      value: {
        answers: [{
          question_index: 0,
          answer: { type: "custom", value: { answer: "pasted text" } },
        }],
      },
    });

    let chunkedSubmitted: QuestionnaireResponse | undefined;
    const chunked = overlay((response) => {
      chunkedSubmitted = response;
    });
    focusCustom(chunked, 2);
    chunked.handleInput("\u001b[200~pasted ");
    chunked.handleInput("text");
    chunked.handleInput("\u001b[201~");
    chunked.handleInput("\t");
    chunked.handleInput("\t");
    chunked.handleInput("\r");
    assert.deepEqual(chunkedSubmitted, submitted);
  });

  it("accepts ordinary multi-character, CJK, emoji, and Kitty printable input", () => {
    let submitted: QuestionnaireResponse | undefined;
    const view = singleOverlay(singleQuestionnaire(), (response) => {
      submitted = response;
    });

    focusCustom(view, 2);
    view.handleInput("你好😀");
    view.handleInput("\u001b[D");
    view.handleInput("🌟");
    view.handleInput("\u001b[97u");
    submitSingle(view);

    assert.deepEqual(submitted, {
      type: "submitted",
      value: {
        answers: [{
          question_index: 0,
          answer: { type: "custom", value: { answer: "你好🌟a😀" } },
        }],
      },
    });
  });

  it("bounds pasted custom answers by Unicode scalar count", () => {
    let exact: QuestionnaireResponse | undefined;
    const exactView = singleOverlay(singleQuestionnaire(), (response) => {
      exact = response;
    });
    focusCustom(exactView, 2);
    const exactText = "😀".repeat(4096);
    exactView.handleInput(`\u001b[200~${exactText}\u001b[201~`);
    submitSingle(exactView);

    assert.equal(exact?.type, "submitted");
    if (exact?.type === "submitted") {
      const answer = exact.value.answers[0]?.answer;
      assert.equal(answer?.type, "custom");
      if (answer?.type === "custom") assert.equal([...answer.value.answer].length, 4096);
    }

    let overflow: QuestionnaireResponse | undefined;
    const overflowView = singleOverlay(singleQuestionnaire(), (response) => {
      overflow = response;
    });
    focusCustom(overflowView, 2);
    overflowView.handleInput(`\u001b[200~${"x".repeat(4097)}\u001b[201~`);
    submitSingle(overflowView);

    assert.equal(overflow?.type, "submitted");
    if (overflow?.type === "submitted") {
      const answer = overflow.value.answers[0]?.answer;
      assert.equal(answer?.type, "custom");
      if (answer?.type === "custom") {
        assert.equal(answer.value.answer, "x".repeat(4096));
        assert.notDeepEqual(overflow.value.answers, []);
      }
    }
  });

  it("keeps maximum questionnaire content bounded and keeps the focused row visible", () => {
    const labels = ["A", "B", "C", "D"].map((prefix) => `${prefix}${"l".repeat(59)}`);
    const specification = singleQuestionnaire({
      question: "q".repeat(4096),
      header: "Maximum",
      answer: {
        type: "single_choice",
        options: labels.map((label, index) => ({
          label,
          description: `${index}${"d".repeat(1023)}`,
          ...(index === 0 ? { preview: "preview-000 " + "p".repeat(8192) } : {}),
        })),
        allow_custom: true,
      },
    });
    const view = singleOverlay(specification, () => {});
    view.setBodyHeight(16);

    for (let row = 0; row <= labels.length; row += 1) {
      const lines = view.render(56);
      assertBounded(lines, 56, 16);
      const focusedLines = lines.filter((line) => plainText(line).includes("›"));
      assert.equal(focusedLines.length, 1, "exactly one focused row is visible");
      if (row < labels.length) {
        assert.ok(plainText(focusedLines[0]!).includes(`${labels[row]![0]}lll`));
      }
      else assert.match(plainText(focusedLines[0]!), /Type something\./);
      if (row < labels.length) view.handleInput("\u001b[B");
    }

    view.handleInput("\t");
    const review = view.render(56);
    assertBounded(review, 56, 16);
    assert.match(plainText(review.join("\n")), /Review \/ submit/);
    assert.match(plainText(review.join("\n")), /Submit/);
  });

  it("supports bounded previews in both layouts and exposes later preview lines", () => {
    const preview = Array.from(
      { length: 400 },
      (_, index) => `preview-${String(index).padStart(3, "0")} ${"x".repeat(20)}`,
    ).join("\n").slice(0, 8192);
    const specification = singleQuestionnaire({
      question: "Which preview?",
      header: "Preview",
      answer: {
        type: "single_choice",
        options: [
        { label: "First", description: "First option.", preview },
        { label: "Second", description: "Second option." },
      ],
        allow_custom: true,
      },
    });
    const view = singleOverlay(specification, () => {});
    view.setBodyHeight(14);

    const narrow = view.render(56);
    assertBounded(narrow, 56, 14);
    assert.match(plainText(narrow.join("\n")), /Preview/);
    const wide = view.render(120);
    assertBounded(wide, 120, 14);
    assert.match(plainText(wide.join("\n")), /preview-000/);

    view.handleInput("\u001b[6~");
    const later = view.render(120);
    assertBounded(later, 120, 14);
    assert.notEqual(plainText(later.join("\n")), plainText(wide.join("\n")));
    assert.match(plainText(later.join("\n")), /preview-01[0-9]/);
  });

  it("keeps focus visible while independently paging previews in both layouts", () => {
    const preview = Array.from(
      { length: 400 },
      (_, index) => `preview-${String(index).padStart(3, "0")} ${"x".repeat(20)}`,
    ).join("\n").slice(0, 8192);

    for (const width of [120, 56]) {
      const view = singleOverlay(
        singleQuestionnaire({
          question: "Which preview should be inspected?",
          header: "Preview",
          answer: {
            type: "single_choice",
            options: [
            { label: "First option", description: "The initially focused option.", preview },
            { label: "Second option", description: "Another option with its own preview.", preview },
          ],
            allow_custom: true,
          },
        }),
        () => {},
      );
      view.setBodyHeight(14);

      const initial = view.render(width);
      assertBounded(initial, width, 14);
      assert.equal(
        initial.filter((line) => plainText(line).includes("›")).length,
        1,
      );
      assert.match(plainText(initial.join("\n")), /› .*First option/);
      assert.match(plainText(initial.join("\n")), /preview-000/);

      view.handleInput("\u001b[6~");
      const paged = view.render(width);
      assertBounded(paged, width, 14);
      assert.notDeepEqual(paged, initial, `PageDown should advance preview at width ${width}`);
      const pagedFocus = paged.filter((line) => plainText(line).includes("›"));
      assert.equal(pagedFocus.length, 1, "PageDown must not hide the focused row");
      assert.match(plainText(pagedFocus[0]!), /First option/);

      for (let page = 0; page < 100; page += 1) view.handleInput("\u001b[6~");
      const end = view.render(width);
      assertBounded(end, width, 14);
      assert.match(plainText(end.join("\n")), /lines \d+-\d+ of \d+/);
      assert.deepEqual(view.render(width), end, "PageDown at the end must be bounded");

      for (let page = 0; page < 100; page += 1) view.handleInput("\u001b[5~");
      const beginning = view.render(width);
      assertBounded(beginning, width, 14);
      assert.match(plainText(beginning.join("\n")), /preview-000/);

      view.handleInput("\u001b[6~");
      view.handleInput("\u001b[B");
      const secondOption = view.render(width);
      assertBounded(secondOption, width, 14);
      const secondFocus = secondOption.filter((line) => plainText(line).includes("›"));
      assert.equal(secondFocus.length, 1, "the newly focused row must remain visible");
      assert.match(plainText(secondFocus[0]!), /Second option/);
      assert.match(
        plainText(secondOption.join("\n")),
        /preview-000/,
        "moving focus must reset preview scrolling for the new option",
      );

      view.handleInput("\u001b[6~");
      view.handleInput("\t");
      const review = view.render(width);
      assertBounded(review, width, 14);
      assert.match(plainText(review.join("\n")), /Review \/ submit/);
      assert.match(plainText(review.join("\n")), /› .*Submit/);
    }
  });

  it("makes every numbered preview line reachable without page gaps", () => {
    const preview = Array.from(
      { length: 100 },
      (_, index) => `preview-${String(index).padStart(3, "0")}`,
    ).join("\n");

    for (const width of [120, 56]) {
      const view = singleOverlay(
        singleQuestionnaire({
          question: "Which numbered preview should be inspected?",
          header: "Preview",
          answer: {
            type: "single_choice",
            options: [
            { label: "First option", description: "The initially focused option.", preview },
            { label: "Second option", description: "Another preview-bearing option.", preview },
          ],
            allow_custom: true,
          },
        }),
        () => {},
      );
      view.setBodyHeight(14);

      let current = previewPage(view, width);
      const visited = new Set<number>();
      for (let page = 0; page < 100; page += 1) {
        assertBounded(current.lines, width, 14);
        assertFocusedOption(current, "First option");
        current.numbers.forEach((number) => visited.add(number));

        const before = current.lines;
        view.handleInput("\u001b[6~");
        const next = previewPage(view, width);
        if (next.lines.every((line, index) => line === before[index]) && next.lines.length === before.length) {
          break;
        }
        assert.ok(next.first > current.first, "PageDown must advance until the end");
        assert.ok(
          next.first <= current.last + 1,
          "PageDown must not skip a preview line between pages",
        );
        current = next;
      }

      assert.equal(current.numbers.at(-1), 99, "the final preview line must be visible");
      assert.equal(visited.size, 100, "the union of visited pages must contain every line");
      for (let number = 0; number < 100; number += 1) {
        assert.ok(visited.has(number), `preview-${String(number).padStart(3, "0")} must be reachable`);
      }

      const atEnd = view.render(width);
      view.handleInput("\u001b[6~");
      assert.deepEqual(view.render(width), atEnd, "PageDown at the end must be idempotent");

      for (let page = 0; page < 100; page += 1) {
        const before = previewPage(view, width);
        view.handleInput("\u001b[5~");
        const previous = previewPage(view, width);
        if (previous.lines.every((line, index) => line === before.lines[index]) && previous.lines.length === before.lines.length) {
          break;
        }
        assert.ok(
          previous.last + 1 >= before.first,
          "PageUp must not skip backward over a preview line",
        );
        assert.ok(previous.first < before.first, "PageUp must move toward the beginning");
        current = previous;
      }
      assert.equal(current.numbers[0], 0, "repeated PageUp must return to the first line");
      const atBeginning = view.render(width);
      view.handleInput("\u001b[5~");
      assert.deepEqual(view.render(width), atBeginning, "PageUp at the beginning must be idempotent");
    }
  });

  it("accounts for a wrapped wide intro and reconciles preview paging across resize", () => {
    const preview = Array.from(
      { length: 100 },
      (_, index) => `preview-${String(index).padStart(3, "0")}`,
    ).join("\n");
    const view = singleOverlay(
      singleQuestionnaire({
        question: ("A long wrapped question intro ".repeat(200)).slice(0, 4096),
        header: "Preview",
        answer: {
          type: "single_choice",
          options: [
          { label: "First option", description: "The initially focused option.", preview },
          { label: "Second option", description: "Another preview-bearing option.", preview },
        ],
          allow_custom: true,
        },
      }),
      () => {},
    );
    view.setBodyHeight(14);

    const initialWide = previewPage(view, 120);
    assertBounded(initialWide.lines, 120, 14);
    assertFocusedOption(initialWide, "First option");
    view.handleInput("\u001b[6~");
    const pagedWide = previewPage(view, 120);
    assertBounded(pagedWide.lines, 120, 14);
    assertFocusedOption(pagedWide, "First option");

    const pagedNarrow = previewPage(view, 56);
    assertBounded(pagedNarrow.lines, 56, 14);
    assertFocusedOption(pagedNarrow, "First option");
    view.handleInput("\u001b[5~");
    const backWide = previewPage(view, 120);
    assertBounded(backWide.lines, 120, 14);
    assertFocusedOption(backWide, "First option");
    assert.equal(backWide.numbers[0], 0, "PageUp after resizing must reach the first line");

    view.handleInput("\u001b[6~");
    view.handleInput("\u001b[B");
    const second = previewPage(view, 120);
    assertBounded(second.lines, 120, 14);
    assertFocusedOption(second, "Second option");
    assert.equal(second.numbers[0], 0, "changing focus must reset preview paging");

    view.handleInput("\t");
    const review = view.render(120).map(plainText);
    assertBounded(review, 120, 14);
    assert.ok(review.some((line) => line.includes("›") && line.includes("Submit")));
  });
  // -------------------------------------------------------------------------
  // The typed question vocabulary (Issue #242)
  // -------------------------------------------------------------------------

  function typedOverlay(
    specification: QuestionnaireSpecification,
    requester: InteractionRequester = NATIVE_REQUESTER,
    onSubmit: (response: QuestionnaireResponse) => void = () => {},
  ): QuestionnaireOverlay {
    return new QuestionnaireOverlay({
      interactionId: "interaction-typed",
      questionnaire: specification,
      requester,
      onSubmit,
      onDecline: () => {},
      onInterrupt: () => {},
    });
  }

  function type(view: QuestionnaireOverlay, value: string): void {
    for (const scalar of value) view.handleInput(scalar);
  }

  it("names the MCP server that asked, and never labels a native prompt as MCP", () => {
    const mcp = typedOverlay(
      { questions: [{ question: "Which channel?", header: "Channel", answer: { type: "boolean" } }] },
      MCP_REQUESTER,
    );
    const rendered = plainText(mcp.render(80).join("\n"));
    assert.match(rendered, /Requested by MCP server: github/);
    assert.match(rendered, /Tool: create_issue/);
    assert.equal(mcp.popupTitle(), "MCP elicitation · github");

    const native = typedOverlay(
      { questions: [{ question: "Which channel?", header: "Channel", answer: { type: "boolean" } }] },
    );
    const nativeRendered = plainText(native.render(80).join("\n"));
    assert.doesNotMatch(nativeRendered, /MCP/);
    assert.match(nativeRendered, /Requested by ask_user/);
    assert.equal(native.popupTitle(), "Ask user · questionnaire");
  });

  it("renders a text question as an input field with no manufactured options", () => {
    let submitted: QuestionnaireResponse | undefined;
    const view = typedOverlay(
      {
        questions: [{
          question: "What is your GitHub username?",
          header: "Operator",
          answer: { type: "text", min_length: 1, max_length: 39 },
        }],
      },
      MCP_REQUESTER,
      (response) => {
        submitted = response;
      },
    );
    const rendered = plainText(view.render(80).join("\n"));
    assert.doesNotMatch(rendered, /Type something\./);
    assert.match(rendered, /Type your answer\./);
    assert.match(rendered, /1–39 characters/);

    type(view, "octocat");
    submitSingle(view);
    assert.deepEqual(submitted, {
      type: "submitted",
      value: {
        answers: [{ question_index: 0, answer: { type: "text", value: { value: "octocat" } } }],
      },
    });
  });

  it("keeps an invalid numeric edit inside the interaction and submits a typed value", () => {
    const integerQuestionnaire: QuestionnaireSpecification = {
      questions: [{
        question: "How many attempts?",
        header: "Attempts",
        answer: { type: "integer", minimum: 1, maximum: 5 },
      }],
    };
    // A fractional value is not an integer: the surface says so concisely and
    // refuses to submit, keeping the user inside the interaction. Nothing here
    // can fail the enclosing MCP invocation.
    let submitted: QuestionnaireResponse | undefined;
    const fractional = typedOverlay(integerQuestionnaire, MCP_REQUESTER, (response) => {
      submitted = response;
    });
    type(fractional, "1.5");
    assert.match(plainText(fractional.render(80).join("\n")), /Enter a whole number\./);
    submitSingle(fractional);
    assert.equal(submitted, undefined, "an invalid draft never submits");
    // The blocked submission returns focus to the offending question and names
    // it on the review surface.
    fractional.handleInput("\t");
    assert.match(
      plainText(fractional.render(80).join("\n")),
      /Correct Attempts before submitting\./,
    );

    // Out of range is refused too, with the request's own bound explained.
    const outOfRange = typedOverlay(integerQuestionnaire, MCP_REQUESTER, () => {
      assert.fail("an out-of-range draft never submits");
    });
    type(outOfRange, "9");
    assert.match(plainText(outOfRange.render(80).join("\n")), /at most 5/);
    submitSingle(outOfRange);

    let valid: QuestionnaireResponse | undefined;
    const view = typedOverlay(integerQuestionnaire, MCP_REQUESTER, (response) => {
      valid = response;
    });
    type(view, "3");
    submitSingle(view);
    assert.deepEqual(valid, {
      type: "submitted",
      value: {
        answers: [{ question_index: 0, answer: { type: "integer", value: { value: 3 } } }],
      },
    });
  });

  it("submits a boolean as true/false while displaying Yes/No", () => {
    for (const [row, value] of [[0, true], [1, false]] as const) {
      let submitted: QuestionnaireResponse | undefined;
      const view = typedOverlay(
        { questions: [{ question: "Notify?", header: "Notify", answer: { type: "boolean" } }] },
        MCP_REQUESTER,
        (response) => {
          submitted = response;
        },
      );
      const rendered = plainText(view.render(80).join("\n"));
      assert.match(rendered, /Yes/);
      assert.match(rendered, /No/);
      for (let index = 0; index < row; index += 1) view.handleInput("\u001b[B");
      view.handleInput("\r");
      submitSingle(view);
      assert.deepEqual(submitted, {
        type: "submitted",
        value: {
          answers: [{ question_index: 0, answer: { type: "boolean", value: { value } } }],
        },
      });
    }
  });

  it("offers no custom-answer row for a bounded MCP choice", () => {
    const view = typedOverlay(
      {
        questions: [{
          question: "Which channel?",
          header: "Channel",
          answer: {
            type: "single_choice",
            options: [
              { label: "stable", description: "MCP value: \"stable\"" },
              { label: "beta", description: "MCP value: \"beta\"" },
            ],
            allow_custom: false,
          },
        }],
      },
      MCP_REQUESTER,
    );
    const rendered = plainText(view.render(80).join("\n"));
    assert.doesNotMatch(rendered, /Type something\./);
    // Only the two declared rows exist: moving down twice cannot leave them.
    view.handleInput("\u001b[B");
    view.handleInput("\u001b[B");
    const focused = view.render(80).filter((line) => plainText(line).includes("›"));
    assert.equal(focused.length, 1);
    assert.match(plainText(focused[0]!), /beta/);
  });

  it("shows multi-select bounds, refuses to exceed them, and blocks a short submission", () => {
    let submitted: QuestionnaireResponse | undefined;
    const view = typedOverlay(
      {
        questions: [{
          question: "Which regions?",
          header: "Regions",
          answer: {
            type: "multi_choice",
            options: [
              { label: "eu", description: "Europe." },
              { label: "us", description: "Americas." },
              { label: "ap", description: "Asia-Pacific." },
            ],
            min_selected: 2,
            max_selected: 2,
            allow_custom: false,
          },
        }],
      },
      MCP_REQUESTER,
      (response) => {
        submitted = response;
      },
    );
    assert.match(plainText(view.render(80).join("\n")), /Select exactly 2/);

    // One selection is below the declared minimum: submission is blocked.
    view.handleInput(" ");
    submitSingle(view);
    assert.equal(submitted, undefined);

    // A third selection is refused client-side rather than silently sent.
    view.handleInput("\u001b[B");
    view.handleInput(" ");
    view.handleInput("\u001b[B");
    view.handleInput(" ");
    submitSingle(view);
    assert.deepEqual(submitted, {
      type: "submitted",
      value: {
        answers: [{
          question_index: 0,
          answer: { type: "options", value: { option_indices: [0, 1] } },
        }],
      },
    });
  });
});
