import { expect, it } from 'vitest';
import type { SourceSettings } from '../../protocol/app-server/v5';
import { observesSourceMutation } from '../src/app/settings/source-outcome';

it('requires exact User policy fields, distinguishes absent from explicit default, and refuses other mutation kinds', () => {
  const policy = { approval: 'never', execution: 'foreground_only', concurrency: 'sequential' } as const;
  const source: SourceSettings = {
    catalog: { valid: true, document: '/models', revision: 'c1', models: { models: [] }, providers: {} },
    user: { document: '/settings', revision: 'u1', active: true, authored: null },
    workspace: { document: '/workspace/rustx.toml', revision: 'w1', active: false, authored: null },
    resolution_available: true, provenance: {},
    integrations: { mcp_tool_policies: { exact: policy }, mcp: [], mcp_valid: false, user: {}, workspace: {}, prospective: {}, provenance: {}, inventory: null, agents: [] },
  };
  expect(observesSourceMutation(source, { kind: 'mcp_policy', id: 'exact', authored: policy })).toBe(true);
  for (const patch of [{ approval: 'always' }, { execution: 'background_only' }, { concurrency: 'parallel' }] as const) {
    expect(observesSourceMutation(source, { kind: 'mcp_policy', id: 'exact', authored: { ...policy, ...patch } })).toBe(false);
  }
  expect(observesSourceMutation(source, { kind: 'mcp_policy', id: 'exact', authored: null })).toBe(false);
  expect(observesSourceMutation(source, { kind: 'mcp_policy', id: 'absent', authored: policy })).toBe(false);
  expect(observesSourceMutation(source, { kind: 'mcp_policy', id: 'absent', authored: null })).toBe(true);
  expect(observesSourceMutation(source, { kind: 'user_model', authored: null })).toBe(false);
});
