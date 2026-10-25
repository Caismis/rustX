import { describe, expect, it } from 'vitest';
import { createActor } from 'xstate';
import { exampleSource } from './workflow-fixtures';
import type { WorkflowCheckReport, WorkflowDefinition, WorkflowEdit, WorkflowLibrary, WorkflowSource } from '../../protocol/app-server/v57';
import { RpcFailure } from '../src/client/errors';
import { WorkspaceHostError, type WorkflowCommit } from '../src/workspaces/host';
import { addNode, deleteNodes, renameNode, replaceNode, setWorkflow } from '../src/app/workflows/document/commands';
import { blockAt, type BlockPath } from '../src/app/workflows/document/model';
import { blockField, encodeField, fieldText, nodeField, setField, updateField, within } from '../src/app/workflows/document/fields';
import { readJson } from '../src/app/workflows/ui/editor/forms/drafts';
import { editorHolds } from '../src/app/workflows/machines/surface';
import { sameDocument, ROOT } from '../src/app/workflows/document/model';
import { isDirty, workflowEditorMachine, type FieldCommand } from '../src/app/workflows/machines/editor';
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
  files: files.map(id => ({ file_name: `${id}.yaml`, id, relative_path: `.agents/workflows/${id}.yaml`, revision: `sha256:${id}`, byte_length: '1', modified_ms: null, description: id, nodes: 1, diagnostic: null, shadows_global: false })),
});
const valid: WorkflowCheckReport = { structure: { status: 'valid', outputs: {}, revision: 'sha256:candidate' }, dependencies: { status: 'not_checked' }, explanation: null };
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
      modelChoices: () => new Promise(() => {}), list: () => { const result = deferred<WorkflowLibrary>(); state.lists.push(result); return result.promise; },
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
    // Native names the revision the attempted candidate commits as.
    expect(sameDocument(script.checks[0].document, attempted)).toBe(true);
    script.checks[0].result.resolve({ ...valid, structure: { status: 'valid', outputs: {}, revision: 'sha256:landed' } });
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

  it('recognizes its own native-normalized commit after a lost acknowledgement and keeps the later generation', async () => {
    const script = scripted();
    const editor = await readyEditor(script);
    // Generation N omits the timeout; native writes the default explicitly
    // and every ordinary edge with an explicit null port.
    editor.send({ type: 'EDIT', apply: document => setWorkflow(document, { description: 'generation N', timeout_ms: undefined }) });
    const attempted = editor.getSnapshot().context.document!;
    expect(attempted.timeout_ms).toBeUndefined();
    editor.send({ type: 'SAVE' });
    editor.send({ type: 'EDIT', apply: describeAs('generation N+1') });
    const later = editor.getSnapshot().context.document!;
    script.writes[0].result.reject(new WorkspaceHostError('acknowledgement lost', undefined, true));
    await flush();
    editor.send({ type: 'RECONCILE' });
    const normalized = { ...attempted, timeout_ms: '600000', block: { ...attempted.block, edges: (attempted.block.edges ?? []).map(edge => ({ port: null, ...edge })) } };
    expect(sameDocument(normalized as typeof attempted, attempted)).toBe(false);
    script.reads[1].result.resolve({ ...source('flow', 'sha256:n'), document: normalized as WorkflowSource['document'] });
    await flush();
    expect(sameDocument(script.checks[0].document, attempted)).toBe(true);
    script.checks[0].result.resolve({ ...valid, structure: { status: 'valid', outputs: {}, revision: 'sha256:n' } });
    await flush();
    const context = editor.getSnapshot().context;
    expect(context.notice).toEqual({ kind: 'reconciled' });
    expect(context.base?.revision).toBe('sha256:n');
    expect(context.receipt).toMatchObject({ revision: 'sha256:n', generation: context.generation - 1, confirmed: true });
    expect(context.document).toBe(later);
    expect(isDirty(context)).toBe(true);
    expect(script.writes).toHaveLength(1);
    // The next write is guarded by the committed revision, not the old base.
    editor.send({ type: 'SAVE' });
    expect(script.writes[1].expected).toBe('sha256:n');
  });

  it('reports a revision whose candidate identity differs as an external change', async () => {
    const script = scripted();
    const editor = await readyEditor(script);
    editor.send({ type: 'EDIT', apply: describeAs('mine') });
    editor.send({ type: 'SAVE' });
    script.writes[0].result.reject(new WorkspaceHostError('acknowledgement lost', undefined, true));
    await flush();
    editor.send({ type: 'RECONCILE' });
    script.reads[1].result.resolve(source('flow', 'sha256:theirs'));
    await flush();
    script.checks[0].result.resolve({ ...valid, structure: { status: 'valid', outputs: {}, revision: 'sha256:mine' } });
    await flush();
    const context = editor.getSnapshot().context;
    expect(context.notice).toEqual({ kind: 'changed_externally', revision: 'sha256:theirs' });
    expect(context.base?.revision).toBe('sha256:base');
  });

  it('keeps unapplied field text dirty and never writes while it cannot be applied', async () => {
    const script = scripted();
    const editor = await readyEditor(script);
    editor.send({ type: 'DRAFT', address: blockField(ROOT, ['input']), text: '{ "type": ' });
    const context = editor.getSnapshot().context;
    expect(isDirty(context)).toBe(true);
    editor.send({ type: 'SAVE' });
    editor.send({ type: 'CHECK', explain: false });
    expect(script.writes).toHaveLength(0);
    expect(script.checks).toHaveLength(0);
    expect(editor.getSnapshot().context.notice).toEqual({ kind: 'pending_fields', count: 1 });
    expect(editor.getSnapshot().context.drafts[encodeField(blockField(ROOT, ['input']))].text).toBe('{ "type": ');
    // Discarding is explicit; then the document is clean and saves again.
    editor.send({ type: 'DISCARD_DRAFTS' });
    expect(isDirty(editor.getSnapshot().context)).toBe(false);
  });

  it('guards leaving an editor whose only change is unapplied field text', async () => {
    const script = scripted();
    const actor = createActor(workflowSurfaceMachine, { input: { ports: () => script.port } }).start();
    actor.send({ type: 'OPEN', id: 'flow', port: script.port });
    script.reads[0].result.resolve(source('flow'));
    await flush();
    const editor = actor.getSnapshot().context.editor!;
    editor.send({ type: 'DRAFT', address: blockField(ROOT, ['output']), text: '{}' });
    expect(editorHolds(editor)).toBe(true);
    let left = false;
    actor.send({ type: 'LEAVE', proceed: () => { left = true; } });
    expect(actor.getSnapshot().matches('confirming')).toBe(true);
    expect(left).toBe(false);
  });

  it('undoes only the latest edit of the branch being edited, never a same-named node elsewhere', async () => {
    const editor = await readyEditor();
    const quality: BlockPath = [{ node: 'reviews', branch: 'quality' }], security: BlockPath = [{ node: 'reviews', branch: 'security' }];
    const retarget = (path: BlockPath, component: string) => (document: WorkflowDefinition) => {
      const done = blockAt(document, path)!.nodes.done;
      return replaceNode(document, path, 'done', { ...done, output: { type: 'reference', path: [component] } } as typeof done);
    };
    editor.send({ type: 'NAVIGATE', block: quality });
    editor.send({ type: 'EDIT', apply: retarget(quality, 'quality_edit'), coalesce: encodeField(nodeField(quality, 'done', ['output'])) });
    editor.send({ type: 'NAVIGATE', block: security });
    editor.send({ type: 'EDIT', apply: retarget(security, 'security_edit'), coalesce: encodeField(nodeField(security, 'done', ['output'])) });
    expect(encodeField(nodeField(quality, 'done', ['output']))).not.toBe(encodeField(nodeField(security, 'done', ['output'])));
    editor.send({ type: 'UNDO' });
    const document = editor.getSnapshot().context.document!;
    expect(blockAt(document, quality)!.nodes.done).toMatchObject({ output: { path: ['quality_edit'] } });
    expect(blockAt(document, security)!.nodes.done).not.toMatchObject({ output: { path: ['security_edit'] } });
  });

  it('ends a coalesced session at navigation, a selection change and a save, but not within one field session', async () => {
    const script = scripted();
    const editor = await readyEditor(script);
    const key = encodeField(nodeField(ROOT, 'decision', ['condition']));
    const past = () => editor.getSnapshot().context.past.length;
    editor.send({ type: 'EDIT', apply: describeAs('a'), coalesce: key });
    editor.send({ type: 'EDIT', apply: describeAs('ab'), coalesce: key });
    expect(past()).toBe(1);
    editor.send({ type: 'BOUNDARY' });
    editor.send({ type: 'EDIT', apply: describeAs('abc'), coalesce: key });
    expect(past()).toBe(2);
    editor.send({ type: 'NAVIGATE', block: ROOT });
    editor.send({ type: 'EDIT', apply: describeAs('abcd'), coalesce: key });
    expect(past()).toBe(3);
    editor.send({ type: 'SAVE' });
    editor.send({ type: 'EDIT', apply: describeAs('abcde'), coalesce: key });
    expect(past()).toBe(4);
  });

  it('undoes a completed node move before an older semantic edit, and redoes both', async () => {
    const script = scripted();
    const editor = await readyEditor(script);
    editor.send({ type: 'EDIT', apply: describeAs('semantic') });
    const semantic = editor.getSnapshot().context.document!;
    const before = { decision: { x: 0, y: 0 } }, after = { decision: { x: 240, y: 80 } };
    editor.send({ type: 'MOVE', block: 'root', path: ROOT, before, after });
    // A move is no semantic change: generation and dirtiness are unchanged.
    const generation = editor.getSnapshot().context.generation;
    editor.send({ type: 'UNDO' });
    let context = editor.getSnapshot().context;
    expect(context.document).toBe(semantic);
    expect(context.generation).toBe(generation);
    expect(context.view).toMatchObject({ block: 'root', positions: before });
    editor.send({ type: 'UNDO' });
    context = editor.getSnapshot().context;
    expect(context.document?.description).not.toBe('semantic');
    editor.send({ type: 'REDO' });
    expect(editor.getSnapshot().context.document?.description).toBe('semantic');
    editor.send({ type: 'REDO' });
    expect(editor.getSnapshot().context.view).toMatchObject({ block: 'root', positions: after });
    expect(script.writes).toHaveLength(0);
    expect(script.checks).toHaveLength(0);
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

describe('field drafts in the editor machine', () => {
  const ready = nodeField(ROOT, 'ready', ['output', 'fields', { key: 'status' }, 'value']);
  const field = encodeField(ready);
  const json: FieldCommand = (document, text) => {
    const reading = readJson(text);
    if (!reading.ok) return { status: 'rejected', error: { kind: reading.reason } };
    return { status: 'applied', document: setField(document, ready, reading.value) };
  };
  const draft = (editor: Awaited<ReturnType<typeof readyEditor>>) => editor.getSnapshot().context.drafts[field];
  const status = (editor: Awaited<ReturnType<typeof readyEditor>>) => blockAt(editor.getSnapshot().context.document!, ROOT)!.nodes.ready;

  it('applies exactly one draft version and never clears newer text of the same field', async () => {
    const editor = await readyEditor();
    editor.send({ type: 'DRAFT', address: ready, text: '"first"' });
    const first = draft(editor).version;
    editor.send({ type: 'DRAFT', address: ready, text: '"second"' });
    // A request for the older version applies nothing and clears nothing.
    editor.send({ type: 'APPLY_DRAFT', field, version: first, command: json });
    expect(draft(editor).text).toBe('"second"');
    expect(status(editor)).toMatchObject({ output: { fields: { status: { value: 'ready' } } } });
    editor.send({ type: 'APPLY_DRAFT', field, version: draft(editor).version, command: json });
    expect(draft(editor)).toBeUndefined();
    expect(status(editor)).toMatchObject({ output: { fields: { status: { value: 'second' } } } });
    expect(editor.getSnapshot().context.applied).toMatchObject({ field, result: { status: 'applied' } });
  });

  it('keeps a rejected draft with its error, and never reports success for it', async () => {
    const script = scripted();
    const editor = await readyEditor(script);
    const document = editor.getSnapshot().context.document;
    editor.send({ type: 'DRAFT', address: ready, text: '"open' });
    editor.send({ type: 'APPLY_DRAFT', field, version: draft(editor).version, command: json });
    expect(draft(editor)).toMatchObject({ text: '"open', error: { kind: 'syntax' } });
    expect(editor.getSnapshot().context.applied?.result).toEqual({ status: 'rejected', error: { kind: 'syntax' } });
    expect(editor.getSnapshot().context.document).toBe(document);
    editor.send({ type: 'SAVE' });
    expect(script.writes).toHaveLength(0);
  });

  it('marks a draft whose field Undo changed, keeps its text, and applies it only once the author keeps it', async () => {
    const script = scripted();
    const editor = await readyEditor(script);
    editor.send({ type: 'EDIT', apply: document => setField(document, ready, 'edited') });
    editor.send({ type: 'DRAFT', address: ready, text: '"mine"' });
    // An unrelated change leaves the draft alone.
    editor.send({ type: 'EDIT', apply: describeAs('unrelated') });
    expect(draft(editor).conflict).toBeUndefined();
    editor.send({ type: 'UNDO' });
    editor.send({ type: 'UNDO' });
    expect(draft(editor)).toMatchObject({ text: '"mine"', conflict: 'changed' });
    // Neither a stale application nor Save reapplies it.
    editor.send({ type: 'APPLY_DRAFT', field, version: draft(editor).version, command: json });
    expect(status(editor)).toMatchObject({ output: { fields: { status: { value: 'ready' } } } });
    editor.send({ type: 'SAVE' });
    expect(script.writes).toHaveLength(0);
    // Redo back to the value it began on does not silently clear the mark.
    editor.send({ type: 'REDO' });
    expect(draft(editor).conflict).toBe('changed');
    editor.send({ type: 'KEEP_DRAFT', field });
    expect(draft(editor)).toMatchObject({ text: '"mine"', base: '"edited"' });
    expect(draft(editor).conflict).toBeUndefined();
    editor.send({ type: 'APPLY_DRAFT', field, version: draft(editor).version, command: json });
    expect(status(editor)).toMatchObject({ output: { fields: { status: { value: 'mine' } } } });
  });

  it('never attaches a draft to another field after deletion, a kind change or an unidentified array resize', async () => {
    const editor = await readyEditor();
    /** The one draft holding `text`, wherever it is kept, and its key. */
    const holding = (text: string) => {
      const found = Object.entries(editor.getSnapshot().context.drafts).filter(([, value]) => value.text === text);
      expect(found).toHaveLength(1);
      return { key: found[0][0], draft: found[0][1] };
    };
    // Deletion, then a new node under the same id: the text stays the old
    // field's, detached from any address a field shows.
    editor.send({ type: 'DRAFT', address: ready, text: '"kept"' });
    editor.send({ type: 'EDIT', apply: document => deleteNodes(document, ROOT, ['ready']) });
    expect(holding('"kept"').draft).toMatchObject({ conflict: 'missing', address: ready });
    expect(draft(editor)).toBeUndefined();
    editor.send({ type: 'EDIT', apply: document => addNode(document, ROOT, 'return', 'ready') });
    expect(draft(editor)).toBeUndefined();
    editor.send({ type: 'KEEP_DRAFT', field: holding('"kept"').key });
    expect(holding('"kept"').draft.conflict).toBe('missing');
    editor.send({ type: 'SAVE' });
    expect(editor.getSnapshot().context.notice).toEqual({ kind: 'pending_fields', count: 1 });
    editor.send({ type: 'DISCARD_DRAFT', field: holding('"kept"').key });
    expect(editor.getSnapshot().context.drafts).toEqual({});
    expect(editor.getSnapshot().context.notice).toBeUndefined();

    // A value-kind change removes the literal the text was typed into.
    const revise = nodeField(ROOT, 'revise', ['output', 'fields', { key: 'status' }]);
    editor.send({ type: 'DRAFT', address: within(revise, ['value']), text: '"later"' });
    editor.send({ type: 'EDIT', apply: document => setField(document, revise, { type: 'object', fields: {} }) });
    editor.send({ type: 'EDIT', apply: document => setField(document, revise, { type: 'literal', value: 'needs_revision' }) });
    expect(holding('"later"').draft.conflict).toBe('missing');
    expect(editor.getSnapshot().context.drafts[encodeField(within(revise, ['value']))]).toBeUndefined();

    // An array resize the editor cannot attribute (no array edit) leaves
    // item identity unknown: the text is kept, detached.
    const predicates = nodeField(ROOT, 'decision', ['condition', 'predicates']);
    const first = within(predicates, [0, 'value', 'path'], 'append');
    editor.send({ type: 'DRAFT', address: first, text: 'acceptable' });
    type Items = { type: string }[];
    editor.send({ type: 'EDIT', apply: document => updateField<Items>(document, predicates, items => items.slice(1)) });
    expect(fieldText(editor.getSnapshot().context.document!, first)).toBe('');
    expect(holding('acceptable').draft.conflict).toBe('missing');
    expect(editor.getSnapshot().context.drafts[encodeField(first)]).toBeUndefined();
  });

  it('takes the drafts of a renamed node along, and keeps a refused rename with its error', async () => {
    const editor = await readyEditor();
    const id = nodeField(ROOT, 'ready', [], 'key');
    const rename: FieldCommand = (document, text) => ({ status: 'applied', document: renameNode(document, ROOT, 'ready', text) });
    editor.send({ type: 'DRAFT', address: ready, text: '"pending"' });
    editor.send({ type: 'DRAFT', address: id, text: 'revise' });
    editor.send({ type: 'APPLY_DRAFT', field: encodeField(id), version: editor.getSnapshot().context.drafts[encodeField(id)].version, command: rename });
    expect(editor.getSnapshot().context.drafts[encodeField(id)]).toMatchObject({ text: 'revise', error: { kind: 'rejected', code: 'duplicate_identifier' } });
    editor.send({ type: 'DRAFT', address: id, text: 'accepted' });
    editor.send({ type: 'APPLY_DRAFT', field: encodeField(id), version: editor.getSnapshot().context.drafts[encodeField(id)].version, command: rename });
    const drafts = Object.values(editor.getSnapshot().context.drafts);
    expect(drafts).toHaveLength(1);
    expect(drafts[0]).toMatchObject({ address: nodeField(ROOT, 'accepted', ready.path), text: '"pending"' });
    expect(drafts[0].conflict).toBeUndefined();
  });

  it('clears drafts only through an explicit discard or a confirmed reload', async () => {
    const script = scripted();
    const editor = await readyEditor(script);
    editor.send({ type: 'DRAFT', address: ready, text: '"pending"' });
    editor.send({ type: 'NAVIGATE', block: [{ node: 'reviews', branch: 'quality' }] });
    editor.send({ type: 'BOUNDARY' });
    expect(draft(editor).text).toBe('"pending"');
    editor.send({ type: 'RELOAD' });
    script.reads[1].result.resolve(source('flow'));
    await flush();
    expect(editor.getSnapshot().context.drafts).toEqual({});
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
    return { actor, script, library: () => actor.getSnapshot().context.library, port: ports };
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
    expect(context.observation?.listing.files.map(file => file.id)).toEqual(['beta']);
    expect(script('workspace:a')[0].lists).toHaveLength(1);
  });

  it('creates the exact file, then opens exactly that file', async () => {
    const { actor, library, script } = surface();
    const scope: WorkflowScope = { kind: 'workspace', workspaceId: 'a' };
    library().send({ type: 'SCOPE', scope });
    const port = script('workspace:a')[0];
    port.lists[0].resolve(listing(scope, []));
    await flush();
    library().send({ type: 'CONFIRM', kind: 'create' });
    library().send({ type: 'SUBMIT', seq: library().getSnapshot().context.confirmation!.seq, id: 'fresh', description: 'Fresh' });
    expect(port.writes[0]).toMatchObject({ id: 'fresh', expected: 'missing', edit: { operation: 'create', description: 'Fresh' } });
    port.writes[0].result.resolve(commit('sha256:fresh', { status: 'observed', source: source('fresh', 'sha256:fresh') }));
    await flush();
    expect(actor.getSnapshot().matches('editor')).toBe(true);
    expect(actor.getSnapshot().context.editor!.getSnapshot().context.target).toEqual({ endpoint: 'ws://127.0.0.1:1/', scope, id: 'fresh' });
  });

  it('guards unsaved edits on Back and on leaving the route, and keeps the target fixed', async () => {
    const { actor, script, port: fresh } = surface();
    actor.send({ type: 'OPEN', id: 'global_flow', port: fresh(USER_SCOPE) });
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
    const { actor, script, port: fresh } = surface();
    actor.send({ type: 'OPEN', id: 'first', port: fresh(USER_SCOPE) });
    actor.send({ type: 'BACK' });
    actor.send({ type: 'OPEN', id: 'second', port: fresh(USER_SCOPE) });
    const reading = (id: string) => script('user').find(port => port.reads[0]?.id === id)!.reads[0].result;
    reading('second').resolve(source('second', 'sha256:second'));
    reading('first').resolve(source('first', 'sha256:first'));
    await flush();
    const context = actor.getSnapshot().context.editor!.getSnapshot().context;
    expect(context.target.id).toBe('second');
    expect(context.base?.revision).toBe('sha256:second');
  });
});
