import type { Block } from '../src/app/workflows/document/model';

const EMPTY_OBJECT = { type: 'object', properties: {}, additionalProperties: false } as const;

/** A root block that returns an empty object. */
export function rootBlock(): Block {
  return { input: { ...EMPTY_OBJECT }, nodes: { start: { type: 'start' }, done: { type: 'return', outputs: []} }, edges: [{ from: 'start', to: 'done' }] };
}
