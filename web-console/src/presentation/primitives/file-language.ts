/* Copyright (c) 2026 DeepSeek. MIT. Source-derived; see PROVENANCE.md. */
/**
 * The single file-extension to syntax-highlighting language table shared by every
 * code surface: the Client's document Code preview and diff review, and the Host
 * read tool's persisted `lang` hint. Language ids are the grammar ids the Client
 * highlighter's alias table resolves, so a language returned here reaches
 * `highlightLines`/`highlightToHtml` unchanged; a filename outside the table, or
 * one naming a language the highlighter does not register, renders as plain text.
 * @module @deepseek-ai/dsh-util-code-language
 */

/**
 * Recognized extensions per canonical language id. Values are lowercase
 * extensions without the dot; the id names the grammar rather than a display
 * label. The set covers common source, config, script, data, and markup
 * extensions a line-numbered code view or preview benefits from highlighting; it
 * is deliberately not an exhaustive linguist registry. Extensions whose Shiki
 * grammar does not exist map to the nearest available grammar
 * (`properties` to `ini`, whose registration carries the `properties` alias) or
 * stay unlisted. Certificate and lock extensions (`pem`, `crt`, `key`, `cer`,
 * `lock`) stay unlisted. Preview registries order specialized viewers before
 * Code so CSV can default to Spreadsheet while retaining syntax highlighting.
 * `makefile` covers only the suffix
 * (`foo.makefile`); the extensionless `Makefile` name stays unlisted.
 */
const LANGUAGE_EXTENSIONS: Readonly<Record<string, readonly string[]>> = {
  typescript: ['ts', 'tsx', 'mts', 'cts'],
  javascript: ['js', 'jsx', 'mjs', 'cjs'],
  shellscript: ['sh', 'bash', 'zsh'],
  fish: ['fish'],
  json: ['json', 'jsonc', 'jsonl', 'ndjson', 'ipynb'],
  csv: ['csv'],
  python: ['py', 'pyw', 'pyi'],
  ruby: ['rb', 'rake', 'gemspec'],
  go: ['go'],
  rust: ['rs'],
  java: ['java'],
  c: ['c', 'h'],
  cpp: ['cc', 'cpp', 'cxx', 'hh', 'hpp', 'hxx'],
  csharp: ['cs'],
  kotlin: ['kt', 'kts'],
  swift: ['swift'],
  php: ['php'],
  yaml: ['yaml', 'yml'],
  toml: ['toml'],
  ini: ['ini', 'conf', 'cfg', 'properties'],
  dotenv: ['env'],
  log: ['log'],
  diff: ['diff', 'patch'],
  http: ['http'],
  markdown: ['md', 'markdown'],
  mdx: ['mdx'],
  rst: ['rst'],
  latex: ['tex', 'sty', 'cls'],
  bibtex: ['bib'],
  asciidoc: ['adoc'],
  html: ['html', 'htm', 'xhtml'],
  css: ['css'],
  scss: ['scss'],
  less: ['less'],
  sql: ['sql'],
  xml: ['xml', 'xsd', 'xsl', 'xslt', 'plist', 'svg'],
  lua: ['lua'],
  bat: ['bat', 'cmd'],
  powershell: ['ps1', 'psm1', 'psd1'],
  r: ['r'],
  julia: ['jl'],
  dart: ['dart'],
  scala: ['scala'],
  clojure: ['clj', 'cljs', 'edn'],
  erlang: ['erl', 'hrl'],
  elixir: ['ex', 'exs'],
  haskell: ['hs'],
  fsharp: ['fs', 'fsi', 'fsx'],
  vb: ['vb'],
  perl: ['pl', 'pm'],
  verilog: ['v'],
  'system-verilog': ['sv', 'svh'],
  graphql: ['graphql', 'gql'],
  proto: ['proto'],
  hcl: ['tf', 'tfvars', 'hcl'],
  nix: ['nix'],
  vue: ['vue'],
  svelte: ['svelte'],
  make: ['makefile', 'mk'],
  cmake: ['cmake'],
  groovy: ['gradle', 'groovy'],
}

/**
 * Lowercase extension to canonical language id. A Map, not an object: a filename
 * whose extension is an `Object.prototype` key (`foo.constructor`,
 * `foo.__proto__`) must miss instead of resolving the inherited member, which
 * would otherwise reach callers as a non-string language value.
 */
const LANGUAGES = new Map(Object.entries(LANGUAGE_EXTENSIONS)
  .flatMap(([language, extensions]) => extensions.map(extension => [extension, language] as const)))

function extensionForPath(path: string): string | undefined {
  const slash = Math.max(path.lastIndexOf('/'), path.lastIndexOf('\\'))
  const base = path.slice(slash + 1)
  const dot = base.lastIndexOf('.')
  return dot < 0 ? undefined : base.slice(dot + 1).toLowerCase()
}

/**
 * Derive the syntax-highlighting language from a filename or path, case
 * insensitively. The extension is the text after the last dot of the final path
 * segment; a leading dot is still the separator, so `.env` resolves to `dotenv`
 * while an unlisted dotfile (`.gitignore`) and an extensionless name return
 * `undefined`. Both path separators are recognized, so
 * `C:\\dir\\main.PS1` resolves like `dir/main.ps1`.
 * @param path - decoded filename or path.
 * @returns the canonical language id, or `undefined` for an unrecognized or absent suffix.
 */
export function languageForPath(path: string): string | undefined {
  const extension = extensionForPath(path)
  return extension === undefined ? undefined : LANGUAGES.get(extension)
}

