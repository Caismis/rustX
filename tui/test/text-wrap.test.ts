/**
 * The lossless hard wrap behind the expanded Approval detail (Issue #185).
 *
 * Expanded Approval is an authorization surface over the runtime's published
 * reason and arguments, so its row generation is held to a reconstruction
 * contract, not to a "looks reasonable" contract: width may only decide where
 * the visual row boundaries fall. These tests prove that property directly on
 * the helper — `rows.join("") === input` for every case — instead of
 * inferring it from a handful of reachable tail markers.
 */

import assert from "node:assert/strict";
import { describe, it } from "node:test";

import { visibleWidth } from "@earendil-works/pi-tui";

import { hardWrapLossless } from "../src/ui/text-wrap.ts";

/** Widths that put a boundary on, before, and after any given column. */
const WIDTHS = [1, 2, 3, 5, 8, 13, 21, 40];

const PLAIN_LINES = [
  "a",
  "foo bar baz",
  "alpha  beta",
  "  leading and trailing  ",
  "BEGIN-" + "x".repeat(97) + "-END-OF-AUTHORITATIVE-PAYLOAD",
  '  "command": "prefix foo bar suffix"',
  "tab\tseparated\tvalue",
];

const WIDE_LINES = [
  "認証トークン",
  "approve 🔒 the 🚀 launch",
  "composed é combining mark",
  "mixed abc認証🔒def",
];

describe("hardWrapLossless", () => {
  it("reconstructs every logical line exactly, at every width", () => {
    for (const line of [...PLAIN_LINES, ...WIDE_LINES]) {
      for (const width of WIDTHS) {
        const rows = hardWrapLossless(line, width);
        assert.equal(
          rows.join(""),
          line,
          `rows of ${JSON.stringify(line)} at width ${width} reconstruct the input`,
        );
      }
    }
  });

  it("keeps every row within the requested terminal width", () => {
    // Width 1 cannot hold a two-column grapheme at all, so the width bound is
    // checked where the terminal could actually render the content. A tab is
    // a single width-3 grapheme in pi-tui's width model, so the tabbed line
    // is checked where three columns exist.
    for (const line of PLAIN_LINES) {
      const minimum = line.includes("\t") ? 3 : 1;
      for (const width of WIDTHS.filter((w) => w >= minimum)) {
        for (const row of hardWrapLossless(line, width)) {
          assert.ok(
            visibleWidth(row) <= width,
            `row ${JSON.stringify(row)} fits width ${width}`,
          );
        }
      }
    }
    for (const line of WIDE_LINES) {
      for (const width of WIDTHS.filter((w) => w >= 2)) {
        for (const row of hardWrapLossless(line, width)) {
          assert.ok(
            visibleWidth(row) <= width,
            `row ${JSON.stringify(row)} fits width ${width}`,
          );
        }
      }
    }
  });

  it("wraps an empty logical line to one empty row", () => {
    assert.deepEqual(hardWrapLossless("", 10), [""]);
  });

  it("preserves a semantic space exactly on the wrap boundary", () => {
    // The space between "prefix" and "foo" is column 20: the last column of
    // the first row at width 21. A prose wrapper trims it; this wrap must not.
    const line = '  "command": "prefix foo bar suffix"';
    assert.deepEqual(hardWrapLossless(line, 21), [
      '  "command": "prefix ',
      'foo bar suffix"',
    ]);
  });

  it("preserves consecutive spaces split across a wrap boundary", () => {
    // The two spaces between "alpha" and "beta" are columns 17 and 18: one
    // ends the first row, the other opens the second, at width 18.
    const line = '  "value": "alpha  beta"';
    const rows = hardWrapLossless(line, 18);
    assert.deepEqual(rows, ['  "value": "alpha ', ' beta"']);
    assert.equal(rows.join(""), line);
  });

  it("preserves significant leading and trailing whitespace inside a value", () => {
    const line = '  "value": "  payload tail  "';
    for (const width of WIDTHS) {
      assert.equal(hardWrapLossless(line, width).join(""), line);
    }
    // Width 27 puts both trailing spaces of the value on row boundaries.
    assert.deepEqual(hardWrapLossless(line, 27), [
      '  "value": "  payload tail ',
      ' "',
    ]);
  });

  it("moves a wide grapheme whole instead of splitting it across rows", () => {
    // At width 4 the two-column 認 would straddle the first row's boundary
    // (3 + 2 > 4); it must move to the next row intact, leaving the boundary
    // column empty rather than half-rendered.
    const line = "abc認証";
    const rows = hardWrapLossless(line, 4);
    assert.deepEqual(rows, ["abc", "認証"]);
    assert.equal(rows.join(""), line);
    for (const row of rows) {
      assert.ok(visibleWidth(row) <= 4);
    }
  });

  it("never slices an escape sequence handed in with styled input", () => {
    const line = "\u001b[31mred alarm\u001b[39m plain tail";
    for (const width of [3, 7, 12]) {
      const rows = hardWrapLossless(line, width);
      assert.equal(rows.join(""), line, "styling survives reconstruction");
      for (const row of rows) {
        // No row may hold a partial sequence: stripping complete SGR codes
        // must leave no ESC byte behind.
        const withoutCodes = row.replace(/\u001b\[[0-9;]*m/g, "");
        assert.ok(
          !withoutCodes.includes("\u001b"),
          `row ${JSON.stringify(row)} holds no partial escape`,
        );
        assert.ok(visibleWidth(row) <= width);
      }
    }
  });
});
