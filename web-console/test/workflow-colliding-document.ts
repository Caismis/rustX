import type { WorkflowDefinition, WorkflowSource } from '../../protocol/app-server/v57';
import { exampleSource } from './workflow-fixtures';

/** A complete, natively valid Workflow whose Return output holds both a
 * direct field named `a.fields.b` and a field `a` constructing `{ b }`:
 * two fields a dotted path would conflate. JSON is YAML, so the same text
 * is written as a native file by the browser acceptance test. */
export const COLLIDING_FIELDS: WorkflowDefinition = {
  description: 'Two object fields a dotted path would conflate.',
  block: {
    input: { type: 'object', properties: {}, additionalProperties: false },
    output: {
      type: 'object',
      properties: {
        'a.fields.b': { type: 'integer' },
        a: { type: 'object', properties: { b: { type: 'integer' } }, required: ['b'], additionalProperties: false },
      },
      required: ['a.fields.b', 'a'],
      additionalProperties: false,
    },
    entry: 'done',
    nodes: {
      done: {
        type: 'return',
        output: { type: 'object', fields: {
          'a.fields.b': { type: 'literal', value: 1 },
          a: { type: 'object', fields: { b: { type: 'literal', value: 2 } } },
        } },
      },
    },
    edges: [],
  },
} as WorkflowDefinition;

export function collidingFields(): WorkflowSource {
  return { ...exampleSource('parallel_review'), id: 'colliding', relative_path: '.agents/workflows/colliding.yaml', document: structuredClone(COLLIDING_FIELDS) };
}
