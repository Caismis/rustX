import assert from "node:assert/strict";
import { describe, it } from "node:test";

import { visibleWidth } from "@earendil-works/pi-tui";

import { InspectionView } from "../src/ui/components/inspection-view.ts";
import { plainText } from "../src/ui/theme.ts";

function plain(lines: string[]): string {
  return lines.map(plainText).join("\n");
}

describe("InspectionView", () => {
  it("keeps long Markdown content in a bounded, scrollable viewport", () => {
    const body = Array.from({ length: 12 }, (_, index) => `- marker-${index}`).join("\n");
    const view = new InspectionView({
      title: "Inspection",
      body,
      viewportLines: 3,
    });

    const initial = view.render(32);
    assert.ok(initial.length <= 4, "the range indicator and the body viewport stay bounded");
    assert.ok(initial.every((line) => visibleWidth(line) <= 32));
    assert.match(plain(initial), /marker-0/);
    assert.doesNotMatch(plain(initial), /marker-11/);

    view.handleInput("\x1b[B");
    const shifted = plain(view.render(32));
    assert.equal(view.offset, 1);
    assert.notEqual(shifted, plain(initial));

    view.handleInput("\x1b[6~");
    assert.ok(view.offset > 1, "PageDown reaches content outside the first viewport");
    const end = plain(view.render(32));
    assert.match(end, /marker-4/);
    assert.doesNotMatch(end, /marker-0/);

    view.handleInput("\x1b[H");
    assert.equal(view.offset, 0, "Home returns to the first body line");
    view.handleInput("\x1b[F");
    assert.equal(view.offset, view.bodyLineCount - 3, "End reaches the last viewport");
  });

  it("reports Escape to its owner instead of handling cancellation semantics", () => {
    const view = new InspectionView({
      title: "Inspection",
      body: "read-only details",
      viewportLines: 2,
    });
    let closed = 0;
    view.onClose = () => {
      closed += 1;
    };

    view.handleInput("\u001b");

    assert.equal(closed, 1);
  });
});
