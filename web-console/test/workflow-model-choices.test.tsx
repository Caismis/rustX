import { act, cleanup, renderHook } from '@testing-library/react';
import { afterEach, expect, it } from 'vitest';
import type { WorkflowModelChoices, WorkflowModelChoicesQuery } from '../../protocol/app-server/v57';
import type { WorkflowScopePort } from '../src/app/workflows/port';
import { useModelChoices } from '../src/app/workflows/ui/editor/forms/ModelSelection';

afterEach(cleanup);
function deferred<T>() {
  let resolve!: (value: T) => void;
  let reject!: (error: unknown) => void;
  const promise = new Promise<T>((accept, refuse) => { resolve = accept; reject = refuse; });
  return { promise, resolve, reject };
}
function scripted() {
  const requests: { query: WorkflowModelChoicesQuery; expected?: string; result: ReturnType<typeof deferred<WorkflowModelChoices>> }[] = [];
  const port: WorkflowScopePort = {
    endpoint: 'ws://fixture/', scope: { kind: 'user' }, list: () => new Promise(() => {}), read: () => new Promise(() => {}),
    check: () => new Promise(() => {}), write: () => new Promise(() => {}),
    modelChoices: (query, expected) => { const result = deferred<WorkflowModelChoices>(); requests.push({ query, expected, result }); return result.promise; },
  };
  return { port, requests };
}
const page = (models: string[], next_offset: number | null = null, revision = 'catalog-1'): WorkflowModelChoices => ({ type: 'models', revision, models, next_offset });

it('pages exactly one bounded native choice page and fences every continuation by its revision', async () => {
  const source = scripted();
  const view = renderHook(() => useModelChoices(source.port, { type: 'models' }));
  expect(source.requests[0].query).toEqual({ type: 'models', offset: 0 });
  const first = Array.from({ length: 32 }, (_, index) => `model-${index}`);
  await act(async () => source.requests[0].result.resolve(page(first, 32)));
  act(() => view.result.current.load(32, 'catalog-1'));
  expect(source.requests[1]).toMatchObject({ query: { type: 'models', offset: 32 }, expected: 'catalog-1' });
  await act(async () => source.requests[1].result.resolve(page(['model-32'])));
  expect(view.result.current.state?.choices).toEqual(page(['model-32']));
  // Prior pages are not accumulated into another unbounded catalog.
  expect(JSON.stringify(view.result.current.state?.choices)).not.toContain('model-0');
});

it('discards late responses after a source scope/port change', async () => {
  const old = scripted(), current = scripted();
  const view = renderHook(({ port }) => useModelChoices(port, { type: 'models' }), { initialProps: { port: old.port } });
  view.rerender({ port: current.port });
  await act(async () => current.requests[0].result.resolve(page(['current-model'])));
  await act(async () => old.requests[0].result.resolve(page(['wrong-source-model'])));
  expect(view.result.current.state?.choices).toEqual(page(['current-model']));
});

it('discards late profile pages belonging to a different selected Model', async () => {
  const source = scripted();
  const view = renderHook(({ model }) => useModelChoices(source.port, { type: 'profiles', model }), { initialProps: { model: 'first' } });
  view.rerender({ model: 'second' });
  await act(async () => source.requests[1].result.resolve({ type: 'profiles', model: 'second', revision: 'catalog-1', profiles: ['current-profile'], next_offset: null }));
  await act(async () => source.requests[0].result.resolve({ type: 'profiles', model: 'first', revision: 'catalog-1', profiles: ['wrong-profile'], next_offset: null }));
  expect(view.result.current.state?.choices).toMatchObject({ model: 'second', profiles: ['current-profile'] });
});

it('fails closed on a changed revision and restarts explicitly from the first page', async () => {
  const source = scripted();
  const view = renderHook(() => useModelChoices(source.port, { type: 'models' }));
  await act(async () => source.requests[0].result.resolve(page(Array.from({ length: 32 }, (_, index) => `model-${index}`), 32)));
  act(() => view.result.current.load(32, 'catalog-1'));
  await act(async () => source.requests[1].result.resolve(page(['replacement'], null, 'catalog-2')));
  expect(view.result.current.state).toMatchObject({ loading: false, failed: true });
  expect(view.result.current.state?.choices).toBeUndefined();
  act(() => view.result.current.load());
  expect(source.requests[2]).toMatchObject({ query: { type: 'models', offset: 0 }, expected: undefined });
  await act(async () => source.requests[2].result.resolve(page(['replacement'], null, 'catalog-2')));
  expect(view.result.current.state?.choices).toEqual(page(['replacement'], null, 'catalog-2'));
});
