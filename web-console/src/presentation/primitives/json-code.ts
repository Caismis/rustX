import { applyEdits, format, visit } from 'jsonc-parser';

export interface JsonSyntaxError { from: number; to: number; line: number; column: number }

/** Strict JSON diagnostics never include authored keys or values. */
export function jsonSyntaxErrors(text: string, allowEmpty = true): JsonSyntaxError[] {
  if (allowEmpty && !text.trim()) return [];
  const errors: JsonSyntaxError[] = [];
  visit(text, { onError: (_code, offset, length, line, column) => {
    errors.push({ from: offset, to: Math.min(text.length, offset + length), line: line + 1, column: column + 1 });
  } }, { disallowComments: true, allowTrailingComma: false, allowEmptyContent: false });
  return errors;
}

/** Apply whitespace edits to the original tokens, without parsing/stringifying
 * away repeated keys, numeric precision, escape spelling or key order. */
export function formatJsonCode(text: string): string {
  if (!text.trim() || jsonSyntaxErrors(text).length) return text;
  return applyEdits(text, format(text, undefined, { tabSize: 2, insertSpaces: true, eol: '\n', keepLines: false }));
}
