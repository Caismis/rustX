import { describe, expect, it } from 'vitest';
import { CommandRejected, renameAgentRoute, replaceNodeExits } from '../src/app/workflows/document/commands';
import { rootBlock } from './workflow-root';
import type { NodeDefinition, WorkflowDocument } from '../src/app/workflows/document/model';
import { portsOf } from '../src/app/workflows/document/ports';

type Agent = Extract<NodeDefinition, { type: 'agent' }>;
const agent = (extra: Partial<Agent> = {}): Agent => ({ type: 'agent', profile: 'reviewer', task: [{ type: 'literal', value: 'Triage.' }], output: { type: 'text' }, ...extra });
const routes = [{ id: 'approve', description: 'Ship it.' }, { id: 'revise', description: 'Needs edits.' }];

function document(node: Agent): WorkflowDocument {
  const block = rootBlock();
  block.nodes.triage = node;
  block.nodes.edit = { type: 'return', output: { type: 'literal', value: {} } };
  block.edges = [{ from: 'start', to: 'triage' }, { from: 'triage', to: 'done', port: 'approve' }, { from: 'triage', to: 'edit', port: 'revise' }, { from: 'triage', to: 'edit', port: 'fail' }];
  return { description: 'Routes.', block };
}

describe('Agent output routes, as Dify', () => {
  it('replaces next with one exit per route, plus fail under a fail branch', () => {
    expect(portsOf(agent())).toEqual(['next']);
    expect(portsOf(agent({ routes }))).toEqual(['approve', 'revise']);
    expect(portsOf(agent({ routes, on_error: { type: 'fail_branch' } }))).toEqual(['approve', 'revise', 'fail']);
    expect(portsOf(agent({ on_error: { type: 'fail_branch' } }))).toEqual(['next', 'fail']);
  });

  it('renames a route with its connections and refuses reserved or taken names', () => {
    const original = document(agent({ routes, on_error: { type: 'fail_branch' } }));
    const renamed = renameAgentRoute(original, [], 'triage', 'revise', 'rework');
    expect(renamed.block.nodes.triage).toMatchObject({ routes: [{ id: 'approve' }, { id: 'rework', description: 'Needs edits.' }] });
    expect(renamed.block.edges).toContainEqual({ from: 'triage', to: 'edit', port: 'rework' });
    expect(renamed.block.edges).toContainEqual({ from: 'triage', to: 'edit', port: 'fail' });
    for (const [to, code] of [['fail', 'reserved_port'], ['approve', 'duplicate_identifier'], ['two words', 'invalid_identifier']] as const) {
      expect(() => renameAgentRoute(original, [], 'triage', 'revise', to)).toThrow(new CommandRejected(code));
    }
  });

  it('drops only the connections whose exits went away', () => {
    const original = document(agent({ routes, on_error: { type: 'fail_branch' } }));
    const plain = replaceNodeExits(original, [], 'triage', agent());
    expect(plain.block.edges!.filter(edge => edge.from === 'triage')).toEqual([]);
    const one = replaceNodeExits(original, [], 'triage', agent({ routes: routes.slice(0, 1), on_error: { type: 'fail_branch' } }));
    expect(one.block.edges!.filter(edge => edge.from === 'triage')).toEqual([{ from: 'triage', to: 'done', port: 'approve' }, { from: 'triage', to: 'edit', port: 'fail' }]);
  });
});
