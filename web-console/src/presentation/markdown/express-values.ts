import { isExpressNode, type ExpressCallNode, type ExpressValue } from './vendor/a2ui-express/visitor';
import {
  FormatCurrencyImplementation, FormatDateImplementation, FormatNumberImplementation, PluralizeImplementation,
} from '@a2ui/web_core/catalogs/basic/v1';
import { ExpressionParser } from '@a2ui/web_core/expressions';
import { DataContext } from '@a2ui/web_core/resolution';
import { DataModel } from '@a2ui/web_core/state';

export const MAX_SOURCE_LENGTH = 65536;
export const MAX_DEPTH = 128;
export const ARRAY_INDEX = /^(0|[1-9]\d*)$/;

export function argumentsOf(call: ExpressCallNode, names: readonly string[], positional = names.length, extras: readonly string[] = []): Record<string, ExpressValue> {
  if (call.args.length > positional) throw new Error('Too many Express arguments');
  const args: Record<string, ExpressValue> = Object.create(null);
  const seen = new Set<string>();
  function assign(name: string, value: ExpressValue) {
    if (seen.has(name)) throw new Error('Duplicate Express argument');
    seen.add(name);
    if (!isExpressNode(value, 'skipped')) args[name] = value;
  }
  call.args.forEach((value, index) => assign(names[index], value));
  for (const [name, value] of Object.entries(call.kwargs)) {
    if (!names.includes(name) && !extras.includes(name)) throw new Error('Unknown Express argument');
    assign(name, value);
  }
  return args;
}

export const functions: Readonly<Record<string, readonly string[]>> = {
  formatString: ['value'],
  formatNumber: ['value', 'decimals', 'grouping'],
  formatCurrency: ['value', 'currency', 'decimals', 'grouping'],
  formatDate: ['value', 'format'],
  pluralize: ['value', 'zero', 'one', 'two', 'few', 'many', 'other'],
  '@index': ['offset'],
};

const formatters = new Map([
  FormatNumberImplementation, FormatCurrencyImplementation, FormatDateImplementation, PluralizeImplementation,
].map(formatter => [formatter.name, formatter]));
// These four SDK functions consume resolved scalar arguments. Only our resolver
// can read surface data or select a function to invoke.
const formatContext = new DataContext(new DataModel(), '/');
const expressions = new ExpressionParser();
const parsedTemplates = new Map<string, ExpressValue[]>();

export interface Scope {
  path: string[];
  index?: number;
}

export function bindingPath(path: string, scope: Scope): string[] {
  const absolute = path.startsWith('/');
  const pointer = absolute ? path.slice(1) : path;
  if (/~(?![01])/.test(pointer)) throw new Error('Invalid Express path');
  const segments = pointer ? pointer.split('/').map(segment => segment.replaceAll('~1', '/').replaceAll('~0', '~')) : [];
  const result = absolute ? segments : [...scope.path, ...segments];
  if (result.length > MAX_DEPTH) throw new Error('Express path is too deep');
  return result;
}

export function lookup(data: Record<string, unknown>, path: readonly string[]): unknown {
  let current: unknown = data;
  for (const segment of path) {
    if (!current || typeof current !== 'object' || (Array.isArray(current) && !ARRAY_INDEX.test(segment))
      || !Object.hasOwn(current, segment)) return undefined;
    current = (current as Record<string, unknown>)[segment];
  }
  return current;
}

export function scalarText(value: unknown): string {
  return typeof value === 'string' || typeof value === 'number' || typeof value === 'boolean' ? String(value) : '';
}

type ParsePart = ReturnType<ExpressionParser['parseExpression']>;
function interpolationExpr(part: ParsePart, restore: (text: string) => string): ExpressValue {
  if (part && typeof part === 'object') {
    if ('path' in part && typeof part.path === 'string') return { kind: 'path', path: restore(part.path) };
    if ('call' in part && typeof part.call === 'string') {
      const args = Object.fromEntries(Object.entries(part.args ?? {}).map(([key, value]) => [restore(key), interpolationExpr(value as ParsePart, restore)]));
      return { kind: 'call', name: part.call, args: [], kwargs: args };
    }
  }
  return typeof part === 'string' ? restore(part) : typeof part === 'number' || typeof part === 'boolean' ? part : null;
}

function parseTemplate(text: string): ExpressValue[] {
  if (!/~[01]/.test(text)) return expressions.parse(text).map(part => interpolationExpr(part, value => value));
  // The SDK expression scanner omits the JSON Pointer escape character.
  // Protect the two RFC 6901 escapes while scanning, then restore them in the AST.
  let index = 0;
  while (text.includes(`__a2ui_pointer${index}__`)) index++;
  const marker = `__a2ui_pointer${index}__`;
  const restore = (value: string) => value.replaceAll(marker, '~');
  const protectedText = text.replace(/~(?=[01])/g, marker);
  return expressions.parse(protectedText).map(part => interpolationExpr(part, restore));
}

export function evaluator(data: Record<string, unknown>, variables: ReadonlyMap<string, ExpressValue>, budget: { remaining: number }) {
  let remainingText = MAX_SOURCE_LENGTH * 16;
  function evaluate(expr: ExpressValue, scope: Scope, depth = 0): unknown {
    if (depth > MAX_DEPTH || --budget.remaining < 0) throw new Error('Express evaluation limit');
    if (expr === null || typeof expr !== 'object') return expr;
    if (Array.isArray(expr)) return expr.map(item => evaluate(item, scope, depth + 1));
    switch (expr.kind) {
      case 'path': return lookup(data, bindingPath(expr.path, scope));
      case 'variable': {
        const value = variables.get(expr.name);
        if (value === undefined) throw new Error('Unknown Express variable');
        return evaluate(value, scope, depth + 1);
      }
      case 'map': return Object.fromEntries(Object.entries(expr.entries).map(([key, value]) => [key, evaluate(value, scope, depth + 1)]));
      case 'call': {
        const parameters = Object.hasOwn(functions, expr.name) ? functions[expr.name] : undefined;
        if (!parameters) throw new Error('Unsupported Express function');
        const args = Object.fromEntries(Object.entries(argumentsOf(expr, parameters)).map(([key, value]) => [key, evaluate(value, scope, depth + 1)]));
        if (expr.name === '@index') return scope.index === undefined ? undefined : scope.index + (typeof args.offset === 'number' ? args.offset : 0);
        if (expr.name === 'formatString') {
          if (typeof args.value !== 'string') return '';
          let parts = parsedTemplates.get(args.value);
          if (!parts) {
            parts = parseTemplate(args.value);
            if (parsedTemplates.size >= 32) parsedTemplates.clear();
            parsedTemplates.set(args.value, parts);
          }
          let result = '';
          for (const part of parts) {
            result += scalarText(evaluate(part, scope, depth + 1));
            if (result.length > MAX_SOURCE_LENGTH) throw new Error('Express format output limit');
          }
          remainingText -= result.length;
          if (remainingText < 0) throw new Error('Express format output limit');
          return result;
        }
        return formatters.get(expr.name)!.execute(args, formatContext);
      }
      default: throw new Error('Unsupported Express value');
    }
  }
  return evaluate;
}
