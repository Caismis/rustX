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
 *
 * # Order matters more than the rules do
 *
 * Untrusted text is reduced **before** it is styled, never after. Once a row
 * has been assembled, an `ESC` the theme emitted and an `ESC` that arrived in
 * a tool argument are the same bytes in the same string, and no rule applied
 * to the finished line can recover which was which — a filter that keeps
 * "the client's own colours" at that point keeps the model's too. So each of
 * the two entry points below runs where provenance is still known:
 * {@link sanitizeField} on one externally-derived field, and
 * {@link sanitizeData} on a whole externally-derived value. What runs on the
 * assembled line, {@link sanitizeLine}, is a layout backstop and nothing
 * more.
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
 * Externally-derived data, reduced to what a renderer may be handed.
 *
 * The companion of {@link sanitizeField} for the tool bands, where the
 * untrusted thing is not one field but a whole published value: parsed call
 * arguments, a committed result, the JSON a tool returned. Every string in
 * the tree — object keys included, since a key is drawn as literally as a
 * value — is reduced; numbers, booleans, and nulls are returned as they are.
 *
 * Two deliberate differences from {@link sanitizeField}:
 *
 * - line breaks survive, because these strings are *split* into rows by the
 *   renderers rather than drawn into one, and a body that lost its line
 *   breaks would be drawn as one enormous row;
 * - offending characters are dropped rather than marked, because this tree
 *   carries arbitrary tool output, where one `U+FFFD` per stray byte is
 *   noise rather than information.
 *
 * Cycles are not defended against: every caller passes a value that came out
 * of `JSON.parse` or off the wire, which is a tree.
 */
export function sanitizeData(value: unknown): unknown {
  if (typeof value === "string") {
    return value.replace(FORBIDDEN_MULTILINE, "");
  }
  if (Array.isArray(value)) {
    return value.map(sanitizeData);
  }
  if (typeof value === "object" && value !== null) {
    return Object.fromEntries(
      Object.entries(value).map(([key, entry]) => [
        key.replace(FORBIDDEN_MULTILINE, ""),
        sanitizeData(entry),
      ]),
    );
  }
  return value;
}

/**
 * One assembled line of output, reduced to what this client may print.
 *
 * This runs *after* styling, which is the one thing it cannot undo: by the
 * time a line is assembled, an `ESC` that arrived in content and an `ESC`
 * this client's theme emitted are the same bytes, and no rule stated here
 * can tell them apart. So this is **not** where content is made safe — it is
 * the layout backstop that keeps one line one line, and it preserves SGR
 * only because provenance was already settled upstream: every untrusted
 * fragment passes {@link sanitizeField} or {@link sanitizeData} *before* it
 * is styled, so the only `ESC` that can still reach here is one the theme
 * wrote.
 *
 * Offending characters are dropped rather than marked, because a line may
 * carry arbitrary tool output where one mark per stray byte is noise rather
 * than information.
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
