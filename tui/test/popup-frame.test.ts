/**
 * The shared PopupFrame contract, proven structurally.
 *
 * Issue #161: every transient overlay surface — questionnaire, model picker,
 * session/tree/boundary pickers, and the read-only inspection dialogs such as
 * `/status` — is presented inside one boundary owned by PopupFrame. These
 * tests pin the frame's geometry (borders on all four sides, title in the
 * top edge, padding, footer containment, finite body allocation) and prove
 * each migrated surface honours it, including at constrained terminal sizes.
 */

import assert from "node:assert/strict";
import { describe, it } from "node:test";

import { CURSOR_MARKER, visibleWidth } from "@earendil-works/pi-tui";

import { InspectionView } from "../src/ui/components/inspection-view.ts";
import { ModelSelector } from "../src/ui/components/model-selector.ts";
import {
  PopupFrame,
  type PopupContent,
} from "../src/ui/components/popup-frame.ts";
import { QuestionnaireOverlay } from "../src/ui/components/questionnaire.ts";
import { SessionSelector } from "../src/ui/components/session-selector.ts";
import type { QuestionnaireSpecification } from "../src/protocol/types.ts";
import { background, plainText, style } from "../src/ui/theme.ts";
import { catalogModel, sessionModel } from "./support/fixtures.ts";

/** A minimal PopupContent double with recording hooks. */
class StubContent implements PopupContent {
  focused = false;
  invalidated = 0;
  received: string[] = [];
  bodyHeights: number[] = [];
  widths: number[] = [];

  readonly body: string[];
  readonly title: string;
  readonly footer: string[];

  constructor(
    body: string[],
    title = "Stub popup",
    footer = ["↑↓ move · Enter ok · Esc close"],
  ) {
    this.body = body;
    this.title = title;
    this.footer = footer;
  }

  popupTitle(): string {
    return this.title;
  }

  popupFooter(): string[] {
    return this.footer;
  }

  setBodyHeight(height: number): void {
    this.bodyHeights.push(height);
  }

  handleInput(data: string): void {
    this.received.push(data);
  }

  invalidate(): void {
    this.invalidated += 1;
  }

  render(width: number): string[] {
    this.widths.push(width);
    return this.body;
  }
}

function framed(
  content: PopupContent,
  width: number,
  budget: number,
): string[] {
  const frame = new PopupFrame(content);
  frame.setViewportHeight(budget);
  return frame.render(width);
}

/**
 * The containment invariant: both horizontal borders enclose every interior
 * row, the top and bottom edges render, and no row exceeds the outer
 * rectangle in either dimension.
 */
function assertContained(lines: string[], width: number, budget: number): void {
  assert.ok(lines.length >= 2, "both horizontal borders render");
  assert.ok(
    lines.length <= budget,
    `the popup never exceeds its height budget: ${lines.length} > ${budget}`,
  );
  for (const line of lines) {
    assert.ok(
      visibleWidth(line) <= width,
      `a popup row escaped its width: ${JSON.stringify(plainText(line))}`,
    );
  }
  const plain = lines.map(plainText);
  assert.match(plain[0]!, /^╭/, "the top border renders");
  assert.ok(plain[0]!.endsWith("╮"), "the top border closes on the right");
  assert.match(plain.at(-1)!, /^╰─*╯$/, "the bottom border renders");
  for (const middle of plain.slice(1, -1)) {
    assert.ok(
      middle.startsWith("│") || middle.startsWith("├"),
      `the left border renders: ${JSON.stringify(middle)}`,
    );
    assert.ok(
      middle.endsWith("│") || middle.endsWith("┤"),
      `the right border renders: ${JSON.stringify(middle)}`,
    );
  }
}

/** The interior rows of a framed render, borders stripped, plain text. */
function interior(lines: string[]): string[] {
  return lines
    .map(plainText)
    .filter((line) => line.startsWith("│"))
    .map((line) => line.replace(/^│ ?/, "").replace(/ ?│$/, ""));
}

describe("PopupFrame geometry", () => {
  it("draws one boundary around the whole surface with the title in the top edge", () => {
    const lines = framed(new StubContent(["first body row", "second body row"]), 40, 20);
    assertContained(lines, 40, 20);

    const plain = lines.map(plainText);
    assert.match(plain[0]!, /^╭─ Stub popup ─+╮$/, "title integrated into the top border");

    // The inner rectangle excludes border cells and keeps at least one cell
    // of horizontal padding between frame and content.
    const bodyRow = plain.find((line) => line.includes("first body row"))!;
    assert.match(bodyRow, /^│ +first body row +│$/);

    // The footer sits inside the same boundary, below a separator that spans
    // edge to edge without overwriting it.
    const separator = plain.findIndex((line) => line.startsWith("├"));
    const help = plain.findIndex((line) => line.includes("↑↓ move"));
    assert.ok(separator > 0 && help === separator + 1, "footer below the separator");
    assert.match(plain[separator]!, /^├─+┤$/);
    assert.ok(help < plain.length - 1, "footer above the bottom border");
  });

  it("allocates the finite body rectangle after border, padding, and footer", () => {
    const content = new StubContent(["row"]);
    framed(content, 40, 20);
    // 20 budget − 2 borders − 2 vertical padding − 1 separator − 1 footer.
    assert.deepEqual(content.bodyHeights, [14]);
    // 40 outer − 2 borders − 2 horizontal padding.
    assert.deepEqual(content.widths, [36]);
  });

  it("hands the exact finite body rectangle to the content on every render pass", () => {
    // The strengthened PopupContent contract: there is one layout model, and
    // every body receives its finite allocation — after border, padding, and
    // footer — before every render.
    const content = new StubContent(["row"]);
    const frame = new PopupFrame(content);
    frame.setViewportHeight(20);
    frame.render(40);
    frame.setViewportHeight(12);
    frame.render(60);
    // 20 − 6 chrome = 14, then 12 − 6 = 6; widths 40 − 4 and 60 − 4.
    assert.deepEqual(content.bodyHeights, [14, 6]);
    assert.deepEqual(content.widths, [36, 56]);
  });

  it("never clips a body that honours its allocation", () => {
    // The frame's slice is defensive containment for a misbehaving body; a
    // conforming body's rows pass through whole.
    const probe = new StubContent(["row"]);
    const probeFrame = new PopupFrame(probe);
    probeFrame.setViewportHeight(20);
    probeFrame.render(40);
    const allocated = probe.bodyHeights[0]!;
    const conforming = new StubContent(
      Array.from({ length: allocated }, (_, index) => `row-${index}`),
    );
    const lines = framed(conforming, 40, 20).map(plainText).join("\n");
    assert.match(lines, new RegExp(`row-${allocated - 1}`));
  });

  it("clips an overlong body only as defensive containment", () => {
    const content = new StubContent(
      Array.from({ length: 100 }, (_, index) => `body-${index}`),
    );
    const lines = framed(content, 40, 12);
    assertContained(lines, 40, 12);
    const plain = lines.map(plainText).join("\n");
    assert.match(plain, /body-0/);
    assert.doesNotMatch(plain, /body-99/, "the tail is clipped, the border is not");
  });

  it("truncates overwide body lines instead of escaping the side borders", () => {
    const lines = framed(new StubContent(["x".repeat(200)]), 30, 10);
    assertContained(lines, 30, 10);
    const row = interior(lines).find((line) => line.includes("x"))!;
    assert.ok(row.includes("…"), "the line is truncated, not clipped raw");
  });

  it("fills the whole rectangle with the popup surface band", () => {
    const lines = framed(new StubContent(["row"]), 40, 12);
    for (const line of lines) {
      assert.ok(
        line.startsWith(background.popup.open),
        "every row opens the popup surface band",
      );
      assert.equal(visibleWidth(line), 40, "the band fills edge to edge");
    }
  });

  it("re-opens the surface band after a full reset inside the content", () => {
    // style.bold ends in a full SGR reset; the band must survive it so the
    // rest of the row — selected-row padding included — stays on the popup
    // surface.
    const lines = framed(new StubContent([style.bold("bold row")]), 40, 12);
    const row = lines.find((line) => line.includes("bold row"))!;
    assert.ok(row.includes(`[0m${background.popup.open}`));
  });

  it("truncates an overlong title inside the top border", () => {
    const content = new StubContent(["row"], "A title far too long to fit this popup");
    const lines = framed(content, 16, 10);
    assertContained(lines, 16, 10);
    const top = plainText(lines[0]!);
    assert.match(top, /^╭─ .+… ─+╮$/, "the title is truncated, the border intact");
  });

  it("omits the footer block when no footer is declared", () => {
    const content = new StubContent(["row"], "No footer", []);
    const lines = framed(content, 40, 12);
    assertContained(lines, 40, 12);
    assert.ok(
      !lines.map(plainText).some((line) => line.startsWith("├")),
      "no separator without a footer",
    );
    assert.deepEqual(content.bodyHeights, [8], "the body reclaims the footer rows");
  });
});

describe("PopupFrame at constrained terminal sizes", () => {
  it("drops vertical padding, then the footer, before touching the boundary", () => {
    const content = new StubContent(["row-1", "row-2", "row-3"]);
    const lines = framed(content, 40, 4);
    assertContained(lines, 40, 4);
    const plain = lines.map(plainText);
    assert.equal(plain.join("\n").includes("↑↓ move"), false, "the footer yields first");
    assert.match(plain.at(-1)!, /^╰─*╯$/, "the bottom border still renders");
    assert.deepEqual(content.bodyHeights, [2], "the body gets exactly the remaining rows");
  });

  it("renders only the boundary when the budget leaves no interior", () => {
    for (const budget of [1, 2]) {
      const lines = framed(new StubContent(["row"]), 40, budget);
      assert.ok(lines.length <= budget);
      assert.match(plainText(lines[0]!), /^╭/);
      if (budget === 2) {
        assert.match(plainText(lines[1]!), /^╰─*╯$/);
      }
    }
  });

  it("never produces invalid geometry on narrow widths", () => {
    for (const width of [1, 2, 3, 4, 5, 8]) {
      const lines = framed(new StubContent(["some body content"]), width, 10);
      assert.ok(lines.length >= 1);
      for (const line of lines) {
        assert.ok(
          visibleWidth(line) <= Math.max(1, width),
          `width ${width}: ${JSON.stringify(plainText(line))}`,
        );
      }
    }
  });

  it("keeps wide graphemes inside the right border", () => {
    const lines = framed(new StubContent(["emoji 😀 row"]), 30, 10);
    assertContained(lines, 30, 10);
    const row = lines.map(plainText).find((line) => line.includes("😀"))!;
    assert.ok(row.endsWith("│"), "the band padding is cell-accurate");
  });
});

describe("PopupFrame delegation", () => {
  it("routes focus, input, and invalidation to the wrapped component", () => {
    const content = new StubContent(["row"]);
    const frame = new PopupFrame(content);

    frame.focused = true;
    assert.equal(content.focused, true);
    assert.equal(frame.focused, true);

    frame.handleInput("x");
    assert.deepEqual(content.received, ["x"]);

    frame.invalidate();
    assert.equal(content.invalidated, 1);
  });

  it("preserves the cursor marker the wrapped input emits", () => {
    const lines = framed(new StubContent([`edit${CURSOR_MARKER}or`]), 40, 12);
    assert.ok(
      lines.some((line) => line.includes(CURSOR_MARKER)),
      "the hardware cursor survives framing",
    );
    assertContained(lines, 40, 12);
  });
});

// ---------------------------------------------------------------------------
// Representative feature surfaces inside the frame
// ---------------------------------------------------------------------------

function questionnaire(): QuestionnaireOverlay {
  const specification: QuestionnaireSpecification = {
    questions: [
      {
        question: "Which visual direction should I use?",
        header: "Visual style",
        options: [
          { label: "Swiss / Klein blue", description: "Information-first typography." },
          { label: "Electronic magazine", description: "A warmer editorial composition." },
        ],
        multi_select: false,
      },
    ],
  };
  return new QuestionnaireOverlay({
    interactionId: "interaction-1",
    questionnaire: specification,
    onSubmit: () => {},
    onDecline: () => {},
    onInterrupt: () => {},
  });
}

function modelSelector(): ModelSelector {
  return new ModelSelector({
    models: [catalogModel("alpha/model-a"), catalogModel("beta/model-b")],
    sessionModel: sessionModel("alpha/model-a"),
  });
}

describe("framed feature surfaces", () => {
  it("contains the questionnaire: title, tabs, focused row, and help", () => {
    const view = questionnaire();
    const lines = framed(view, 80, 30);
    assertContained(lines, 80, 30);

    const plain = lines.map(plainText);
    assert.match(plain[0]!, /^╭─ Ask user · questionnaire ─+╮$/);
    const rows = interior(lines);
    assert.ok(rows.some((line) => line.includes("Visual style")), "tabs inside the frame");
    assert.ok(
      rows.some((line) => line.includes("›") && line.includes("Swiss / Klein blue")),
      "the focused row renders inside the frame",
    );
    assert.ok(
      rows.some((line) => line.includes("Tab/Shift+Tab tabs")),
      "the help footer renders inside the frame",
    );
  });

  it("keeps the questionnaire contained at small sizes", () => {
    const view = questionnaire();
    for (const [width, budget] of [[24, 8], [40, 6], [16, 12]] as const) {
      assertContained(framed(view, width, budget), width, budget);
    }
  });

  it("contains the model selector: search, selected row, context, and help", () => {
    const selector = modelSelector();
    const frame = new PopupFrame(selector);
    frame.setViewportHeight(30);

    // Feature semantics still belong to the wrapped component through the
    // frame: navigation moves the highlight, Enter selects, Esc cancels.
    let selected: string | undefined;
    let cancelled = 0;
    selector.onSelect = (model) => {
      selected = model.model;
    };
    selector.onCancel = () => {
      cancelled += 1;
    };
    frame.handleInput("[B");
    const lines = frame.render(80);
    assertContained(lines, 80, 30);

    const plain = lines.map(plainText);
    assert.match(plain[0]!, /^╭─ Select model ─+╮$/);
    const rows = interior(lines);
    assert.ok(rows.some((line) => line.includes("Search:")), "search inside the frame");
    assert.ok(
      rows.some((line) => line.includes("❯") && line.includes("beta/model-b")),
      "the moved selection renders inside the frame",
    );
    assert.ok(
      rows.some((line) => line.includes("configured · effective")),
      "the context block renders inside the frame",
    );
    assert.ok(rows.some((line) => line.includes("↑↓ navigate")), "help inside the frame");

    frame.handleInput("\r");
    assert.equal(selected, "beta/model-b");
    frame.handleInput("");
    assert.equal(cancelled, 1);
  });

  it("keeps the model selector contained at small sizes", () => {
    for (const [width, budget] of [[20, 10], [12, 6], [60, 4]] as const) {
      assertContained(framed(modelSelector(), width, budget), width, budget);
    }
  });

  it("contains the status inspection dialog: title, range, body, and help", () => {
    // `/status` is presented through the shared inspection surface.
    const view = new InspectionView({
      title: "Runtime status",
      body: Array.from({ length: 30 }, (_, index) => `status line ${index}`).join("\n"),
    });
    const lines = framed(view, 60, 12);
    assertContained(lines, 60, 12);

    const plain = lines.map(plainText);
    assert.match(plain[0]!, /^╭─ Runtime status ─+╮$/);
    const rows = interior(lines);
    assert.ok(rows.some((line) => /lines 1-\d+ of 30/.test(line)), "range inside the frame");
    assert.ok(rows.some((line) => line.includes("↑↓ scroll")), "help inside the frame");
    assert.doesNotMatch(plain.join("\n"), /status line 29/, "the body is bounded by the frame");

    // Scrolling stays the feature's own state machine, inside the boundary.
    view.handleInput("[B");
    assertContained(framed(view, 60, 12), 60, 12);
    assert.equal(view.offset, 1);
  });

  it("keeps the status inspection dialog contained at small sizes", () => {
    const view = new InspectionView({ title: "Runtime status", body: "details" });
    for (const [width, budget] of [[18, 6], [10, 4], [30, 3]] as const) {
      assertContained(framed(view, width, budget), width, budget);
    }
  });

  it("contains the session picker: search, rows, and help", () => {
    const selector = new SessionSelector({
      sessions: [
        {
          id: "session-1",
          name: "current work",
          updated_at: "2026-08-21T00:00:00Z",
          active_node: "node-1",
          active: true,
        },
      ],
    });
    const lines = framed(selector, 60, 20);
    assertContained(lines, 60, 20);

    const plain = lines.map(plainText);
    assert.match(plain[0]!, /^╭─ Resume session ─+╮$/);
    const rows = interior(lines);
    assert.ok(rows.some((line) => line.includes("❯") && line.includes("current work")));
    assert.ok(rows.some((line) => line.includes("↑↓ navigate")));
  });
});
