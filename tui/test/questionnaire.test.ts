import assert from "node:assert/strict";
import { describe, it } from "node:test";

import { QuestionnaireOverlay } from "../src/ui/components/questionnaire.ts";
import type { QuestionnaireResponse, QuestionnaireSpecification } from "../src/protocol/types.ts";

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
});
