/**
 * A minimal ANSI palette for the rustX terminal projection.
 *
 * Deliberately not a theme system: Issue #39 lists complex theming as a
 * non-goal. These are plain SGR wrappers with no configuration surface.
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
