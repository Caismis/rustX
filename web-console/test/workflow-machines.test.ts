import { describe, expect, it } from 'vitest';
import { createActor } from 'xstate';
import { exampleSource } from './workflow-fixtures';
import type { WorkflowCheckReport, WorkflowDefinition, WorkflowEdit, WorkflowLibrary, WorkflowSource } from '../../protocol/app-server/v51';
import { RpcFailure } from '../src/client/errors';
import { WorkspaceHostError, type WorkflowCommit } from '../src/workspaces/host';
import { addNode, renameNode, replaceNode, setWorkflow } from '../src/app/workflows/document/commands';
import { sameDocument, ROOT } from '../src/app/workflows/document/model';
import { isDirty, workflowEditorMachine } from '../src/app/workflows/machines/editor';
import { workflowSurfaceMachine } from '../src/app/workflows/machines/surface';
import { scopeKey, USER_SCOPE, type WorkflowScope, type WorkflowScopePort } from '../src/app/workflows/port';

/** Every ordering below is established by a deferred promise the test
 * releases itself: no sleeps, timers or scheduling assumptions. */
function deferred<T>() {
  let resolve!: (value: T) => void;
  let reject!: (reason?: unknown) => void;
  const promise = new Promise<T>((res, rej) => { resolve = res; reject = rej; });
  promise.catch(() => {});
  return { promise, resolve, reject };
}
const flush = async () => { for (let turn = 0; turn < 16; turn++) await Promise.resolve(); };

function source(id: string, revision = 'sha256:base'): WorkflowSource {
  return { ...exampleSource('parallel_review'), id, revision, relative_path: `.agents/workflows/${id}.yaml` };
}
const listing = (scope: WorkflowScope, files: string[]): WorkflowLibrary => ({
  target: scope.kind === 'user' ? { kind: 'user' } : { kind: 'workspace', directory: `/${scope.workspaceId}` },
  directory: '/fixture', absent_revision: 'missing', diagnostic: null,
  files: files.map(id => ({ file_name: `${id}.yaml`, id, path: `/fixture/${id}.yaml`, relative_path: `.agents/workflows/${id}.yaml`, revision: `sha256:${id}`, byte_length: '1', modified_ms: null, description: id, nodes: 1, diagnostic: null, shadows_global: false })),
});
const valid: WorkflowCheckReport = { structure: { status: 'valid', outputs: {} }, dependencies: { status: 'not_checked' }, explanation: null };
const commit = (revision: string, reread: WorkflowCommit['reread']): WorkflowCommit => ({ acknowledgement: { revision, configuration_revision: '7' }, reread });

interface Scripted {
  port: WorkflowScopePort;
  lists: ReturnType<typeof deferred<WorkflowLibrary>>[];
  reads: { id: string; result: ReturnType<typeof deferred<WorkflowSource>> }[];
  checks: { id: string; document: WorkflowDefinition; result: ReturnType<typeof deferred<WorkflowCheckReport>> }[];
  writes: { id: string; expected: string; edit: WorkflowEdit; result: ReturnType<typeof deferred<WorkflowCommit>> }[];
}
function scripted(scope: WorkflowScope = USER_SCOPE): Scripted {
  const state: Omit<Scripted, 'port'> = { lists: [], reads: [], checks: [], writes: [] };
  return {
    ...state,
    port: {
      endpoint: 'ws://127.0.0.1:1/', scope,
      list: () => { const result = deferred<WorkflowLibrary>(); state.lists.push(result); return result.promise; },
      read: id => { const result = deferred<WorkflowSource>(); state.reads.push({ id, result }); return result.promise; },
      check: (id, document) => { const result = deferred<WorkflowCheckReport>(); state.checks.push({ id, document, result }); return result.promise; },
      write: (id, expected, edit) => { const result = deferred<WorkflowCommit>(); state.writes.push({ id, expected, edit, result }); return result.promise; },
    },
  };
}

async function readyEditor(script = scripted(), id = 'flow') {
  const actor = createActor(workflowEditorMachine, { input: { port: script.port, target: { endpoint: script.port.endpoint, scope: script.port.scope, id } } }).start();
  script.reads[0].result.resolve(source(id));
  await flush();
  expect(actor.getSnapshot().matches('ready')).toBe(true);
  return actor;
}
const describeAs = (text: string) => (document: WorkflowDefinition) => setWorkflow(document, { description: text, timeout_ms: document.timeout_ms });

describe('Workflow editor races', () => {
  it('never lets a check of generation N validate generation N+1', async () => {
    const script = scripted();
    const editor = await readyEditor(script);
    editor.send({ type: 'CHECK', explain: false });
    const checked = editor.getSnapshot().context.generation;
    editor.send({ type: 'EDIT', apply: describeAs('edited during check') });
    script.checks[0].result.resolve(valid);
    await flush();
    const { check, generation } = editor.getSnapshot().context;
    expect(check?.generation).toBe(checked);
    expect(generation).toBe(checked + 1);
    expect(sameDocument(script.checks[0].document, editor.getSnapshot().context.document)).toBe(false);
    // A newer check stops the older one: its late result has no path in.
    editor.send({ type: 'CHECK', explain: false });
    editor.send({ type: 'CHECK', explain: true });
    script.checks[2].result.resolve({ ...valid, dependencies: { status: 'admitted', context: '/w' } });
    script.checks[1].result.resolve(valid);
    await flush();
    expect(editor.getSnapshot().context.check).toMatchObject({ generation, explain: true, report: { dependencies: { status: 'admitted' } } });
    expect(script.checks).toHaveLength(3);
  });

  it('keeps generation N+1 dirty after generation N saves', async () => {
    const script = scripted();
    const editor = await readyEditor(script);
    editor.send({ type: 'EDIT', apply: describeAs('generation N') });
    const saved = editor.getSnapshot().context.document!;
    editor.send({ type: 'SAVE' });
    editor.send({ type: 'EDIT', apply: describeAs('generation N+1') });
    expect(script.writes).toHaveLength(1);
    expect(script.writes[0]).toMatchObject({ id: 'flow', expected: 'sha256:base', edit: { operation: 'replace' } });
    expect(sameDocument((script.writes[0].edit as { document: WorkflowDefinition }).document, saved)).toBe(true);
    script.writes[0].result.resolve(commit('sha256:n', { status: 'observed', source: { ...source('flow', 'sha256:n'), document: saved } }));
    await flush();
    const context = editor.getSnapshot().context;
    expect(context.base?.revision).toBe('sha256:n');
    expect(context.receipt).toMatchObject({ revision: 'sha256:n', confirmed: true });
    expect(context.document?.description).toBe('generation N+1');
    expect(isDirty(context)).toBe(true);
    // The next save is guarded by the committed revision.
    editor.send({ type: 'SAVE' });
    expect(script.writes[1].expected).toBe('sha256:n');
  });

  it('reports a committed save with a failed reread as committed and unconfirmed', async () => {
    const script = scripted();
    const editor = await readyEditor(script);
    editor.send({ type: 'EDIT', apply: describeAs('committed') });
    editor.send({ type: 'SAVE' });
    script.writes[0].result.resolve(commit('sha256:c', { status: 'failed', error: 'reread failed' }));
    await flush();
    const context = editor.getSnapshot().context;
    expect(context.notice).toEqual({ kind: 'committed_unconfirmed' });
    expect(context.receipt).toMatchObject({ revision: 'sha256:c', confirmed: false });
    expect(context.base?.revision).toBe('sha256:c');
    expect(isDirty(context)).toBe(false);
    expect(script.writes).toHaveLength(1);
  });

  it('reconciles an uncertain save by reading, never by resending it', async () => {
    const script = scripted();
    const editor = await readyEditor(script);
    editor.send({ type: 'EDIT', apply: describeAs('uncertain') });
    const attempted = editor.getSnapshot().context.document!;
    editor.send({ type: 'SAVE' });
    script.writes[0].result.reject(new WorkspaceHostError('acknowledgement lost', undefined, true));
    await flush();
    expect(editor.getSnapshot().context.notice).toMatchObject({ kind: 'failure', operation: 'save', failure: { kind: 'uncertain' } });
    editor.send({ type: 'SAVE' });
    expect(script.writes).toHaveLength(1);
    editor.send({ type: 'RECONCILE' });
    expect(script.reads).toHaveLength(2);
    script.reads[1].result.resolve({ ...source('flow', 'sha256:landed'), document: attempted });
    await flush();
    const context = editor.getSnapshot().context;
    expect(context.notice).toEqual({ kind: 'reconciled' });
    expect(context.base?.revision).toBe('sha256:landed');
    expect(isDirty(context)).toBe(false);
    expect(script.writes).toHaveLength(1);

    // An uncertain save that did not land is reported, and only then may save again.
    editor.send({ type: 'EDIT', apply: describeAs('second') });
    editor.send({ type: 'SAVE' });
    script.writes[1].result.reject(new RpcFailure({ code: -32000, message: 'unknown', data: { kind: 'outcome_unknown' } }));
    await flush();
    editor.send({ type: 'RECONCILE' });
    script.reads[2].result.resolve(source('flow', 'sha256:landed'));
    await flush();
    expect(editor.getSnapshot().context.notice).toEqual({ kind: 'not_committed' });
    editor.send({ type: 'SAVE' });
    expect(script.writes).toHaveLength(3);
  });

  it('preserves local edits on conflict and on authorization revocation', async () => {
    const script = scripted({ kind: 'workspace', workspaceId: 'registration-a' });
    const editor = await readyEditor(script);
    editor.send({ type: 'EDIT', apply: describeAs('local edit') });
    editor.send({ type: 'SAVE' });
    script.writes[0].result.reject(new WorkspaceHostError('source changed', 'source_conflict', false, { kind: 'source_conflict', scope: 'workspace', expected: 'sha256:base', actual: 'sha256:external' }));
    await flush();
    let context = editor.getSnapshot().context;
    expect(context.notice).toEqual({ kind: 'failure', operation: 'save', failure: { kind: 'conflict', expected: 'sha256:base', actual: 'sha256:external' } });
    expect(context.document?.description).toBe('local edit');
    expect(context.base?.revision).toBe('sha256:base');
    editor.send({ type: 'SAVE' });
    script.writes[1].result.reject(new WorkspaceHostError('Unknown Workspace registration', 'workspace_unavailable'));
    await flush();
    context = editor.getSnapshot().context;
    expect(context.notice).toMatchObject({ failure: { kind: 'unavailable' } });
    expect(context.document?.description).toBe('local edit');
    expect(isDirty(context)).toBe(true);
    expect(script.writes.map(write => write.id)).toEqual(['flow', 'flow']);
  });

  it('keeps outstanding operations bound to the document while navigating nested blocks', async () => {
    const script = scripted();
    const editor = await readyEditor(script);
    editor.send({ type: 'CHECK', explain: false });
    const quality = [{ node: 'reviews', branch: 'quality' }];
    editor.send({ type: 'NAVIGATE', block: quality });
    editor.send({ type: 'SAVE' });
    expect(editor.getSnapshot().context.block).toEqual(quality);
    script.writes[0].result.resolve(commit('sha256:s', { status: 'observed', source: source('flow', 'sha256:s') }));
    await flush();
    expect(editor.getSnapshot().context.block).toEqual(quality);
    // Renaming the owner of the open block returns to the nearest live block.
    editor.send({ type: 'EDIT', apply: document => renameNode(document, ROOT, 'reviews', 'audits') });
    expect(editor.getSnapshot().context.block).toEqual([]);
    editor.send({ type: 'NAVIGATE', block: [{ node: 'audits', branch: 'security' }] });
    expect(editor.getSnapshot().context.block).toEqual([{ node: 'audits', branch: 'security' }]);
    editor.send({ type: 'UNDO' });
    expect(editor.getSnapshot().context.block).toEqual([]);
  });

  it('coalesces one field session into one undo step and refuses invalid edits atomically', async () => {
    const editor = await readyEditor();
    const original = editor.getSnapshot().context.document!;
    for (const text of ['a', 'ab', 'abc']) editor.send({ type: 'EDIT', apply: describeAs(text), coalesce: 'description' });
    expect(editor.getSnapshot().context.past).toHaveLength(1);
    editor.send({ type: 'EDIT', apply: document => renameNode(document, ROOT, 'decision', 'ready') });
    expect(editor.getSnapshot().context.rejection).toBe('duplicate_identifier');
    expect(editor.getSnapshot().context.document?.description).toBe('abc');
    editor.send({ type: 'UNDO' });
    expect(sameDocument(editor.getSnapshot().context.document, original)).toBe(true);
    editor.send({ type: 'REDO' });
    expect(editor.getSnapshot().context.document?.description).toBe('abc');
  });

  it('sends nothing while a typed identity is still unchosen', async () => {
    const script = scripted();
    const editor = await readyEditor(script);
    editor.send({ type: 'EDIT', apply: document => addNode(document, ROOT, 'agent', 'draft_agent') });
    editor.send({ type: 'CHECK', explain: false });
    editor.send({ type: 'SAVE' });
    expect(editor.getSnapshot().context.notice).toEqual({ kind: 'incomplete', identities: [{ block: [], node: 'draft_agent', field: 'profile' }] });
    expect(script.checks).toHaveLength(0);
    expect(script.writes).toHaveLength(0);
    editor.send({ type: 'EDIT', apply: document => replaceNode(document, ROOT, 'draft_agent', { ...(document.block.nodes.draft_agent as Extract<WorkflowDefinition['block']['nodes'][string], { type: 'agent' }>), profile: 'reviewer' }) });
    editor.send({ type: 'SAVE' });
    expect(script.writes).toHaveLength(1);
  });

  it('presents an unparseable file without an editable document', async () => {
    const script = scripted();
    const editor = createActor(workflowEditorMachine, { input: { port: script.port, target: { endpoint: script.port.endpoint, scope: USER_SCOPE, id: 'broken' } } }).start();
    script.reads[0].result.resolve({ ...source('broken'), document: null, diagnostic: { path: '$', category: 'workflow_language', reason: 'invalid YAML', correction: null, detail: null, line: 1, column: 2 } });
    await flush();
    editor.send({ type: 'EDIT', apply: describeAs('nope') });
    editor.send({ type: 'SAVE' });
    expect(editor.getSnapshot().context.document).toBeUndefined();
    expect(script.writes).toHaveLength(0);
  });
});

describe('Workflow surface and library', () => {
  function surface() {
    const scripts = new Map<string, Scripted>();
    const ports = (scope: WorkflowScope) => {
      const script = scripted(scope);
      scripts.set(`${scopeKey(scope)}#${scripts.size}`, script);
      return script.port;
    };
    const actor = createActor(workflowSurfaceMachine, { input: { ports } }).start();
    // Nothing is listed until the library is shown.
    expect([...scripts.values()].every(script => script.lists.length === 0)).toBe(true);
    actor.getSnapshot().context.library.send({ type: 'ACTIVATE' });
    const script = (key: string) => [...scripts].filter(([name]) => name.startsWith(`${key}#`)).map(([, value]) => value);
    return { actor, script, library: () => actor.getSnapshot().context.library };
  }

  it('drops a superseded Workspace listing and lists exactly the selected scope', async () => {
    const { library, script } = surface();
    const a: WorkflowScope = { kind: 'workspace', workspaceId: 'a' }, b: WorkflowScope = { kind: 'workspace', workspaceId: 'b' };
    library().send({ type: 'SCOPE', scope: a });
    library().send({ type: 'SCOPE', scope: b });
    script('workspace:b')[0].lists[0].resolve(listing(b, ['beta']));
    script('workspace:a')[0].lists[0].resolve(listing(a, ['alpha']));
    script('user')[0].lists[0].resolve(listing(USER_SCOPE, ['global']));
    await flush();
    const context = library().getSnapshot().context;
    expect(context.scope).toEqual(b);
    expect(context.library?.files.map(file => file.id)).toEqual(['beta']);
    expect(script('workspace:a')[0].lists).toHaveLength(1);
  });

  it('creates the exact file, then opens exactly that file', async () => {
    const { actor, library, script } = surface();
    const scope: WorkflowScope = { kind: 'workspace', workspaceId: 'a' };
    library().send({ type: 'SCOPE', scope });
    const port = script('workspace:a')[0];
    port.lists[0].resolve(listing(scope, []));
    await flush();
    library().send({ type: 'OPERATE', operation: { kind: 'create', id: 'fresh', description: 'Fresh' } });
    expect(port.writes[0]).toMatchObject({ id: 'fresh', expected: 'missing', edit: { operation: 'create', description: 'Fresh' } });
    port.writes[0].result.resolve(commit('sha256:fresh', { status: 'observed', source: source('fresh', 'sha256:fresh') }));
    await flush();
    expect(actor.getSnapshot().matches('editor')).toBe(true);
    expect(actor.getSnapshot().context.editor!.getSnapshot().context.target).toEqual({ endpoint: 'ws://127.0.0.1:1/', scope, id: 'fresh' });
  });

  it('guards unsaved edits on Back and on leaving the route, and keeps the target fixed', async () => {
    const { actor, script } = surface();
    actor.send({ type: 'OPEN', scope: USER_SCOPE, id: 'global_flow' });
    const editor = actor.getSnapshot().context.editor!;
    const port = script('user')[1];
    port.reads[0].result.resolve(source('global_flow'));
    await flush();
    let left = 0;
    // Clean: leaving the route keeps the same editor and target for return.
    actor.send({ type: 'LEAVE', proceed: () => { left++; } });
    expect(left).toBe(1);
    expect(actor.getSnapshot().context.editor).toBe(editor);
    editor.send({ type: 'EDIT', apply: describeAs('unsaved') });
    actor.send({ type: 'LEAVE', proceed: () => { left++; } });
    expect(actor.getSnapshot().matches('confirming')).toBe(true);
    expect(left).toBe(1);
    actor.send({ type: 'CANCEL' });
    actor.send({ type: 'BACK' });
    expect(actor.getSnapshot().matches('confirming')).toBe(true);
    actor.send({ type: 'CANCEL' });
    expect(editor.getSnapshot().context.target).toEqual({ endpoint: 'ws://127.0.0.1:1/', scope: USER_SCOPE, id: 'global_flow' });
    expect(editor.getSnapshot().context.document?.description).toBe('unsaved');
    actor.send({ type: 'LEAVE', proceed: () => { left++; } });
    actor.send({ type: 'CONFIRM' });
    expect(left).toBe(2);
    expect(actor.getSnapshot().matches('library')).toBe(true);
    expect(editor.getSnapshot().status).toBe('stopped');
    expect(port.writes).toHaveLength(0);
  });

  it('never applies an obsolete file read to a newer editor', async () => {
    const { actor, script } = surface();
    actor.send({ type: 'OPEN', scope: USER_SCOPE, id: 'first' });
    actor.send({ type: 'BACK' });
    actor.send({ type: 'OPEN', scope: USER_SCOPE, id: 'second' });
    const reading = (id: string) => script('user').find(port => port.reads[0]?.id === id)!.reads[0].result;
    reading('second').resolve(source('second', 'sha256:second'));
    reading('first').resolve(source('first', 'sha256:first'));
    await flush();
    const context = actor.getSnapshot().context.editor!.getSnapshot().context;
    expect(context.target.id).toBe('second');
    expect(context.base?.revision).toBe('sha256:second');
  });
});
