import parse from 'shell-quote/parse';
import quote from 'shell-quote/quote';

export const formatArguments = (args: string[] = []) => quote(args);
/** Presentation only: native receives argv, never a shell command. */
export function parseArguments(text: string): string[] {
  const args = parse(text, () => { throw new Error('Quote literal variables'); })
    .map(arg => typeof arg !== 'string' && 'op' in arg && arg.op === 'glob' ? arg.pattern : arg);
  if (!args.every((arg): arg is string => typeof arg === 'string')) throw new Error('Quote literal shell syntax');
  return args;
}
export const formatReferences = (value: Record<string, string> = {}) => Object.keys(value).length ? JSON.stringify(value, null, 2) : '';
export function parseReferences(text: string): Record<string, string> {
  const value: unknown = text.trim() ? JSON.parse(text) : {};
  if (!value || typeof value !== 'object' || Array.isArray(value)
    || !Object.values(value).every(item => typeof item === 'string' && /^\$[A-Za-z_][A-Za-z0-9_]*$/.test(item))) throw new Error('Expected environment references');
  return value as Record<string, string>;
}
