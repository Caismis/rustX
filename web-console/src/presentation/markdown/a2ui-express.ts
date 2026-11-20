/** Bounded reader for assistant-authored Express surfaces. */
const signatures = {
  Slider: ['label', 'min', 'max', 'value', 'steps', 'weight'],
  CheckBox: ['label', 'value', 'weight'],
  TextField: ['label', 'value', 'placeholder', 'variant', 'weight'],
  ChoicePicker: ['label', 'variant', 'options', 'value', 'displayStyle', 'filterable', 'weight'],
  DateTimeInput: ['value', 'enableDate', 'enableTime', 'min', 'max', 'label', 'weight'],
  Button: ['child', 'variant', 'action', 'weight', 'color', 'size', 'block'],
  Table: ['rows', 'columns', 'title'],
  LineChart: ['rows', 'x', 'y', 'title'],
  BarChart: ['rows', 'x', 'y', 'title'],
} satisfies Record<string, string[]>;

export type ExpressKind = 'Text' | 'Column' | 'List' | 'Card' | 'Row' | 'Divider' | keyof typeof signatures;
export interface ExpressNode {
  kind: ExpressKind;
  children: ExpressNode[];
  text?: string;
  textTemplate?: TextPart[];
  variant?: string;
  props?: Record<string, unknown>;
  bindings?: Record<string, string[]>;
}
export interface ExpressSurface {
  root: ExpressNode;
  data: Record<string, unknown>;
}

type Token =
  | { type: 'ident'; value: string }
  | { type: 'string'; value: string }
  | { type: 'number'; value: number }
  | { type: 'ref'; path: string[] }
  | { type: 'punct'; value: string }
  | { type: 'eof' };

const PUNCT = new Set(['=', '(', ')', '[', ']', '{', '}', ',', ':']);
const IDENT_START = /[A-Za-z_]/;
const IDENT_PART = /[A-Za-z0-9_]/;
// Path segments accept any run of non-delimiter characters so unicode keys
// (`$/标题`) work; `/` separates segments and every grammar punct terminates.
const SEGMENT = /^[^\s\/()[\]{},:="'`$=<>]+/;
const NUMBER = /^-?\d+(\.\d+)?/;
const EOF_TOKEN: Token = { type: 'eof' };
const MAX_SOURCE_LENGTH = 65536;

function isSpace(ch: string): boolean {
  return ch === ' ' || ch === '\t' || ch === '\n' || ch === '\r' || ch === '\f' || ch === '\v';
}

/**
 * Split express source into tokens. Returns null on any character the
 * grammar cannot place (including unterminated strings and `$` not followed
 * by a `/`-path), which the caller treats as malformed input.
 */
function tokenize(source: string): Token[] | null {
  if (source.length > MAX_SOURCE_LENGTH) return null;
  const tokens: Token[] = [];
  let i = 0;
  while (i < source.length) {
    if (tokens.length >= 8192) return null;
    const ch = source[i];
    if (isSpace(ch)) { i++; continue; }
    if (ch === '#' || source.startsWith('//', i)) {
      const end = source.indexOf('\n', i);
      i = end < 0 ? source.length : end + 1;
      continue;
    }
    if (ch === '"') {
      let j = i + 1;
      let value = '';
      let closed = false;
      while (j < source.length) {
        const c = source[j];
        if (c === '"') { closed = true; j++; break; }
        if (c === '\\') {
          j++;
          if (j >= source.length) return null;
          const esc = source[j];
          if (esc === 'u') {
            const hex = source.slice(j + 1, j + 5);
            if (!/^[0-9a-fA-F]{4}$/.test(hex)) return null;
            value += String.fromCharCode(parseInt(hex, 16));
            j += 5;
          } else {
            value += esc === 'n' ? '\n' : esc === 't' ? '\t' : esc === 'r' ? '\r'
              : esc === 'b' ? '\b' : esc === 'f' ? '\f' : esc;
            j++;
          }
          continue;
        }
        value += c;
        j++;
      }
      if (!closed) return null;
      tokens.push({ type: 'string', value });
      i = j;
      continue;
    }
    if (ch === '$') {
      if (source[i + 1] !== '/') return null;
      const path: string[] = [];
      let j = i + 2;
      for (;;) {
        const m = SEGMENT.exec(source.slice(j));
        if (m === null) return null;
        if (path.length >= 64 || (/^\d+$/.test(m[0]) && Number(m[0]) > 8192)) return null;
        path.push(m[0]);
        j += m[0].length;
        if (source[j] !== '/') break;
        j++;
      }
      tokens.push({ type: 'ref', path });
      i = j;
      continue;
    }
    if (ch >= '0' && ch <= '9' || ch === '-' && source[i + 1] >= '0' && source[i + 1] <= '9') {
      const m = NUMBER.exec(source.slice(i));
      if (m === null) return null;
      const value = Number(m[0]);
      if (!Number.isFinite(value)) return null;
      tokens.push({ type: 'number', value });
      i += m[0].length;
      continue;
    }
    if (IDENT_START.test(ch)) {
      let j = i + 1;
      while (j < source.length && IDENT_PART.test(source[j])) j++;
      tokens.push({ type: 'ident', value: source.slice(i, j) });
      i = j;
      continue;
    }
    if (PUNCT.has(ch)) {
      tokens.push({ type: 'punct', value: ch });
      i++;
      continue;
    }
    return null;
  }
  tokens.push(EOF_TOKEN);
  return tokens;
}

/** Literal data value on the right of a `$/path =` binding. */
type Value =
  | { type: 'string'; value: string }
  | { type: 'number'; value: number }
  | { type: 'boolean'; value: boolean }
  | { type: 'null' }
  | { type: 'array'; items: Value[] }
  | { type: 'object'; entries: [string, Value][] };

/** Scalar operand for Text content and variants: literal or `$` reference. */
type ScalarExpr =
  | { type: 'literal'; value: string | number | boolean | null }
  | { type: 'ref'; path: string[] };

type TextPart = string | { path: string[] };
type TextExpr = ScalarExpr | { type: 'format'; parts: TextPart[] };

type PropertyExpr = { type: 'value'; value: unknown } | { type: 'ref'; path: string[] };

/** Pre-resolution component: containers hold raw children; Text holds its operand. */
type RawNode = { reference: string } | {
  kind: ExpressKind;
  children: RawNode[];
  textExpr?: TextExpr;
  properties?: Record<string, PropertyExpr>;
  variant?: string;
}

/** Parsed program: the resolved-pending root plus the `$` binding store. */
interface Program {
  root: RawNode;
  definitions: ReadonlyMap<string, RawNode>;
  data: Record<string, unknown>;
}

const COMPONENT_KINDS = new Set<string>(['Card', 'Column', 'List', 'Row', 'Divider', 'Text', ...Object.keys(signatures)]);
const INDEX_SEGMENT = /^\d+$/;
// Bounds recursion on untrusted input; ~256 nesting levels is far beyond any
// legitimate card, and deeper input falls back to the raw source.
const MAX_PARSE_DEPTH = 256;

/** Evaluate a literal binding value into plain JS data (null-prototype objects). */
function evalValue(value: Value): unknown {
  switch (value.type) {
    case 'string':
    case 'number':
    case 'boolean':
      return value.value;
    case 'null':
      return null;
    case 'array':
      return value.items.map(evalValue);
    case 'object': {
      const out: Record<string, unknown> = Object.create(null);
      for (const [key, item] of value.entries) out[key] = evalValue(item);
      return out;
    }
  }
}

/**
 * Deep-assign `value` at `path` inside `root`, creating intermediate objects
 * or arrays based on whether the next segment is numeric. Returns false on
 * type conflicts (a scalar or mismatched container already occupies a step).
 */
function deepSet(root: Record<string, unknown>, path: readonly string[], value: unknown): boolean {
  let cur: unknown = root;
  for (let i = 0; i < path.length - 1; i++) {
    const seg = path[i];
    const container: Record<string, unknown> | unknown[] = INDEX_SEGMENT.test(path[i + 1]) ? [] : Object.create(null);
    if (Array.isArray(cur)) {
      if (!INDEX_SEGMENT.test(seg)) return false;
      const idx = Number(seg);
      const existing = cur[idx];
      if (existing === undefined || existing === null) cur[idx] = container;
      else if (typeof existing !== 'object') return false;
      cur = cur[idx];
    } else if (typeof cur === 'object' && cur !== null) {
      const existing = (cur as Record<string, unknown>)[seg];
      if (existing === undefined) (cur as Record<string, unknown>)[seg] = container;
      else if (typeof existing !== 'object' || existing === null) return false;
      cur = (cur as Record<string, unknown>)[seg];
    } else {
      return false;
    }
  }
  const last = path[path.length - 1];
  if (Array.isArray(cur)) {
    if (!INDEX_SEGMENT.test(last)) return false;
    cur[Number(last)] = value;
  } else if (typeof cur === 'object' && cur !== null) {
    (cur as Record<string, unknown>)[last] = value;
  } else {
    return false;
  }
  return true;
}

const MISSING = Symbol('a2ui-missing');

/** Walk `path` through `data`; MISSING when any step is absent or mistyped. */
function lookupPath(data: Record<string, unknown>, path: readonly string[]): unknown {
  let cur: unknown = data;
  for (const seg of path) {
    if (Array.isArray(cur)) {
      if (!INDEX_SEGMENT.test(seg)) return MISSING;
      const idx = Number(seg);
      if (idx >= cur.length) return MISSING;
      cur = cur[idx];
    } else if (typeof cur === 'object' && cur !== null) {
      if (!Object.prototype.hasOwnProperty.call(cur, seg)) return MISSING;
      cur = (cur as Record<string, unknown>)[seg];
    } else {
      return MISSING;
    }
  }
  return cur;
}

/** Text displays scalar data bindings. Composite values need a nested path. */
function scalarToText(value: unknown): string | null {
  if (value === null || value === undefined) return '';
  if (typeof value === 'string') return value;
  if (typeof value === 'number' || typeof value === 'boolean') return String(value);
  return null;
}

/** Compile absolute JSON Pointer placeholders and escaped literal `${` markers. */
function parseTextTemplate(template: string): TextPart[] | null {
  const parts: TextPart[] = [];
  let end = 0;
  for (const match of template.matchAll(/\\\$\{|\$\{([^{}]*)\}|\$\{/g)) {
    if (match.index > end) parts.push(template.slice(end, match.index));
    end = match.index + match[0].length;
    if (match[0] === '\\\${') {
      parts.push('${');
      continue;
    }
    const pointer = match[1];
    if (!pointer?.startsWith('/') || /~(?![01])/.test(pointer)) return null;
    const path = pointer.slice(1).split('/');
    if (path.length > 64) return null;
    parts.push({ path: path.map(segment => segment.replaceAll('~1', '/').replaceAll('~0', '~')) });
  }
  if (end < template.length) parts.push(template.slice(end));
  return parts;
}

function resolveTemplate(parts: TextPart[], data: Record<string, unknown>): string | null {
  let result = '';
  for (const part of parts) {
    const value = typeof part === 'string' ? part : scalarToText(lookupPath(data, part.path));
    if (value === null || result.length + value.length > MAX_SOURCE_LENGTH) return null;
    result += value;
  }
  return result;
}

/**
 * Recursive-descent parser over the token stream. Grammar:
 *
 *   program    := statement* EOF
 *   statement  := ref '=' value            // `$/path =` data binding
 *               | 'root' '=' component     // the component tree (last wins)
 *   component  := ident '(' args ')'       // ident in {@link COMPONENT_KINDS}
 *   args       := child (',' child)*       // containers: components/arrays
 *               | textOperand (',' variant)? // Text
 *               | variant?                 // Divider
 *   child      := component | '[' child (',' child)* ']'   // arrays flatten
 *   scalar     := string | number | ref | 'true' | 'false' | 'null'
 *   textOperand := scalar | 'formatString' '(' ('value' '=')? string ')'
 *   variant    := string | number | ident
 *   value      := scalar-without-ref | array | object
 *
 * Trailing commas are tolerated before a closing bracket; every other
 * deviation returns null so the caller falls back to the raw source.
 */
class Parser {
  private pos = 0;
  private readonly definitions = new Map<string, RawNode>();
  private readonly data: Record<string, unknown> = Object.create(null);

  constructor(private readonly tokens: Token[]) {}

  parse(): Program | null {
    while (this.peek().type !== 'eof') {
      if (!this.statement()) return null;
    }
    const root = this.definitions.get('root');
    return root ? { root, data: this.data, definitions: this.definitions } : null;
  }

  private peek(): Token {
    return this.tokens[this.pos] ?? EOF_TOKEN;
  }

  private atPunct(value: string): boolean {
    const t = this.peek();
    return t.type === 'punct' && t.value === value;
  }

  private expectPunct(value: string): boolean {
    if (!this.atPunct(value)) return false;
    this.pos++;
    return true;
  }

  private statement(): boolean {
    const t = this.peek();
    if (t.type === 'ref') {
      this.pos++;
      if (!this.expectPunct('=')) return false;
      const value = this.parseValue(0);
      return value !== null && deepSet(this.data, t.path, evalValue(value));
    }
    if (t.type === 'ident' && !this.definitions.has(t.value)) {
      this.pos++;
      if (!this.expectPunct('=')) return false;
      const node = this.parseComponent(0);
      if (node === null) return false;
      this.definitions.set(t.value, node);
      return true;
    }
    return false;
  }

  private parseComponent(depth: number): RawNode | null {
    if (depth > MAX_PARSE_DEPTH) return null;
    const name = this.peek();
    if (name.type !== 'ident') return null;
    const kind = name.value as ExpressKind;
    this.pos++;
    if (!this.expectPunct('(')) return { reference: name.value };
    if (!COMPONENT_KINDS.has(name.value)) return null;
    let node: RawNode;
    if (kind === 'Text') {
      const textExpr = this.parseTextExpr();
      if (textExpr === null) return null;
      let variant: string | undefined;
      if (this.atPunct(',')) {
        this.pos++;
        const parsed = this.parseVariant();
        if (parsed === null) return null;
        variant = parsed;
      }
      node = { kind, children: [], textExpr, variant };
    } else if (Object.hasOwn(signatures, kind)) {
      const parsed = this.parseProperties(kind as keyof typeof signatures, depth);
      if (!parsed) return null;
      node = parsed;
    } else if (kind === 'Divider') {
      let variant: string | undefined;
      if (!this.atPunct(')')) {
        const parsed = this.parseVariant();
        if (parsed === null) return null;
        variant = parsed;
      }
      node = { kind, children: [], variant };
    } else {
      const children: RawNode[] = [];
      while (!this.atPunct(')')) {
        const seq = this.parseChildSeq(depth + 1);
        if (seq === null) return null;
        children.push(...seq);
        if (!this.atPunct(',')) break;
        this.pos++;
      }
      node = { kind, children };
    }
    return this.expectPunct(')') ? node : null;
  }

  private parseProperties(kind: keyof typeof signatures, depth: number): RawNode | null {
    const names: readonly string[] = signatures[kind];
    const seen = new Set<string>();
    const properties: Record<string, PropertyExpr> = Object.create(null);
    const children: RawNode[] = [];
    let index = 0;
    while (!this.atPunct(')')) {
      let key = names[index++];
      const first = this.peek(), next = this.tokens[this.pos + 1];
      if (first.type === 'ident' && next?.type === 'punct' && next.value === '=') {
        key = first.value;
        this.pos += 2;
      }
      const argument = this.peek();
      if (!key || !names.includes(key) || seen.has(key)) return null;
      seen.add(key);
      if (argument.type === 'ident' && argument.value === '_') {
        this.pos++;
      } else if (key === 'child') {
        const child = this.parseComponent(depth + 1);
        if (!child) return null;
        children.push(child);
      } else if (key === 'action') {
        const url = this.parseAction();
        if (url === null) return null;
        if (url) properties.openUrl = url;
      } else if (argument.type === 'ref') {
        properties[key] = argument;
        this.pos++;
      } else {
        const value = this.parseValue(depth + 1);
        if (!value) return null;
        properties[key] = { type: 'value', value: evalValue(value) };
      }
      if (!this.expectPunct(',')) break;
    }
    if (kind === 'Button' && children.length !== 1) return null;
    return { kind, properties, children };
  }

  private parseAction(): PropertyExpr | undefined | null {
    const name = this.peek();
    if (name.type !== 'ident') return null;
    this.pos++;
    if (!this.expectPunct('(')) return null;
    const start = this.pos;
    let nesting = 1;
    while (nesting > 0) {
      if (this.peek().type === 'eof') return null;
      if (this.atPunct('(')) nesting++;
      if (this.atPunct(')')) nesting--;
      this.pos++;
    }
    // Only the URL operand is retained. Other actions remain display-only.
    if (name.value !== 'openUrl') return undefined;
    const args = this.tokens.slice(start, this.pos - 1);
    const last = args.at(-1);
    if (last?.type === 'punct' && last.value === ',') args.pop();
    const [first, equals, namedValue] = args;
    const value = args.length === 1 ? first
      : args.length === 3 && first.type === 'ident' && first.value === 'url' && equals.type === 'punct' && equals.value === '='
        ? namedValue : undefined;
    if (value?.type === 'string') return { type: 'value', value: value.value };
    if (value?.type === 'ref') return value;
    return undefined;
  }

  /** One child position: a component, or an array literal flattened to nodes. */
  private parseChildSeq(depth: number): RawNode[] | null {
    if (depth > MAX_PARSE_DEPTH) return null;
    if (this.atPunct('[')) {
      this.pos++;
      const items: RawNode[] = [];
      while (!this.atPunct(']')) {
        const seq = this.parseChildSeq(depth + 1);
        if (seq === null) return null;
        items.push(...seq);
        if (!this.atPunct(',')) break;
        this.pos++;
      }
      if (!this.expectPunct(']')) return null;
      return items;
    }
    const node = this.parseComponent(depth);
    return node === null ? null : [node];
  }

  /** Text additionally accepts a template that reads absolute data paths. */
  private parseTextExpr(): TextExpr | null {
    const name = this.peek();
    if (name.type !== 'ident' || name.value !== 'formatString') return this.parseScalarExpr();
    this.pos++;
    if (!this.expectPunct('(')) return null;
    const argument = this.peek();
    if (argument.type === 'ident' && argument.value === 'value') {
      this.pos++;
      if (!this.expectPunct('=')) return null;
    }
    const template = this.peek();
    if (template.type !== 'string') return null;
    this.pos++;
    this.expectPunct(',');
    if (!this.expectPunct(')')) return null;
    const parts = parseTextTemplate(template.value);
    return parts && { type: 'format', parts };
  }

  /** A literal scalar or a data reference. */
  private parseScalarExpr(allowRef = true): ScalarExpr | null {
    const t = this.peek();
    switch (t.type) {
      case 'string':
        this.pos++;
        return { type: 'literal', value: t.value };
      case 'number':
        this.pos++;
        return { type: 'literal', value: t.value };
      case 'ref':
        if (!allowRef) return null;
        this.pos++;
        return { type: 'ref', path: t.path };
      case 'ident': {
        this.pos++;
        if (t.value === 'true') return { type: 'literal', value: true };
        if (t.value === 'false') return { type: 'literal', value: false };
        if (t.value === 'null') return { type: 'literal', value: null };
        return null;
      }
      default:
        return null;
    }
  }

  /** Variant operand: string, number, or bare identifier rendered literally. */
  private parseVariant(): string | null {
    const t = this.peek();
    if (t.type === 'string' || t.type === 'ident') {
      this.pos++;
      return t.value;
    }
    if (t.type === 'number') {
      this.pos++;
      return String(t.value);
    }
    return null;
  }

  private parseValue(depth: number): Value | null {
    if (depth > MAX_PARSE_DEPTH) return null;
    const t = this.peek();
    if (t.type === 'punct' && t.value === '[') {
      this.pos++;
      const items: Value[] = [];
      while (!this.atPunct(']')) {
        const item = this.parseValue(depth + 1);
        if (item === null) return null;
        items.push(item);
        if (!this.atPunct(',')) break;
        this.pos++;
      }
      return this.expectPunct(']') ? { type: 'array', items } : null;
    }
    if (t.type === 'punct' && t.value === '{') {
      this.pos++;
      const entries: [string, Value][] = [];
      while (!this.atPunct('}')) {
        const key = this.peek();
        let name: string;
        if (key.type === 'ident' || key.type === 'string') {
          name = key.value;
        } else if (key.type === 'number') {
          name = String(key.value);
        } else {
          return null;
        }
        this.pos++;
        if (!this.expectPunct(':')) return null;
        const item = this.parseValue(depth + 1);
        if (item === null) return null;
        entries.push([name, item]);
        if (!this.atPunct(',')) break;
        this.pos++;
      }
      return this.expectPunct('}') ? { type: 'object', entries } : null;
    }
    const scalar = this.parseScalarExpr(false);
    if (scalar === null || scalar.type !== 'literal') return null;
    if (scalar.value === null) return { type: 'null' };
    if (typeof scalar.value === 'string') return { type: 'string', value: scalar.value };
    if (typeof scalar.value === 'number') return { type: 'number', value: scalar.value };
    return { type: 'boolean', value: scalar.value };
  }
}

/** Resolve a Text operand against the binding store; null when unresolvable. */
function resolveText(expr: TextExpr, data: Record<string, unknown>): string | null {
  if (expr.type === 'literal') return scalarToText(expr.value);
  if (expr.type === 'format') return resolveTemplate(expr.parts, data);
  const value = lookupPath(data, expr.path);
  return value === MISSING ? null : scalarToText(value);
}

/** Resolve a raw component tree into the public {@link ExpressNode} shape. */
function resolveNode(node: RawNode, program: Program, budget: { remaining: number }, depth: number): ExpressNode | null {
  if (--budget.remaining < 0 || depth > MAX_PARSE_DEPTH) return null;
  if ('reference' in node) {
    const target = program.definitions.get(node.reference);
    return target ? resolveNode(target, program, budget, depth + 1) : null;
  }
  if (node.kind === 'Text') {
    const template = node.textExpr?.type === 'format' ? node.textExpr.parts : undefined;
    if (template && (budget.remaining -= template.length) < 0) return null;
    const text = node.textExpr === undefined ? '' : resolveText(node.textExpr, program.data);
    if (text === null) return null;
    return {
      kind: 'Text', children: [], text,
      ...(template ? { textTemplate: template } : {}),
      ...(node.textExpr?.type === 'ref' ? { bindings: { text: node.textExpr.path } } : {}),
      ...(node.variant === undefined ? {} : { variant: node.variant }),
    };
  }
  if (node.kind === 'Divider') {
    return { kind: 'Divider', children: [], ...(node.variant === undefined ? {} : { variant: node.variant }) };
  }
  const children: ExpressNode[] = [];
  for (const child of node.children) {
    const resolved = resolveNode(child, program, budget, depth + 1);
    if (resolved === null) return null;
    children.push(resolved);
  }
  if (!node.properties) return { kind: node.kind, children };
  const props: Record<string, unknown> = Object.create(null);
  const bindings: Record<string, string[]> = Object.create(null);
  for (const [key, expr] of Object.entries(node.properties)) {
    if (expr.type === 'ref') {
      bindings[key] = expr.path;
      const value = lookupPath(program.data, expr.path);
      props[key] = value === MISSING ? undefined : value;
    } else {
      props[key] = expr.value;
    }
  }
  if (['Table', 'LineChart', 'BarChart'].includes(node.kind)) {
    if (!Array.isArray(props.rows) || !props.rows.every(row => row && typeof row === 'object' && !Array.isArray(row))) return null;
    const strings = (value: unknown) => Array.isArray(value) && value.every(item => typeof item === 'string');
    if (node.kind === 'Table' ? props.columns !== undefined && !strings(props.columns)
      : typeof props.x !== 'string' || !(typeof props.y === 'string' || strings(props.y))) return null;
    // Bound rendered cells and plotted points, including repeated named components.
    const fields = node.kind === 'Table'
      ? (Array.isArray(props.columns) && props.columns.length ? props.columns.length : new Set(props.rows.flatMap(row => Object.keys(row))).size)
      : (Array.isArray(props.y) ? props.y.length : 1);
    if (!fields && node.kind !== 'Table') return null;
    budget.remaining -= props.rows.length * Math.max(fields, 1);
    if (budget.remaining < 0) return null;
  }
  return { kind: node.kind, children, props, bindings };
}

/** Strip an optional `<a2ui>...</a2ui>` wrapper so callers may pass either form. */
function unwrapA2ui(source: string): string {
  const text = source.trim();
  return text.startsWith('<a2ui>') && text.endsWith('</a2ui>')
    ? text.slice('<a2ui>'.length, -'</a2ui>'.length)
    : text;
}

/**
 * Parse express source into a resolved node tree. Accepts the bare DSL or a
 * `<a2ui>`-wrapped block. Returns null for empty, partial, or malformed
 * input — including unresolved `$` references and missing `root` — so the
 * Markdown pipeline can fall back to showing the source verbatim.
 * @param source - Express DSL text (optionally `<a2ui>`-wrapped).
 * @returns The resolved root node, or null when the input is not valid express.
 */
export function parseExpress(source: string): ExpressSurface | null {
  const tokens = tokenize(unwrapA2ui(source));
  if (tokens === null) return null;
  const program = new Parser(tokens).parse();
  if (program === null) return null;
  const root = resolveNode(program.root, program, { remaining: 4096 }, 0);
  return root && { root, data: program.data };
}

/** Copy only containers on the edited path. Each mounted surface owns its own model. */
export function writeBinding(data: Record<string, unknown>, path: string[], value: unknown): Record<string, unknown> {
  function write(parent: unknown, index: number): unknown {
    if (index === path.length) return value;
    const key = path[index]!;
    if (Array.isArray(parent)) {
      if (!INDEX_SEGMENT.test(key)) return parent;
      const copy = parent.slice();
      copy[Number(key)] = write(parent[Number(key)], index + 1);
      return copy;
    }
    const copy = Object.assign(Object.create(null), parent && typeof parent === 'object' ? parent : undefined);
    copy[key] = write(copy[key], index + 1);
    return copy;
  }
  return write(data, 0) as Record<string, unknown>;
}

export function boundProps(node: ExpressNode, data: Record<string, unknown>): Record<string, unknown> {
  const props = { ...node.props };
  if (node.kind === 'Text') props.text = node.textTemplate ? resolveTemplate(node.textTemplate, data) : node.text;
  for (const [key, path] of Object.entries(node.bindings ?? {})) {
    const value = lookupPath(data, path);
    props[key] = value === MISSING ? undefined : value;
  }
  return props;
}
