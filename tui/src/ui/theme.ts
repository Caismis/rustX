/**
 * The rustX terminal palette, organized by *semantic role*.
 *
 * Three layers, deliberately:
 *
 * ```text
 * palette raw colour values          (which colour)
 * style   SGR wrappers               (what a colour is)
 * role    presentation vocabulary    (what a colour means)
 * ```
 *
 * Components reference roles, never raw colours, so "denied" and "cancelled"
 * look the same everywhere and a specialized tool renderer cannot invent its
 * own colour language. There is no per-tool palette: a tool may choose a
 * *renderer*, it may not choose a colour system.
 *
 * The colour values are Pi's `dark` theme, so a reader moving between Pi and
 * rustX reads the same visual grammar: the same message background, the same
 * three tool-state backgrounds, the same Markdown accents. This is still not
 * a theme system — Issue #39 lists configurable theming as a non-goal, and
 * nothing here has a configuration surface.
 *
 * Backgrounds are a *band* language, not a text-decoration one. A background
 * role names the whole visual block it fills — a user turn, a tool card in
 * one of its three runtime states — and is applied by the shell that lays the
 * block out, which is the only layer that knows the terminal width. Nothing
 * below composes a background into a string.
 */

// The width measure the band filler pads by must be the terminal's own cell
// accounting: code-point counts misjudge wide graphemes, and a line padded by
// the wrong measure either leaks past its rectangle or leaves the band short.
import { visibleWidth } from "@earendil-works/pi-tui";

const wrap = (open: string) => (text: string) => `[${open}m${text}[0m`;

// ---------------------------------------------------------------------------
// Colour values
// ---------------------------------------------------------------------------

/**
 * The raw palette. Pi's `dark` theme values, kept as one table so a colour
 * appears exactly once and every role below is a reference to it.
 */
export const palette = {
  cyan: "#00d7ff",
  blue: "#5f87ff",
  green: "#b5bd68",
  red: "#cc6666",
  yellow: "#ffff00",
  text: "#d4d4d4",
  gray: "#808080",
  dimGray: "#666666",
  darkGray: "#505050",
  accent: "#8abeb7",
  heading: "#f0c674",
  link: "#81a2be",
  /** The background of a human turn. */
  userMessageBg: "#343541",
  /** A tool call that has not settled. */
  toolPendingBg: "#282832",
  /** A tool call the runtime settled as a success. */
  toolSuccessBg: "#283228",
  /** A tool call the runtime settled as anything but a success. */
  toolErrorBg: "#3c2828",
  /** The raised surface of a modal/popup overlay above the transcript. */
  popupBg: "#2a2a3a",
} as const;

/**
 * Whether the terminal was told to accept 24-bit colour.
 *
 * A terminal that did not say so gets the nearest 256-colour index instead of
 * an escape sequence it would print as text. This is the one environment
 * question this module asks, and it is asked once.
 */
const truecolor =
  process.env["COLORTERM"] === "truecolor" || process.env["COLORTERM"] === "24bit";

function hexToRgb(hex: string): { r: number; g: number; b: number } {
  const cleaned = hex.replace("#", "");
  return {
    r: Number.parseInt(cleaned.slice(0, 2), 16),
    g: Number.parseInt(cleaned.slice(2, 4), 16),
    b: Number.parseInt(cleaned.slice(4, 6), 16),
  };
}

/** The 6×6×6 colour-cube channel values. */
const CUBE = [0, 95, 135, 175, 215, 255];
/** The 24-step grayscale ramp of indices 232–255. */
const GRAYS = Array.from({ length: 24 }, (_, index) => 8 + index * 10);

function closest(values: readonly number[], value: number): number {
  let best = 0;
  let bestDistance = Number.POSITIVE_INFINITY;
  for (const [index, candidate] of values.entries()) {
    const distance = Math.abs(value - candidate);
    if (distance < bestDistance) {
      bestDistance = distance;
      best = index;
    }
  }
  return best;
}

/** Perceptually weighted distance: the eye weights green most, blue least. */
function distance(
  r1: number,
  g1: number,
  b1: number,
  r2: number,
  g2: number,
  b2: number,
): number {
  return (
    (r1 - r2) ** 2 * 0.299 + (g1 - g2) ** 2 * 0.587 + (b1 - b2) ** 2 * 0.114
  );
}

/** The nearest 256-colour index, preferring the cube whenever hue matters. */
function rgbTo256(r: number, g: number, b: number): number {
  const rIndex = closest(CUBE, r);
  const gIndex = closest(CUBE, g);
  const bIndex = closest(CUBE, b);
  const cubeDistance = distance(
    r,
    g,
    b,
    CUBE[rIndex]!,
    CUBE[gIndex]!,
    CUBE[bIndex]!,
  );

  const luminance = Math.round(0.299 * r + 0.587 * g + 0.114 * b);
  const grayIndex = closest(GRAYS, luminance);
  const gray = GRAYS[grayIndex]!;
  const grayDistance = distance(r, g, b, gray, gray, gray);

  // A tinted colour keeps its tint even when a gray is numerically nearer:
  // losing the hue is more visible than losing a few units of luminance.
  const spread = Math.max(r, g, b) - Math.min(r, g, b);
  if (spread < 10 && grayDistance < cubeDistance) {
    return 232 + grayIndex;
  }
  return 16 + 36 * rIndex + 6 * gIndex + bIndex;
}

/**
 * A foreground colour wrapper.
 *
 * Only the foreground is reset, so colouring a fragment inside a background
 * band never punches a hole in the band.
 */
function fg(hex: string): (text: string) => string {
  const { r, g, b } = hexToRgb(hex);
  const open = truecolor
    ? `[38;2;${r};${g};${b}m`
    : `[38;5;${rgbTo256(r, g, b)}m`;
  return (text: string) => `${open}${text}[39m`;
}

/** A background band wrapper plus its raw SGR opener. */
export interface BackgroundBand {
  (text: string): string;
  /** The band's raw SGR opener, for re-opening after a mid-line full reset. */
  readonly open: string;
}

/**
 * A background colour wrapper. Only the background is reset.
 *
 * The raw SGR opener is exposed as {@link BackgroundBand.open} so the layout
 * shell that owns a band can re-open it after a full reset (`[0m`) inside the
 * content: emphasis styles like bold end in a full reset, and without a
 * re-open they would punch a hole in the band for the rest of the line.
 */
function bg(hex: string): BackgroundBand {
  const { r, g, b } = hexToRgb(hex);
  const open = truecolor
    ? `[48;2;${r};${g};${b}m`
    : `[48;5;${rgbTo256(r, g, b)}m`;
  const band = Object.assign(
    (text: string) => `${open}${text}[49m`,
    { open },
  );
  return band;
}

// ---------------------------------------------------------------------------
// Styles
// ---------------------------------------------------------------------------

export const style = {
  dim: fg(palette.dimGray),
  bold: wrap("1"),
  italic: wrap("3"),
  red: fg(palette.red),
  green: fg(palette.green),
  yellow: fg(palette.yellow),
  blue: fg(palette.blue),
  magenta: fg(palette.accent),
  cyan: fg(palette.cyan),
  grey: fg(palette.gray),
  text: fg(palette.text),
  accent: fg(palette.accent),
  heading: fg(palette.heading),
  link: fg(palette.link),
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
  /** A human or runtime-originated inbound turn, drawn on its own band. */
  user: style.text,
  /** The current selection or an interactive affordance. */
  accent: style.accent,
  /** Work the runtime says is in flight. */
  pending: style.yellow,
  /** A runtime-published success. */
  success: style.green,
  /** A runtime-published refusal, denial, cancellation, or timeout. */
  warning: style.yellow,
  /** A runtime-published failure. */
  error: style.red,
  /** A tool card's title. */
  toolTitle: style.text,
  /** Verbatim tool output. */
  toolOutput: style.grey,
  /** Identities, counts, durations, hints — anything supporting. */
  meta: style.dim,
  /** Structural punctuation and separators. */
  chrome: style.grey,
  /** Emphasis inside an otherwise unstyled line. */
  strong: style.bold,
};

/**
 * The background bands.
 *
 * One entry per *visual block* the transcript can draw, not per colour. A
 * block names its band by role and the layout shell fills the terminal width
 * with it; no component composes a background into a string of its own,
 * because no component knows how wide the line will be.
 */
export const background = {
  user: bg(palette.userMessageBg),
  toolPending: bg(palette.toolPendingBg),
  toolSuccess: bg(palette.toolSuccessBg),
  toolError: bg(palette.toolErrorBg),
  popup: bg(palette.popupBg),
};

/** The name of one background band. */
export type BackgroundRole = keyof typeof background;

/**
 * Fills one laid-out line with a background band, edge to edge.
 *
 * This is the shell-level composition the band language reserves for the
 * layer that knows the width: the line is padded to `width` first, and the
 * band is re-opened after every full reset inside the content so an emphasis
 * style cannot leave the remainder of the line — including the padding that
 * makes the band a rectangle — on the terminal's default background.
 */
export function fillBand(band: BackgroundBand, line: string, width: number): string {
  const padded = line + " ".repeat(Math.max(0, width - visibleWidth(line)));
  return `${band.open}${padded.replaceAll(`[0m`, `[0m${band.open}`)}[49m`;
}

/** The markdown theme handed to Pi's renderer. */
export const markdownTheme = {
  heading: (text: string) => style.bold(style.heading(text)),
  link: style.link,
  linkUrl: style.dim,
  code: style.accent,
  codeBlock: style.green,
  codeBlockBorder: style.grey,
  quote: style.grey,
  quoteBorder: style.grey,
  hr: style.grey,
  listBullet: style.accent,
  bold: style.bold,
  italic: style.italic,
  strikethrough: style.dim,
  underline: wrap("4"),
};

/** The select-list theme handed to Pi's overlay lists and the editor. */
export const selectListTheme = {
  selectedPrefix: style.accent,
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
