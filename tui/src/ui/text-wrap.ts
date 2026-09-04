/**
 * Lossless hard wrap for authorization disclosure.
 *
 * pi-tui's `wrapTextWithAnsi` is a *prose* wrapper: it trims trailing
 * whitespace off every wrapped row and refuses to start a row with
 * whitespace. That is the right shape for flowing text and the wrong shape
 * for the expanded Approval detail, which is an authorization surface over
 * the runtime's published reason and arguments: a space that happens to fall
 * on a wrap boundary is a semantic character of the value being authorized,
 * and a prose wrapper silently deletes it (flattening the rows turns
 * `"foo bar"` into `"foobar"`).
 *
 * {@link hardWrapLossless} is the opposite contract. Width decides *where
 * the visual row boundaries are* and never *what the content is*:
 *
 * - concatenating the rows in order reproduces the input exactly;
 * - no whitespace is trimmed, collapsed, relocated, or normalized;
 * - a grapheme cluster or wide character is never split — one grapheme wider
 *   than the whole width stays whole on its own row rather than being cut;
 * - an escape sequence is never sliced — it rides with the current row.
 *
 * The slicing reuses pi-tui's own primitives — its grapheme segmenter, its
 * terminal visible-width measure, and its escape-sequence recognizer — so
 * there is exactly one width model in the client and no hand-rolled Unicode
 * or ANSI handling. The Approval path wraps the *plain* authoritative text
 * and styles each visual row afterwards; the escape pass-through keeps the
 * contract total even if styled input is ever handed in.
 */
import {
  extractAnsiCode,
  getGraphemeSegmenter,
  visibleWidth,
  // The package root re-exports only the prose-oriented helpers; the
  // character-faithful primitives live in its utils module.
} from "@earendil-works/pi-tui/dist/utils.js";

/**
 * Slices one logical line into visual rows of at most `width` terminal
 * columns, inserting row boundaries only.
 *
 * `hardWrapLossless(line, width).join("") === line` for every input, and
 * every row's {@link visibleWidth} is at most `width` unless it carries a
 * single grapheme wider than `width` itself (which no terminal could render
 * in the space either). An empty line wraps to one empty row, preserving the
 * layout invariant that a logical line always owns at least one visual row.
 */
export function hardWrapLossless(line: string, width: number): string[] {
  const columns = Math.max(1, Math.floor(width));
  if (line === "") {
    return [""];
  }
  const segmenter = getGraphemeSegmenter();
  const rows: string[] = [];
  let row = "";
  let rowWidth = 0;
  let index = 0;
  while (index < line.length) {
    const ansi = extractAnsiCode(line, index);
    if (ansi !== null) {
      row += ansi.code;
      index += ansi.length;
      continue;
    }
    // Segment between escape sequences; a control character always terminates
    // a grapheme cluster, so slicing the text runs cannot split one.
    let textEnd = index;
    while (textEnd < line.length && extractAnsiCode(line, textEnd) === null) {
      textEnd += 1;
    }
    for (const { segment } of segmenter.segment(line.slice(index, textEnd))) {
      const segmentWidth = visibleWidth(segment);
      if (rowWidth > 0 && rowWidth + segmentWidth > columns) {
        rows.push(row);
        row = "";
        rowWidth = 0;
      }
      row += segment;
      rowWidth += segmentWidth;
    }
    index = textEnd;
  }
  rows.push(row);
  return rows;
}
