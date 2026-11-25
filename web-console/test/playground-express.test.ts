import { describe, expect, it } from 'vitest';
import { createSurface } from '../src/presentation/markdown/PlaygroundExpress';
import catalog from '../src/presentation/markdown/vendor/playground/catalog.json';
import { playgroundUI } from '../src/presentation/markdown/vendor/playground/ui';
import samples from './fixtures/playground-express.json';

const cases = { ...samples.official, ...samples.additional };

it('covers every component in the published Playground catalog', () => {
  const expected = Object.keys(catalog.components).sort();
  expect(Object.keys(cases).sort()).toEqual(expected);
  expect([...playgroundUI.components.keys()].sort()).toEqual(expected);
});

describe('published Playground gallery', () => {
  it.each(Object.entries(cases))('compiles and applies the %s Express sample', (_name, source) => {
    const view = createSurface(source);
    expect('error' in view ? view.error : view.surface.clientRejected).toEqual([]);
  });
});
