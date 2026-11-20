import { ARRAY_INDEX, MAX_DEPTH, MAX_SOURCE_LENGTH, bindingPath, evaluator, functions, lookup, scalarText, type Expr, type Scope } from './express-values';

const signatures = {
  Text: ['text', 'variant', 'weight'],
  Column: ['children', 'justify', 'align', 'weight', 'wrap', 'gap'],
  Row: ['children', 'justify', 'align', 'weight', 'wrap', 'gap'],
  List: ['children', 'direction', 'align', 'weight', 'marker', 'gap', 'items'],
  Card: ['child', 'weight', 'variant', 'children'],
  Divider: ['axis', 'weight'],
  Icon: ['name', 'weight', 'label', 'size'],
  Slider: ['label', 'min', 'max', 'value', 'steps', 'weight'],
  CheckBox: ['label', 'value', 'weight'],
  TextField: ['label', 'value', 'placeholder', 'variant', 'weight'],
  ChoicePicker: ['label', 'variant', 'options', 'value', 'displayStyle', 'filterable', 'weight'],
  DateTimeInput: ['value', 'enableDate', 'enableTime', 'min', 'max', 'label', 'weight'],
  Button: ['child', 'variant', 'action', 'weight', 'color', 'size', 'block'],
  Table: ['rows', 'columns', 'title'],
  LineChart: ['rows', 'x', 'y', 'title'],
  BarChart: ['rows', 'x', 'y', 'title'],
} satisfies Record<string, readonly string[]>;

export type ExpressKind = keyof typeof signatures;
type EventAction = { name: Expr; context?: Expr; userMessage?: Expr };
export interface ExpressNode {
  kind: ExpressKind;
  children: ExpressNode[];
  props: Record<string, unknown>;
  bindings: Record<string, string[]>;
  scope: Scope;
  event?: EventAction;
}
export interface ExpressSurface {
  root: ExpressNode;
  data: Record<string, unknown>;
  render: (data: Record<string, unknown>) => ExpressNode | null;
  resolve: (data: Record<string, unknown>) => ReturnType<typeof evaluator>;
}

type Child = RawNode | { template: RawNode; path: string };
type RawNode = { reference: string } | {
  kind: ExpressKind;
  children: Child[];
  props: Record<string, Expr>;
  event?: EventAction;
};

type Token =
  | { type: 'ident'; value: string }
  | { type: 'string'; value: string }
  | { type: 'number'; value: number }
  | { type: 'ref'; path: string }
  | { type: 'punct'; value: string }
  | { type: 'eof' };

const PUNCT = new Set(['=', '(', ')', '[', ']', '{', '}', ',', ':']);
const IDENTIFIER = /@?[\p{XID_Start}_][\p{XID_Continue}]*/uy;
// Path segments accept any run of non-delimiter characters so unicode keys
// (`$/标题`) work; `/` separates segments and every grammar punct terminates.
const SEGMENT = /^[^\s\/()[\]{},:="'`$=<>]+/;
const NUMBER = /^[+-]?(?:\d+(?:\.\d*)?|\.\d+)(?:[eE][+-]?\d+)?/;
const EOF_TOKEN: Token = { type: 'eof' };

function isSpace(ch: string): boolean {
  return ch === ' ' || ch === '\t' || ch === '\n' || ch === '\r' || ch === '\f' || ch === '\v';
}

/**
 * Split express source into tokens. Returns null on any character the
 * grammar cannot place (including unterminated strings), which the caller treats as malformed input.
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
    const raw = source.startsWith('r"', i);
    if (ch === '"' || raw) {
      const start = raw ? i + 1 : i;
      const quote = source.startsWith('"""', start) ? '"""' : '"';
      let j = start + quote.length;
      let value = '';
      let closed = false;
      while (j < source.length) {
        const c = source[j];
        if (source.startsWith(quote, j)) { closed = true; j += quote.length; break; }
        if (!raw && c === '\\') {
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
      let j = i + 1;
      if (source[j] === '/') j++;
      for (;;) {
        const segment = SEGMENT.exec(source.slice(j));
        if (!segment) break;
        j += segment[0].length;
        if (source[j] !== '/') break;
        j++;
      }
      tokens.push({ type: 'ref', path: source.slice(i + 1, j) || '/' });
      i = j;
      continue;
    }
    const number = /^[+-.0-9]$/.test(ch) ? NUMBER.exec(source.slice(i)) : null;
    if (number) {
      const value = Number(number[0]);
      if (!Number.isFinite(value)) return null;
      tokens.push({ type: 'number', value });
      i += number[0].length;
      continue;
    }
    IDENTIFIER.lastIndex = i;
    const identifier = IDENTIFIER.exec(source);
    if (identifier) {
      tokens.push({ type: 'ident', value: identifier[0] });
      i = IDENTIFIER.lastIndex;
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

const ROOT_SCOPE: Scope = { path: [] };
const INVALID = Symbol('invalid Express');
const literal = (value: unknown): Expr => ({ type: 'literal', value });
const reserved = new Set(['true', 'false', 'null', '_']);

class Parser {
  private pos = 0;
  private readonly components = new Map<string, RawNode>();
  private readonly variables = new Map<string, Expr>();
  private data: Record<string, unknown> = Object.create(null);
  private modelSlots = MAX_SOURCE_LENGTH;

  constructor(private readonly tokens: Token[]) {}

  parse(): ExpressSurface | null {
    while (this.peek().type !== 'eof') this.statement();
    const raw = this.components.get('root');
    if (!raw) return null;
    const { components, variables, data } = this;
    const resolve = (model: Record<string, unknown>) => evaluator(model, variables, { remaining: 8192 });
    const render = (model: Record<string, unknown>) => {
      try {
        const budget = { remaining: 8192 };
        return resolveNode(raw, { model, components, variables, resolve: evaluator(model, variables, budget), budget }, ROOT_SCOPE, 0);
      } catch {
        return null;
      }
    };
    const root = render(data);
    return root && { root, data, render, resolve };
  }

  private peek(offset = 0): Token {
    return this.tokens[this.pos + offset] ?? EOF_TOKEN;
  }

  private at(value: string, offset = 0): boolean {
    const token = this.peek(offset);
    return token.type === 'punct' && token.value === value;
  }

  private take(value: string): boolean {
    if (!this.at(value)) return false;
    this.pos++;
    return true;
  }

  private expect(value: string): void {
    if (!this.take(value)) throw INVALID;
  }

  private statement(): void {
    const key = this.peek();
    this.pos++;
    this.expect('=');
    if (key.type === 'ref' && key.path.startsWith('/')) {
      const value = this.expression(0, false);
      if (value.type !== 'literal') throw INVALID;
      this.setData(bindingPath(key.path, ROOT_SCOPE), value.value);
      return;
    }
    if (key.type !== 'ident' || reserved.has(key.value) || key.value.startsWith('@')
      || this.components.has(key.value) || this.variables.has(key.value)) throw INVALID;
    const first = this.peek();
    if (first.type === 'ident' && this.at('(', 1) && Object.hasOwn(signatures, first.value)) {
      this.components.set(key.value, this.component(0));
    } else {
      this.variables.set(key.value, this.expression(0));
    }
  }

  private setData(path: string[], value: unknown): void {
    if (path.length === 0) {
      if (!value || typeof value !== 'object' || Array.isArray(value)) throw INVALID;
      this.data = value as Record<string, unknown>;
      return;
    }
    let current = this.data;
    for (const [index, key] of path.entries()) {
      if (Array.isArray(current)) {
        if (!ARRAY_INDEX.test(key) || Number(key) > 10000) throw INVALID;
        this.modelSlots -= Math.max(0, Number(key) - current.length + 1);
        if (this.modelSlots < 0) throw INVALID;
        while (current.length < Number(key)) current.push(null);
      }
      const next = path[index + 1];
      if (next === undefined) {
        current[key] = value;
      } else {
        current[key] ??= ARRAY_INDEX.test(next) && Number(next) <= 10000 ? [] : Object.create(null);
        if (typeof current[key] !== 'object') throw INVALID;
        current = current[key] as Record<string, unknown>;
      }
    }
  }

  private arguments(names: readonly string[], read: (key: string) => void, positional = names.length): void {
    this.expect('(');
    const seen = new Set<string>();
    let index = 0;
    while (!this.take(')')) {
      const t = this.peek();
      let key: string;
      if (t.type === 'ident' && this.at('=', 1)) {
        key = t.value;
        this.pos += 2;
      } else {
        while (seen.has(names[index])) index++;
        if (index >= positional) throw INVALID;
        key = names[index++];
      }
      if (!names.includes(key) || seen.has(key)) throw INVALID;
      seen.add(key);
      const arg = this.peek();
      if (arg.type === 'ident' && arg.value === '_') this.pos++;
      else read(key);
      if (!this.at(')')) this.expect(',');
    }
  }

  private component(depth: number): RawNode {
    if (depth > MAX_DEPTH) throw INVALID;
    const t = this.peek();
    if (t.type !== 'ident') throw INVALID;
    this.pos++;
    if (!this.at('(')) return { reference: t.value };
    if (!Object.hasOwn(signatures, t.value)) throw INVALID;
    const kind = t.value as ExpressKind;
    const children: Child[] = [];
    const props: Record<string, Expr> = Object.create(null);
    let event: EventAction | undefined;
    this.arguments([...signatures[kind], 'id'], key => {
      if (key === 'child' || key === 'children') children.push(...this.children(depth + 1));
      else if (key === 'action') {
        const action = this.peek();
        if (action.type !== 'ident') throw INVALID;
        this.pos++;
        if (action.value === 'Event') {
          const args = this.call(['name', 'context', 'userMessage'], depth + 1, 2);
          if (!args.name) throw INVALID;
          event = { name: args.name, context: args.context, userMessage: args.userMessage };
        } else if (action.value === 'openUrl') {
          const { url } = this.call(['url'], depth + 1);
          if (url) props.openUrl = url;
        } else {
          // Reject undeclared operations without evaluating or dispatching them.
          throw INVALID;
        }
      } else props[key] = this.expression(depth + 1);
    }, signatures[kind].length);
    if ((kind === 'Text' && !props.text) || (kind === 'Button' && children.length !== 1)) throw INVALID;
    return { kind, children, props, event };
  }

  private children(depth: number): Child[] {
    if (depth > MAX_DEPTH) throw INVALID;
    if (this.take('[')) {
      const children: Child[] = [];
      while (!this.take(']')) {
        children.push(...this.children(depth + 1));
        if (!this.at(']')) this.expect(',');
      }
      return children;
    }
    const token = this.peek();
    if (token.type === 'ident' && token.value === '_template') {
      this.pos++;
      this.expect('(');
      const path = this.peek();
      if (path.type !== 'ref') throw INVALID;
      this.pos++;
      this.expect(',');
      const template = this.component(depth + 1);
      this.take(',');
      this.expect(')');
      return [{ path: path.path, template }];
    }
    return [this.component(depth)];
  }

  private call(names: readonly string[], depth: number, positional?: number): Record<string, Expr> {
    const args: Record<string, Expr> = Object.create(null);
    this.arguments(names, key => { args[key] = this.expression(depth + 1); }, positional);
    return args;
  }

  private expression(depth: number, dynamic = true): Expr {
    if (depth > MAX_DEPTH) throw INVALID;
    const t = this.peek();
    if (dynamic && t.type === 'ref') {
      this.pos++;
      return t;
    }
    if (dynamic && t.type === 'ident' && !reserved.has(t.value)) {
      this.pos++;
      if (!this.at('(')) return { type: 'variable', name: t.value };
      if (!Object.hasOwn(functions, t.value)) throw INVALID;
      return { type: 'call', name: t.value, args: this.call(functions[t.value], depth + 1) };
    }
    if (this.take('[')) {
      const items: Expr[] = [];
      while (!this.take(']')) {
        items.push(this.expression(depth + 1, dynamic));
        if (!this.at(']')) this.expect(',');
      }
      return items.every(item => item.type === 'literal') ? literal(items.map(item => item.value)) : { type: 'list', items };
    }
    if (this.take('{')) {
      const entries: Record<string, Expr> = Object.create(null);
      while (!this.take('}')) {
        const key = this.key();
        this.expect(':');
        entries[key] = this.expression(depth + 1, dynamic);
        if (!this.at('}')) this.expect(',');
      }
      const pairs = Object.entries(entries);
      if (!pairs.every((pair): pair is [string, Extract<Expr, { type: 'literal' }>] => pair[1].type === 'literal')) return { type: 'record', entries };
      const value: Record<string, unknown> = Object.create(null);
      for (const [key, entry] of pairs) value[key] = entry.value;
      return literal(value);
    }
    this.pos++;
    if (t.type === 'string' || t.type === 'number') return literal(t.value);
    if (t.type === 'ident') {
      if (t.value === 'true') return literal(true);
      if (t.value === 'false') return literal(false);
      if (t.value === 'null') return literal(null);
    }
    throw INVALID;
  }

  private key(): string {
    const t = this.peek();
    if (t.type !== 'ident' && t.type !== 'string' && t.type !== 'number') throw INVALID;
    this.pos++;
    return String(t.value);
  }

}

interface Resolution {
  model: Record<string, unknown>;
  components: ReadonlyMap<string, RawNode>;
  variables: ReadonlyMap<string, Expr>;
  resolve: ReturnType<typeof evaluator>;
  budget: { remaining: number };
}

function resolveNode(raw: RawNode, state: Resolution, scope: Scope, depth: number): ExpressNode {
  const { model, components, resolve, budget } = state;
  if (depth > MAX_DEPTH || --budget.remaining < 0) throw INVALID;
  if ('reference' in raw) {
    const node = components.get(raw.reference);
    if (!node) throw INVALID;
    return resolveNode(node, state, scope, depth + 1);
  }
  const children: ExpressNode[] = [];
  for (const child of raw.children) {
    if ('template' in child) {
      const path = bindingPath(child.path, scope);
      const list = lookup(model, path);
      if (list === undefined || list === null) continue;
      if (!Array.isArray(list) || list.length > budget.remaining) throw INVALID;
      for (let index = 0; index < list.length; index++) {
        children.push(resolveNode(child.template, state, { path: [...path, String(index)], index }, depth + 1));
      }
    } else {
      children.push(resolveNode(child, state, scope, depth + 1));
    }
  }
  const props: Record<string, unknown> = Object.create(null);
  const bindings: Record<string, string[]> = Object.create(null);
  for (const [key, expr] of Object.entries(raw.props)) {
    props[key] = resolve(expr, scope);
    if (key === 'value') {
      // Resolving the property above already rejects circular variable aliases.
      let binding: Expr | undefined = expr;
      while (binding?.type === 'variable') binding = state.variables.get(binding.name);
      if (binding?.type === 'ref') bindings[key] = bindingPath(binding.path, scope);
    }
  }
  if (raw.kind === 'List' && Array.isArray(props.items)) {
    budget.remaining -= props.items.length;
    if (budget.remaining < 0) throw INVALID;
  }
  if (raw.kind === 'Table' || raw.kind === 'BarChart' || raw.kind === 'LineChart') {
    if (!Array.isArray(props.rows) || !props.rows.every(row => row && typeof row === 'object' && !Array.isArray(row))) throw INVALID;
    const strings = (value: unknown): value is string[] => Array.isArray(value) && value.every(item => typeof item === 'string');
    if (raw.kind === 'Table' ? props.columns !== undefined && !strings(props.columns)
      : typeof props.x !== 'string' || !(typeof props.y === 'string' || strings(props.y) && props.y.length)) throw INVALID;
    const fields = raw.kind === 'Table' ? (strings(props.columns) && props.columns.length
      || new Set(props.rows.flatMap(row => Object.keys(row))).size) : (strings(props.y) ? props.y.length : 1);
    budget.remaining -= props.rows.length * Math.max(fields, 1);
    if (budget.remaining < 0) throw INVALID;
  }
  return { kind: raw.kind, children, props, bindings, scope, event: raw.event };
}

export function parseExpress(source: string): ExpressSurface | null {
  const text = source.trim();
  const unwrapped = text.startsWith('<a2ui>') && text.endsWith('</a2ui>') ? text.slice(6, -7) : text;
  try {
    const tokens = tokenize(unwrapped);
    return tokens && new Parser(tokens).parse();
  } catch {
    return null;
  }
}

/** Only a control's resolved scalar binding is writable. */
export function writeBinding(data: Record<string, unknown>, path: string[], value: unknown): Record<string, unknown> {
  if (path.length === 0) return data;
  function write(parent: unknown, index: number): unknown {
    if (index === path.length) return value;
    const key = path[index];
    if (Array.isArray(parent)) {
      if (!ARRAY_INDEX.test(key) || Number(key) >= parent.length) return parent;
      const copy = parent.slice();
      copy[Number(key)] = write(parent[Number(key)], index + 1);
      return copy;
    }
    if (parent !== undefined && (parent === null || typeof parent !== 'object')) return parent;
    const copy = Object.assign(Object.create(null), parent);
    copy[key] = write(copy[key], index + 1);
    return copy;
  }
  return write(data, 0) as Record<string, unknown>;
}

export function eventMessage(node: ExpressNode, resolve: ReturnType<typeof evaluator>): string | undefined {
  const event = node.event;
  if (!event) return undefined;
  try {
    const name = resolve(event.name, node.scope);
    if (typeof name !== 'string' || !name.trim()) return undefined;
    const message = event.userMessage && scalarText(resolve(event.userMessage, node.scope)).trim();
    const context = !message && event.context && resolve(event.context, node.scope);
    const text = message || (context && typeof context === 'object' && !Array.isArray(context) && Object.keys(context).length
      ? name + '\n' + JSON.stringify(context) : name);
    return text.length <= 16384 ? text : undefined;
  } catch {
    return undefined;
  }
}
