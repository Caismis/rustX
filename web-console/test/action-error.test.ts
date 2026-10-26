import { expect, it } from 'vitest';
import { actionError } from '../src/bindings/action-error';
import { OutcomeUncertain, RpcFailure } from '../src/client/errors';

it('action notices preserve native diagnostic text and semantic codes without wire JSON', () => {
  const failure = new RpcFailure({ code: -32000, message: 'Operation rejected', data: { kind: 'model_unavailable', diagnostic: 'Selected model is unavailable' } });
  expect(actionError(failure)).toBe('Selected model is unavailable (model_unavailable)');
  expect(failure.error.data).toEqual({ kind: 'model_unavailable', diagnostic: 'Selected model is unavailable' });
  expect(actionError(new RpcFailure({ code: -32000, message: 'Export failed', data: { kind: 'archive_preparation_failed', reason: 'busy' } }))).toBe('Export failed (archive_preparation_failed/busy)');
  expect(actionError(new Error('Request failed'))).toBe('Request failed');
  expect(actionError(new OutcomeUncertain())).toContain('Outcome uncertain');
});
