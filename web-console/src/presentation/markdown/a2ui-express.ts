import { isExpressNode, parseExpress as parseSyntax, type ExpressCallNode, type ExpressStatement, type ExpressValue } from './vendor/a2ui-express/visitor';
import { ARRAY_INDEX, MAX_DEPTH, MAX_SOURCE_LENGTH, argumentsOf, bindingPath, evaluator, functions, lookup, scalarText, type Scope } from './express-values';

const signatures = {
  Text: ['text', 'variant', 'weight', 'children', 'size'],
  Heading: ['text', 'eyebrow', 'subtitle', 'level'],
  Title: ['text', 'color', 'fontWeight', 'textAlign', 'tabularNums', 'size', 'strong'],
  Markdown: ['text', 'streaming'],
  Section: ['eyebrow', 'title', 'description', 'children'],
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
  AreaChart: ['title', 'subtitle', 'height', 'format', 'unit', 'data', 'stacked', 'highlight'],
  StockTicker: ['symbol', 'name', 'price', 'change', 'changePct', 'currency', 'series', 'stats', 'sample'],
  CandlestickChart: ['title', 'subtitle', 'height', 'format', 'unit', 'data'],
  KeyValueList: ['title', 'items', 'columns'],
  Alert: ['title', 'message', 'tone'],
  Stat: ['label', 'value', 'delta', 'trend', 'caption', 'tone', 'icon'],
  StatGroup: ['items'],
  Sparkline: ['label', 'values', 'value', 'delta', 'tone'],
} satisfies Record<string, readonly string[]>;

type SupportedKind = keyof typeof signatures;
export type ExpressKind = SupportedKind | 'Unsupported';
type EventAction = { name: ExpressValue; context?: ExpressValue; userMessage?: ExpressValue };
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

const ROOT_SCOPE: Scope = { path: [] };
const MAX_NODES = 8192;
const literalOnly = new Set(['true', 'false', 'null', '_']);

function literal(value: ExpressValue, depth = 0): unknown {
  if (depth > MAX_DEPTH) throw new Error('Express model is too deep');
  if (Array.isArray(value)) return value.map(item => literal(item, depth + 1));
  if (isExpressNode(value, 'map')) {
    const entries: Record<string, unknown> = Object.create(null);
    for (const [key, item] of Object.entries(value.entries)) entries[key] = literal(item, depth + 1);
    return entries;
  }
  if (value === null || typeof value !== 'object') return value;
  throw new Error('Express model assignments require literal values');
}

function buildProgram(statements: ExpressStatement[]) {
  const symbols = new Map<string, ExpressValue>();
  let data: Record<string, unknown> = Object.create(null);
  let modelSlots = MAX_SOURCE_LENGTH;
  for (const statement of statements) {
    if (statement[0] !== 'ASSIGN') throw new Error('Unsupported Express operation');
    const [, key, expr] = statement;
    if (!key.startsWith('$')) {
      if (symbols.has(key) || literalOnly.has(key)) throw new Error('Invalid Express declaration');
      symbols.set(key, expr);
      continue;
    }
    if (key !== '$' && !key.startsWith('$/')) throw new Error('Expected an absolute data path');
    const path = bindingPath(key.slice(1) || '/', ROOT_SCOPE);
    const value = literal(expr);
    if (path.length === 0) {
      if (!value || typeof value !== 'object' || Array.isArray(value)) throw new Error('Expected an object model');
      data = value as Record<string, unknown>;
      continue;
    }
    let current = data;
    for (const [index, segment] of path.entries()) {
      if (Array.isArray(current)) {
        if (!ARRAY_INDEX.test(segment) || Number(segment) > 10000) throw new Error('Invalid Express array index');
        modelSlots -= Math.max(0, Number(segment) - current.length + 1);
        if (modelSlots < 0) throw new Error('Express model limit');
        while (current.length < Number(segment)) current.push(null);
      }
      const next = path[index + 1];
      if (next === undefined) {
        current[segment] = value;
      } else {
        current[segment] ??= ARRAY_INDEX.test(next) && Number(next) <= 10000 ? [] : Object.create(null);
        if (typeof current[segment] !== 'object') throw new Error('Invalid Express model path');
        current = current[segment] as Record<string, unknown>;
      }
    }
  }
  return { symbols, data };
}

interface Resolution {
  model: Record<string, unknown>;
  symbols: ReadonlyMap<string, ExpressValue>;
  resolve: ReturnType<typeof evaluator>;
  budget: { remaining: number };
}

function consume(state: Resolution, depth: number): void {
  if (depth > MAX_DEPTH || --state.budget.remaining < 0) throw new Error('Express render limit');
}

function symbolOf(value: ExpressValue, state: Resolution): ExpressValue {
  if (!isExpressNode(value, 'variable')) return value;
  const symbol = state.symbols.get(value.name);
  if (symbol === undefined) throw new Error('Unknown Express component or value');
  return symbol;
}

function callSlot(value: ExpressValue): ExpressCallNode {
  if (!isExpressNode(value, 'call')) throw new Error('Expected an Express call');
  return value;
}

function appendChildren(value: ExpressValue, state: Resolution, scope: Scope, depth: number, children: ExpressNode[]): void {
  consume(state, depth);
  if (isExpressNode(value, 'variable')) {
    appendChildren(symbolOf(value, state), state, scope, depth + 1, children);
  } else if (Array.isArray(value)) {
    for (const item of value) appendChildren(item, state, scope, depth + 1, children);
  } else if (isExpressNode(value, 'call') && value.name === '_template') {
    const [pathArg, template] = value.args;
    if (!isExpressNode(pathArg, 'path') || template === undefined || value.args.length !== 2 || Object.keys(value.kwargs).length) {
      throw new Error('Invalid Express list template');
    }
    const path = bindingPath(pathArg.path, scope);
    const list = lookup(state.model, path);
    if (list === null || list === undefined) return;
    if (!Array.isArray(list) || list.length > state.budget.remaining) throw new Error('Invalid Express template collection');
    for (let index = 0; index < list.length; index++) {
      children.push(resolveNode(template, state, { path: [...path, String(index)], index }, depth + 1));
    }
  } else {
    children.push(resolveNode(value, state, scope, depth + 1));
  }
}

function unwrapVariable(value: ExpressValue | undefined, state: Resolution, depth: number): ExpressValue | undefined {
  let current = value;
  let hops = depth;
  while (isExpressNode(current, 'variable')) {
    consume(state, ++hops);
    current = symbolOf(current, state);
  }
  return current;
}

function isButtonTextArg(value: ExpressValue | undefined): boolean {
  return typeof value === 'string' || isExpressNode(value, 'path')
    || (isExpressNode(value, 'call') && Object.hasOwn(functions, value.name));
}

function resolveNode(value: ExpressValue, state: Resolution, scope: Scope, depth: number): ExpressNode {
  consume(state, depth);
  if (isExpressNode(value, 'variable')) return resolveNode(symbolOf(value, state), state, scope, depth + 1);
  const call = callSlot(value);
  const children: ExpressNode[] = [];
  const props: Record<string, unknown> = Object.create(null);
  const bindings: Record<string, string[]> = Object.create(null);
  let event: EventAction | undefined;
  if (!Object.hasOwn(signatures, call.name)) {
    if (Object.hasOwn(functions, call.name) || call.name === 'Event' || call.name === 'openUrl' || call.name === '_template') throw new Error('Expected an Express component');
    return { kind: 'Unsupported', children, props: { name: call.name }, bindings, scope };
  }
  const kind = call.name as SupportedKind;
  // Playground's Button(label, onClick) and the chat's child/variant/action
  // form share the same rendered action slot.
  const first = kind === 'Button' ? unwrapVariable(call.args[0], state, depth) : undefined;
  const second = kind === 'Button' ? unwrapVariable(call.args[1], state, depth) : undefined;
  const actionSecond = kind === 'Button' && isButtonTextArg(first) && isExpressNode(second, 'call');
  const playgroundChart = (kind === 'LineChart' || kind === 'BarChart')
    && (call.args.length >= 6 || Object.hasOwn(call.kwargs, 'data'));
  const chartArgs = ['title', 'subtitle', 'height', 'format', 'unit', 'data'];
  const parameters = actionSecond ? ['child', 'action'] : playgroundChart
    ? [...chartArgs, ...(kind === 'LineChart' ? ['curve', 'highlight'] : ['stacked', 'horizontal', 'highlight'])]
    : signatures[kind];
  const extra = kind === 'Button' ? ['id', 'label', 'text', 'onClick'] : ['id'];
  const args = argumentsOf(call, parameters, parameters.length, extra);
  if (Object.hasOwn(args, 'action') && Object.hasOwn(args, 'onClick')) throw new Error('Duplicate Express action');
  delete args.id;
  for (const [key, arg] of Object.entries(args)) {
    if (key === 'children' || key === 'child') {
      const target = kind === 'Button' ? unwrapVariable(arg, state, depth) : arg;
      if (kind === 'Button' && isButtonTextArg(target)) props.label = state.resolve(arg, scope);
      else appendChildren(arg, state, scope, depth + 1, children);
    } else if (key === 'action' || key === 'onClick') {
      const action = callSlot(unwrapVariable(arg, state, depth) ?? arg);
      if (action.name === 'Event') {
        const { name, context, userMessage } = argumentsOf(action, ['name', 'context', 'userMessage'], 2);
        if (name === undefined) throw new Error('Expected an Express event name');
        event = { name, context, userMessage };
      } else if (action.name === 'openUrl') {
        const { url } = argumentsOf(action, ['url']);
        if (url !== undefined) props.openUrl = state.resolve(url, scope);
      } else {
        throw new Error('Unsupported Express action');
      }
    } else {
      props[key] = state.resolve(arg, scope);
      if (key === 'value') {
        let binding = arg;
        // Value resolution above already bounds variable alias chains.
        while (isExpressNode(binding, 'variable')) binding = symbolOf(binding, state);
        if (isExpressNode(binding, 'path')) bindings[key] = bindingPath(binding.path, scope);
      }
    }
  }
  const buttonLabel = props.label ?? props.text;
  const hasButtonLabel = !!scalarText(buttonLabel).trim();
  if ((kind === 'Text' && !Object.hasOwn(args, 'text') && children.length === 0)
    || (kind === 'Button' && children.length !== 1 && !hasButtonLabel)) {
    throw new Error('Missing required Express argument');
  }
  const size = (value: unknown) => Array.isArray(value) ? value.length : 0;
  const { budget } = state;
  if (kind === 'List' || kind === 'KeyValueList' || kind === 'StatGroup') budget.remaining -= size(props.items);
  if (kind === 'ChoicePicker') budget.remaining -= size(props.options);
  if (kind === 'StockTicker') budget.remaining -= size(props.series) + size(props.stats);
  if (kind === 'CandlestickChart') budget.remaining -= size(props.data) * 3;
  if (kind === 'AreaChart' || playgroundChart) {
    // Series share a category axis. Bound both dimensions before pivoting rows.
    const records = Array.isArray(props.data) ? props.data : [];
    const series = new Set(records.map(row => row && typeof row === 'object' ? row.series : undefined));
    const categories = new Set(records.map(row => row && typeof row === 'object' ? row.x : undefined));
    budget.remaining -= Math.max(records.length, series.size * categories.size);
  }
  if (kind === 'Sparkline') budget.remaining -= size(props.values);
  if (budget.remaining < 0) throw new Error('Express chart budget');
  if (kind === 'Table' || !playgroundChart && (kind === 'BarChart' || kind === 'LineChart')) {
    if (!Array.isArray(props.rows) || !props.rows.every(row => row && typeof row === 'object' && !Array.isArray(row))) throw new Error('Invalid chart data or budget');
    const strings = (value: unknown): value is string[] => Array.isArray(value) && value.every(item => typeof item === 'string');
    if (kind === 'Table' ? props.columns !== undefined && !strings(props.columns)
      : typeof props.x !== 'string' || !(typeof props.y === 'string' || strings(props.y) && props.y.length)) throw new Error('Invalid chart data or budget');
    const fields = kind === 'Table' ? (strings(props.columns) && props.columns.length
      || new Set(props.rows.flatMap(row => Object.keys(row))).size) : (strings(props.y) ? props.y.length : 1);
    budget.remaining -= props.rows.length * Math.max(fields, 1);
    if (budget.remaining < 0) throw new Error('Invalid chart data or budget');
  }
  return { kind, children, props, bindings, scope, event };
}

export function parseExpress(source: string): ExpressSurface | null {
  const text = source.trim();
  const unwrapped = text.startsWith('<a2ui>') && text.endsWith('</a2ui>') ? text.slice(6, -7) : text;
  if (unwrapped.length > MAX_SOURCE_LENGTH) return null;
  try {
    const { errors, statements } = parseSyntax(unwrapped);
    if (errors.length || statements.length > MAX_NODES) return null;
    const { symbols, data } = buildProgram(statements);
    const entry = symbols.get('root');
    if (entry === undefined) return null;
    const render = (model: Record<string, unknown>) => {
      try {
        const budget = { remaining: MAX_NODES };
        return resolveNode(entry, { model, symbols, budget, resolve: evaluator(model, symbols, budget) }, ROOT_SCOPE, 0);
      } catch {
        return null;
      }
    };
    const root = render(data);
    return root && { root, data, render, resolve: model => evaluator(model, symbols, { remaining: MAX_NODES }) };
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
    const message = event.userMessage === undefined ? '' : scalarText(resolve(event.userMessage, node.scope)).trim();
    const context = !message && event.context !== undefined ? resolve(event.context, node.scope) : undefined;
    const text = message || (context && typeof context === 'object' && !Array.isArray(context) && Object.keys(context).length
      ? name + '\n' + JSON.stringify(context) : name);
    return text.length <= 16384 ? text : undefined;
  } catch {
    return undefined;
  }
}
