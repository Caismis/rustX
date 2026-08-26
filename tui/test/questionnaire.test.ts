import assert from "node:assert/strict";
import { describe, it } from "node:test";

import { visibleWidth } from "@earendil-works/pi-tui";

import { QuestionnaireOverlay } from "../src/ui/components/questionnaire.ts";
import type { QuestionnaireResponse, QuestionnaireSpecification } from "../src/protocol/types.ts";
import { plainText } from "../src/ui/theme.ts";

function questionnaire(): QuestionnaireSpecification {
  return {
    questions: [
      {
        question: "Which visual direction should I use?",
        header: "Visual style",
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
        multi_select: false,
      },
      {
        question: "Which elements should be enabled?",
        header: "Elements",
        options: [
          { label: "Charts", description: "Show quantitative charts." },
          { label: "Comments", description: "Show reviewer comments." },
        ],
        multi_select: true,
      },
    ],
  };
}

function overlay(
  onSubmit: (response: QuestionnaireResponse) => void,
  onDecline: () => void = () => {},
  onInterrupt: () => void = () => {},
): QuestionnaireOverlay {
  return new QuestionnaireOverlay({
    interactionId: "attempt-1-interaction-questionnaire-1",
    questionnaire: questionnaire(),
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

describe("QuestionnaireOverlay", () => {
  it("preserves a single selection while switching tabs and submits in question order", () => {
    let submitted: QuestionnaireResponse | undefined;
    let changes = 0;
    const view = new QuestionnaireOverlay({
      interactionId: "interaction-1",
      questionnaire: questionnaire(),
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
          answer: { type: "single_option", value: { label: "Swiss / Klein blue" } },
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
            type: "multiple_option",
            value: { selected: ["Charts", "Comments"] },
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
      options: labels.map((label, index) => ({
        label,
        description: `${index}${"d".repeat(1023)}`,
        ...(index === 0 ? { preview: "preview-000 " + "p".repeat(8192) } : {}),
      })),
      multi_select: false,
    });
    const view = singleOverlay(specification, () => {});
    view.setViewportHeight(16);

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
      options: [
        { label: "First", description: "First option.", preview },
        { label: "Second", description: "Second option." },
      ],
      multi_select: false,
    });
    const view = singleOverlay(specification, () => {});
    view.setViewportHeight(14);

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
          options: [
            { label: "First option", description: "The initially focused option.", preview },
            { label: "Second option", description: "Another option with its own preview.", preview },
          ],
          multi_select: false,
        }),
        () => {},
      );
      view.setViewportHeight(14);

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
});
