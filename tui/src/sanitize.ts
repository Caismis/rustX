/**
 * The one place model-written text stops being arbitrary bytes.
 *
 * Two hazards live in the same character classes, and this module is where
 * both are answered for the whole client:
 *
 * - **layout**: a line break, a carriage return, or a tab makes one logical
 *   row occupy more — or less — than the one physical row the caller
 *   budgeted for it. A panel whose whole contract is that it is bounded
 *   stops being bounded;
 * - **control**: `ESC` introduces every CSI and OSC sequence, so text that
 *   carries one can repaint colours, move the cursor, clear the screen, or
 *   retitle the terminal window. The C1 range does the same in one byte, and
 *   the Unicode bidi overrides reverse the reading order of the text around
 *   them without changing a single code point of it.
 *
 * The runtime rejects these where a task is created, which is the right
 * place: a task that cannot be drawn should never be stored. This module is
 * the other half of that boundary, on the side that actually holds the
 * terminal — and it is not redundant, because a client draws things the
 * runtime never validated. A tool *call* is drawn from the model's own
 * arguments while the assistant message is still streaming, long before any
 * executor has looked at them, so a rejected call has already been rendered
 * by the time it is rejected.
 */

/** Every character no rendered text may carry. */
const FORBIDDEN = /[\u0000-\u001f\u007f-\u009f\u200e\u200f\u202a-\u202e\u2066-\u2069]/g;

/** The same set minus the line break, for text that is allowed to wrap. */
const FORBIDDEN_MULTILINE = /[\u0000-\u0009\u000b-\u001f\u007f-\u009f\u200e\u200f\u202a-\u202e\u2066-\u2069]/g;

/** The SGR sequences this client emits itself. */
const SGR = /\u001b\[[0-9;]*m/g;

/** The mark left where a character was removed. */
const REMOVED = "\ufffd";

/**
 * One field of externally-derived text, reduced to what a row can hold.
 *
 * Each offending character becomes U+FFFD rather than disappearing, so a
 * reader sees that something was removed instead of silently reading
 * doctored text. `multiline` keeps line breaks for the fields whose whole
 * purpose is prose.
 */
export function sanitizeField(value: string, multiline = false): string {
  return value.replace(multiline ? FORBIDDEN_MULTILINE : FORBIDDEN, REMOVED);
}

/**
 * One assembled line of output, reduced to what this client may print.
 *
 * Unlike {@link sanitizeField} this runs *after* styling, so the SGR
 * sequences the theme itself emitted are preserved and everything else that
 * begins with `ESC` is not: a renderer may colour a line, and no content it
 * embedded may move the cursor or retitle the window. Offending characters
 * are dropped rather than marked, because a line may carry arbitrary tool
 * output where one mark per stray byte is noise rather than information.
 *
 * The result contains no line break, so a caller that counted this as one
 * line is right.
 */
export function sanitizeLine(line: string): string {
  const parts: string[] = [];
  let index = 0;
  SGR.lastIndex = 0;
  for (let match = SGR.exec(line); match !== null; match = SGR.exec(line)) {
    parts.push(strip(line.slice(index, match.index)), match[0]);
    index = match.index + match[0].length;
  }
  parts.push(strip(line.slice(index)));
  return parts.join("");
}

function strip(value: string): string {
  return value.replace(FORBIDDEN, "");
}
