import type { Block } from '../src/app/workflows/document/model';

const EMPTY_OBJECT = { type: 'object', properties: {}, additionalProperties: false } as const;

/** A root block that returns an empty object. */
export function rootBlock(): Block {
  return { input: { ...EMPTY_OBJECT }, output: { ...EMPTY_OBJECT }, nodes: { start: { type: 'start' }, done: { type: 'return', output: { type: 'literal', value: {} } } }, edges: [{ from: 'start', to: 'done' }] };
}
