// @vitest-environment node
import { expect, it } from 'vitest';
import { createWorkflowPort } from '../src/app/workflows/port';
import { WorkflowAnalysisSession, type AnalysisInput } from '../src/app/workflows/document/analysis';
import type { AppServerClient } from '../src/client/app-server';
import { withWorkflowDocument, type WorkflowRequest } from '../../protocol/app-server/workflow-documents';
import type { ProductHostWorkspaces } from '../src/workspaces/host';
import type { WorkspaceAuthorityObservation } from '../src/workspaces/authority';
import { exampleSource } from './workflow-fixtures';
import type { WorkflowFileDocument } from '../../protocol/app-server/v79';

function deferred() { let resolve!: () => void; const promise = new Promise<void>(accept => { resolve = accept; }); return { promise, resolve }; }

it.each((['user', 'workspace'] as const).flatMap(scope => (['save', 'latest query', 'authority'] as const).map(mode => ({ scope, mode }))))('skips superseded queued analyses before upload: $scope / $mode', async ({ scope, mode }) => {
  const entered = deferred(), release = deferred();
  let authorityRevision = 1, uploads = 0, analyses = 0, uploadedBytes = 0, live = 0;
  const calls: string[] = [];
  const source = exampleSource('typed_agent');
  const document: WorkflowFileDocument = { description: 'Queued generation', block: {
    input: { type: 'object', properties: {}, additionalProperties: false },
    nodes: { start: { type: 'start' }, large: { type: 'code', code_language: 'python3', code: "def main(): return {'value': 1}\n" + '# queued document\n'.repeat(65_536), variables: {}, outputs: { value: 'number' }, on_error: { type: 'stop' } }, done: { type: 'return', outputs: [] } },
    edges: [{ from: 'start', to: 'large' }, { from: 'large', to: 'done' }],
  } };
  const request = (async (operation: Parameters<WorkflowRequest>[0]) => {
    calls.push(operation.method);
    const params = 'params' in operation ? operation.params as Record<string, any> : {};
    switch (operation.method) {
      case 'workflow/documentBegin': live++; uploads++; expect(live).toBe(1); return { type: 'workflow_document', reference: { id: String(uploads), byte_length: params.byte_length, sha256: params.sha256 } };
      case 'workflow/documentWrite': uploadedBytes += Buffer.from(params.data, 'base64').length; return { type: 'workflow_document_accepted' };
      case 'workflow/documentFinish': return { type: 'workflow_document_accepted' };
      case 'workflow/documentRelease': live--; return { type: 'workflow_document_accepted' };
      case 'workflow/analyze':
        if (++analyses === 1) { entered.resolve(); await release.promise; }
        return { type: 'workflow_analysis', report: { target: params.target, id: params.id, generation: params.generation, query: params.query ?? null, candidate_revision: `sha256:${'a'.repeat(64)}`, analysis: { outputs: [], references: [], selected: null, diagnostic: null, incomplete: false, truncated: false }, presentation: [] } };
      case 'configuration/sourceWrite': return { type: 'sources_written', revision: 'saved' };
      case 'workflow/read': return { type: 'workflow_source', source: { ...source, document: null } };
      default: throw new Error(`Unexpected request ${operation.method}`);
    }
  }) as WorkflowRequest;
  const client = { getSnapshot: () => ({ endpoint: 'ws://fixture/', authorityRevision, authorityId: 'native' }), request } as unknown as AppServerClient;
  const nativeConnection = {};
  const target = { kind: 'workspace' as const, directory: '/fixture' };
  const host = { workflowWorkspace: async (_scope, _native, _id, operation) => {
    if (operation.kind === 'analyze') {
      const { report } = await withWorkflowDocument(nativeConnection, request, target, operation.document, reference => request({ method: 'workflow/analyze', params: { target, id: operation.id, document: reference, generation: operation.generation, query: operation.query } }, 'workflow_analysis'));
      return { kind: 'analyze', report };
    }
    if (operation.kind === 'write' && operation.edit.operation === 'replace') {
      await withWorkflowDocument(nativeConnection, request, target, operation.edit.document, reference => request({ method: 'configuration/sourceWrite', params: { target, expected_revision: operation.expected_revision, mutation: { kind: 'workflow', id: operation.id, edit: { operation: 'replace', document: reference } } } }, 'sources_written'));
      return { kind: 'write', commit: { acknowledgement: { revision: 'saved', configuration_revision: null }, reread: { status: 'observed', source } } };
    }
    throw new Error(`Unexpected Host operation ${operation.kind}`);
  } } as Pick<ProductHostWorkspaces, 'workflowWorkspace'> as ProductHostWorkspaces;
  const hostAuthority = { scope: { authorityId: 'host', endpoint: 'ws://fixture/' }, current: () => true } as WorkspaceAuthorityObservation;
  const makePort = () => createWorkflowPort({ client, host: () => host, hostAuthority, scope: scope === 'user' ? { kind: 'user' } : { kind: 'workspace', workspaceId: 'workspace' } });
  const port = makePort();
  const session = new WorkflowAnalysisSession();
  let current: AnalysisInput = { port, target: { endpoint: port.endpoint, scope: port.scope, id: 'flow' }, document, generation: 1 };
  const pending = [session.request(current, () => current)];
  await entered.promise;
  for (let generation = 2; generation <= 13; generation++) {
    session.cancel();
    current = { ...current, generation, document: { ...document, description: `Generation ${generation}` }, query: { consumer: { address: { block: [], node: 'done' }, field: { kind: 'return_output' }, path: [] }, search: `query ${generation}`, limit: 8 } };
    pending.push(session.request(current, () => current));
  }
  let completion: Promise<unknown>;
  if (mode === 'save') {
    session.cancel();
    completion = makePort().write('flow', source.revision, { operation: 'replace', document });
  } else if (mode === 'authority') {
    completion = port.write('flow', source.revision, { operation: 'replace', document }).then(() => { throw new Error('Obsolete queued write dispatched'); }, error => error);
    authorityRevision++;
    if (scope === 'user') live = 0; // Native connection retirement closes its connection-owned scratch.
    current = { ...current, port: { ...port } };
  } else completion = pending[pending.length - 1];
  release.resolve();
  const results = await Promise.all(pending);
  const completed = await completion;
  if (mode === 'authority') { expect(completed).toBeInstanceOf(Error); expect(calls).not.toContain('configuration/sourceWrite'); }
  expect(results.slice(0, mode === 'latest query' ? -1 : undefined).every(value => value === undefined)).toBe(true);
  expect(uploads).toBe(mode === 'authority' ? 1 : 2);
  expect(analyses).toBe(mode === 'latest query' ? 2 : 1);
  expect(uploadedBytes).toBeLessThan(3 * 1024 * 1024);
  expect(live).toBe(0);
  if (mode === 'save') expect(calls.filter(call => call === 'configuration/sourceWrite')).toHaveLength(1);
  if (mode === 'latest query') expect(results.at(-1)).toMatchObject({ generation: '13', query: { search: 'query 13' } });
});
