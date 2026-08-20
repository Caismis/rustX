/**
 * The rustX terminal palette, organized by *semantic role*.
 *
 * Two layers, deliberately:
 *
 * ```text
 * style   raw SGR wrappers          (what a colour is)
 * role    presentation vocabulary   (what a colour means)
 * ```
 *
 * Components reference roles, never raw colours, so "denied" and "cancelled"
 * look the same everywhere and a specialized tool renderer cannot invent its
 * own colour language. There is no per-tool palette: a tool may choose a
 * *renderer*, it may not choose a colour system.
 *
 * This is still not a theme system — Issue #39 lists configurable theming as
 * a non-goal, and nothing here has a configuration surface.
 */

const wrap = (open: string) => (text: string) => `[${open}m${text}[0m`;

export const style = {
  dim: wrap("2"),
  bold: wrap("1"),
  italic: wrap("3"),
  red: wrap("31"),
  green: wrap("32"),
  yellow: wrap("33"),
  blue: wrap("34"),
  magenta: wrap("35"),
  cyan: wrap("36"),
  grey: wrap("90"),
  plain: (text: string) => text,
};

/**
 * The semantic presentation roles.
 *
 * Every visible component picks from this vocabulary. Adding a role is a
 * presentation decision; adding a *meaning* is not — a role must correspond
 * to something the runtime already distinguishes.
 */
export const role = {
  /** The assistant's visible answer: primary content, unstyled by default. */
  assistant: style.plain,
  /** Model reasoning: present, readable, and clearly secondary to the answer. */
  reasoning: style.grey,
  /** A human or runtime-originated inbound turn. */
  user: style.cyan,
  /** The current selection or an interactive affordance. */
  accent: style.cyan,
  /** Work the runtime says is in flight. */
  pending: style.yellow,
  /** A runtime-published success. */
  success: style.green,
  /** A runtime-published refusal, denial, cancellation, or timeout. */
  warning: style.yellow,
  /** A runtime-published failure. */
  error: style.red,
  /** A tool card's title. */
  toolTitle: style.magenta,
  /** Verbatim tool output. */
  toolOutput: style.plain,
  /** Identities, counts, durations, hints — anything supporting. */
  meta: style.dim,
  /** Structural punctuation and separators. */
  chrome: style.grey,
  /** Emphasis inside an otherwise unstyled line. */
  strong: style.bold,
};

/** The markdown theme handed to Pi's renderer. */
export const markdownTheme = {
  heading: style.bold,
  link: style.cyan,
  linkUrl: style.dim,
  code: style.yellow,
  codeBlock: style.plain,
  codeBlockBorder: style.grey,
  quote: style.dim,
  quoteBorder: style.grey,
  hr: style.grey,
  listBullet: style.cyan,
  bold: style.bold,
  italic: style.italic,
  strikethrough: style.dim,
  underline: wrap("4"),
};

/** The select-list theme handed to Pi's overlay lists and the editor. */
export const selectListTheme = {
  selectedPrefix: style.cyan,
  selectedText: style.bold,
  description: style.dim,
  scrollInfo: style.grey,
  noMatch: style.dim,
};

export const editorTheme = {
  borderColor: style.grey,
  selectList: selectListTheme,
};

/** Strips SGR sequences. Width maths and tests both need the plain text. */
// eslint-disable-next-line no-control-regex
const SGR = /\[[0-9;]*m/g;

export function plainText(text: string): string {
  return text.replace(SGR, "");
}

/** The visible column count of a styled string, ignoring SGR sequences. */
export function plainWidth(text: string): number {
  return [...plainText(text)].length;
}
