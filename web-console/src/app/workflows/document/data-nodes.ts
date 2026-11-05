import type { WorkflowListOperator, WorkflowVarType } from '../../../../../protocol/app-server/v72';
import type { Value } from './model';
export type ItemKind = 'string' | 'number' | 'boolean' | 'object' | 'unknown';
export function itemKind(schema?: Record<string, unknown>): ItemKind {
  const type = schema?.type;
  return type === 'integer'
    ? 'number'
    : type === 'string' || type === 'number' || type === 'boolean' || type === 'object'
      ? type
      : 'unknown';
}
export function variableType(schema?: Record<string, unknown>): WorkflowVarType | undefined {
  if (schema?.type === 'array') {
    const kind = itemKind(schema.items as Record<string, unknown>);
    return kind === 'unknown' ? undefined : `array[${kind}]`;
  }
  const kind = itemKind(schema);
  return kind === 'unknown' ? undefined : kind;
}
export function listOperators(kind: ItemKind): WorkflowListOperator[] {
  return kind === 'number'
    ? ['equal', 'not_equal', 'greater', 'greater_equal', 'less', 'less_equal']
    : kind === 'boolean'
      ? ['equal', 'not_equal']
      : [
          'contains',
          'not_contains',
          'starts_with',
          'ends_with',
          'equal',
          'not_equal',
          'empty',
          'not_empty',
          'in',
          'not_in',
        ];
}
export const scalarValue = (kind: ItemKind): Value => ({ type: 'literal', value: kind === 'boolean' ? false : '' });
export function listFields(
  schema?: Record<string, unknown>,
  path: string[] = [],
): { path: string[]; kind: ItemKind }[] {
  const kind = itemKind(schema);
  if (kind !== 'object') return kind === 'unknown' ? [] : [{ path, kind }];
  const properties = schema?.properties as Record<string, Record<string, unknown>> | undefined,
    required = schema?.required as string[] | undefined;
  return Object.entries(properties ?? {})
    .filter(([name]) => required?.includes(name))
    .flatMap(([name, child]) => listFields(child, [...path, name]));
}
