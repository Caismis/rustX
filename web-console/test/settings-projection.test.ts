// @vitest-environment node
import { expect, it } from 'vitest';
import type { RuntimeLayer, SourceMutation } from '../../protocol/app-server/v17';
import {
  applicationScope, authoredUnit, changeBehavior, changeBehaviorLabel, observedResult, observedResultLabel,
  provenanceLabel, settingsLifecycle, settingsTargetKey, settingsTargetLabel, unitApplication, unitFacts,
  userSettingsTarget, workspaceSettingsTarget,
} from '../src/app/settings/projection';
import { cfg3Application, cfg3Source } from './cfg3-data';

const tools: SourceMutation = { kind: 'config', mutation: { unit: 'native_tools', authored: [] } };

it('S1-03 absent, false, empty list, empty object and explicit values are distinct authored facts', () => {
  expect(unitFacts(cfg3Source(), 'workspace', tools).presence).toBe('absent');
  for (const authored of [[], false, {}, ['read']]) {
    const source = cfg3Source();
    source.workspace!.authored = { agent: { tools: { builtin: authored as never } } };
    expect(unitFacts(source, 'workspace', tools).presence).toBe('authored');
  }
  // An authored document that simply omits the unit is absent, never materialized.
  const omitted = cfg3Source();
  omitted.workspace!.authored = { agent: { tools: {} } };
  expect(unitFacts(omitted, 'workspace', tools).presence).toBe('absent');
});

it('S1-03 native effective value and provenance are projected, never recomputed in TypeScript', () => {
  const source = cfg3Source();
  source.resolved = { agent: { tools: { builtin: ['read', 'glob'] } } } as RuntimeLayer;
  source.provenance = { 'agent.tools.builtin': { kind: 'user', document: '/bound/rustx.toml', base: '/bound' } };
  const facts = unitFacts(source, 'workspace', tools);
  expect(facts.presence).toBe('absent');
  expect(facts.effective).toEqual(['read', 'glob']);
  expect(provenanceLabel(facts.origin)).toBe('Inherited from User');
  expect(provenanceLabel({ kind: 'builtin' })).toBe('Native default');
  expect(provenanceLabel({ kind: 'workspace', document: '/workspace/rustx.toml', base: '/workspace' })).toBe('Workspace override');
  expect(provenanceLabel({ kind: 'process', base: '/run' })).toBe('Process default');
});

it('S1-03 invalid and unavailable are neither absent nor empty', () => {
  const invalid = cfg3Source();
  invalid.workspace = { path: '/workspace/rustx.toml', revision: 'workspace-1', authored: null, diagnostic: 'invalid rustx.toml' };
  const facts = unitFacts(invalid, 'workspace', tools);
  expect(facts.presence).toBe('invalid');
  expect(facts.diagnostic).toBe('invalid rustx.toml');
  expect(unitFacts(undefined, 'workspace', tools).presence).toBe('unavailable');
});

it('S1-01/S1-02 entry ownership is named by the exact target, never by a selector value', () => {
  expect(settingsTargetLabel(userSettingsTarget)).toBe('User Settings');
  expect(settingsTargetLabel(workspaceSettingsTarget('a', 'Workspace A'))).toBe('Workspace Settings — Workspace A');
  expect(settingsTargetKey(userSettingsTarget)).toBe('user');
  expect(settingsTargetKey(workspaceSettingsTarget('a', 'A'))).toBe('workspace:a');
  expect(applicationScope({ kind: 'user' })).toBe('source:user');
  expect(applicationScope({ kind: 'workspace', directory: '/w' })).toBe('source:workspace:/w');
  expect(authoredUnit({ agent: { tools: { builtin: ['read', 'glob'] } } }, { kind: 'config', mutation: { unit: 'native_tools', authored: null } })).toEqual(['read', 'glob']);
});

it('S1-08 native per-unit observations stay independent and never pose as a classification', () => {
  const application = cfg3Application();
  application.units = { capabilities: { status: 'preparing' }, instructions: { status: 'applied' }, provider: { status: 'failed', diagnostic: 'resource failed' }, process_bindings: { status: 'process_restart' }, shared_capacity: { status: 'ready', impact: 'unproven' } };
  expect(observedResultLabel(observedResult(unitApplication(application, 'capabilities')))).toBe('Preparing');
  expect(observedResultLabel(observedResult(unitApplication(application, 'instructions')))).toBe('Applied');
  expect(observedResult(unitApplication(application, 'provider'))).toEqual({ state: 'failed', diagnostic: 'resource failed' });
  expect(observedResultLabel(observedResult(unitApplication(application, 'process_bindings')))).toBe('Restart pending');
  expect(observedResult(unitApplication(application, 'execution_policy'))).toEqual({ state: 'unavailable' });
  expect(changeBehaviorLabel(changeBehavior({ max_connections: 'hot', shutdown_deadline_ms: 'restart' }, 'max_connections'))).toBe('Applies immediately');
  expect(changeBehaviorLabel(changeBehavior({ shutdown_deadline_ms: 'restart' }, 'shutdown_deadline_ms'))).toBe('Requires App Server restart');
});

it('S1-07 connecting, loading, ready, stale and failed are distinct lifecycle states', () => {
  expect(settingsLifecycle({ connection: 'connecting', hasSource: false, targetValid: false, readError: '' })).toBe('connecting');
  expect(settingsLifecycle({ connection: 'connected', hasSource: false, targetValid: false, readError: '' })).toBe('loading');
  expect(settingsLifecycle({ connection: 'connected', hasSource: true, targetValid: true, readError: '' })).toBe('ready');
  expect(settingsLifecycle({ connection: 'connected', hasSource: true, targetValid: false, readError: 'read failed' })).toBe('stale');
  expect(settingsLifecycle({ connection: 'connected', hasSource: false, targetValid: false, readError: 'read failed' })).toBe('failed');
  expect(settingsLifecycle({ connection: 'error', hasSource: true, targetValid: true, readError: '' })).toBe('failed');
});
