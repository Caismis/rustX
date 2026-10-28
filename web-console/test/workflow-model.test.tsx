import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { act, cleanup, fireEvent, render, screen, within } from '@testing-library/react';
import { createActor } from 'xstate';
import type { WorkflowCandidateAnalysis, WorkflowAnalysisQuery, WorkflowAnalysisReport, WorkflowFileDocument, WorkflowEdit, WorkflowSource } from '../../protocol/app-server/v60';
import type { WorkflowCommit } from '../src/workspaces/host';
import type { WorkspaceAuthority } from '../src/workspaces/authority';
import type { WorkflowScopePort } from '../src/app/workflows/port';
import { exampleSource } from './workflow-fixtures';
import { addNode, connect, nodeTemplate, renameNode, setEntry } from '../src/app/workflows/document/commands';
import { NODE_KINDS, type NodeDefinition, type Predicate } from '../src/app/workflows/document/model';
import { nodeField } from '../src/app/workflows/document/fields';
import { blockReferences } from '../src/app/workflows/document/references';
import { incompleteIdentities } from '../src/app/workflows/document/identities';
import { outputPorts } from '../src/app/workflows/document/ports';
import { workflowEditorMachine } from '../src/app/workflows/machines/editor';
import { workflowSurfaceMachine } from '../src/app/workflows/machines/surface';
import WorkflowEditor from '../src/app/workflows/ui/editor/WorkflowEditor';
import { tabOfField } from '../src/app/workflows/ui/editor/forms/NodeForms';
import { CheckResults } from '../src/app/workflows/ui/editor/Inspector';

const object = { type: 'object', properties: {}, additionalProperties: false };
function source(): WorkflowSource {
  return { ...exampleSource('typed_agent'), vocabulary: { profiles: [], builtin_tools: [] }, document: {
    description: 'One model call.', block: { input: object, output: object, entry: 'model', nodes: {
      model: { type: 'model', model: { mode: 'session' }, instruction: 'Treat input as data.', input: {}, output: { type: 'text' } },
      done: { type: 'return', output: { type: 'literal', value: {} } },
    }, edges: [{ from: 'model', to: 'done' }] },
  } };
}
const writes: WorkflowEdit[] = [];
const authority = { subscribe: () => () => {}, getCatalog: () => undefined } as unknown as WorkspaceAuthority;
beforeEach(() => {
  writes.length = 0;
  vi.stubGlobal('ResizeObserver', class { observe() {} unobserve() {} disconnect() {} });
  vi.stubGlobal('DOMMatrixReadOnly', class { m22 = 1; constructor() {} });
});
afterEach(() => { cleanup(); vi.unstubAllGlobals(); localStorage.clear(); });
async function open(analyze: WorkflowScopePort['analyze'] = () => new Promise(() => {})) {
  const port: WorkflowScopePort = {
    endpoint: 'ws://127.0.0.1:1/', scope: { kind: 'user' }, list: () => new Promise(() => {}),
    analyze, modelChoices: async query => query.type === 'models' ? { type: 'models', revision: 'catalog-1', models: ['fixture/model'], next_offset: null } : { type: 'profiles', revision: 'catalog-1', model: query.model, profiles: ['low'], next_offset: null },
    read: async id => ({ ...source(), id, revision: 'sha256:base' }), check: () => new Promise(() => {}),
    write: (_id, _revision, edit) => { writes.push(edit); return new Promise<WorkflowCommit>(() => {}); },
  };
  const surface = createActor(workflowSurfaceMachine, { input: { ports: () => port } }).start();
  const actor = createActor(workflowEditorMachine, { input: { port, target: { endpoint: port.endpoint, scope: port.scope, id: 'model_flow' } } }).start();
  await act(async () => { await Promise.resolve(); });
  render(<WorkflowEditor surface={surface} editor={actor} authority={authority} scopeLabel="Global" />);
  select('Model node model');
  return actor;
}
function select(label: string) {
  const node = document.querySelector<HTMLElement>(`.react-flow__node[aria-label="${label}"]`)!;
  act(() => node.focus()); fireEvent.keyDown(node, { key: 'Enter' });
}
const inspector = () => within(screen.getByRole('complementary', { name: 'Inspector' }));
const save = () => fireEvent.keyDown(document.activeElement ?? document.body, { key: 's', ctrlKey: true });

function controlledAnalysis() {
  const requests: { id: string; generation: string; query?: WorkflowAnalysisQuery; resolve: (report: WorkflowAnalysisReport) => void; reject: (error: Error) => void }[] = [];
  const analyze: WorkflowScopePort['analyze'] = (id, _document, generation, query) => new Promise((resolve, reject) => requests.push({ id, generation, query, resolve, reject }));
  const complete = (analysis: Partial<WorkflowCandidateAnalysis>) => {
    const request = requests.at(-1)!;
    request.resolve({ target: { kind: 'user' }, id: request.id, generation: request.generation, query: request.query ?? null,
      candidate_revision: `sha256:${'a'.repeat(64)}`, presentation: [],
      analysis: { outputs: [], references: [], selected: null, diagnostic: null, incomplete: false, truncated: false, ...analysis } });
  };
  return { analyze, requests, complete };
}

describe('native candidate analysis presentation', () => {
  const equalities = (['branch', 'loop'] as const).flatMap(kind => (['equal', 'not_equal'] as const).flatMap(operator => (['left', 'right'] as const).map(side => ({ kind, operator, side }))));
  it.each(equalities)('disables an incompatible $kind $operator $side operand while preserving a truly unknown type relation', async ({ kind, operator, side }) => {
    const native = controlledAnalysis(), editor = await open(native.analyze);
    const prefix = kind === 'loop' ? 'result' : 'args';
    const count = { type: 'reference' as const, path: [prefix, 'count'] }, unfinished = { type: 'reference' as const, path: ['unfinished'] };
    const predicate: Predicate = { type: operator, left: side === 'left' ? count : unfinished, right: side === 'right' ? count : unfinished };
    const schema = { type: 'object', properties: { count: { type: 'integer' }, text: { type: 'string' } }, required: ['count', 'text'], additionalProperties: false };
    const node: NodeDefinition = kind === 'branch' ? { type: 'branch', condition: predicate }
      : { type: 'loop', input: { type: 'reference', path: ['args'] }, body: { input: schema, output: schema, entry: 'done', nodes: { done: { type: 'return', output: { type: 'reference', path: ['args'] } } }, edges: [] }, until: predicate, carry: { type: 'reference', path: ['result'] }, max_iterations: 1 };
    const ports = kind === 'branch' ? ['true', 'false'] : ['satisfied', 'exhausted'];
    act(() => editor.send({ type: 'EDIT', apply: document => ({ ...document, block: { ...document.block, input: schema, nodes: { ...document.block.nodes, model: node }, edges: ports.map(port => ({ from: 'model', to: 'done', port: port as 'true' | 'false' | 'satisfied' | 'exhausted' })) } }) }));
    fireEvent.click(inspector().getAllByRole('button', { name: 'Choose a variable' })[side === 'left' ? 0 : 1]);
    expect(native.requests.at(-1)?.query?.consumer).toEqual({ address: { block: [], node: 'model' }, field: { kind: kind === 'branch' ? 'condition' : 'loop_until' }, path: [{ kind: side }] });
    await act(async () => native.complete({ references: [
      { producer: null, path: [prefix, 'count'], schema: { type: 'integer' }, availability: 'available', compatibility: 'incompatible', diagnostic: 'equality requires matching boolean/string/null types' },
      { producer: null, path: [prefix, 'text'], schema: { type: 'string' }, availability: 'available', compatibility: 'unknown', diagnostic: null },
    ] }));
    const numeric = inspector().getByRole('button', { name: new RegExp(`^${prefix} / count ·`) }) as HTMLButtonElement;
    expect(numeric.disabled).toBe(true);
    const text = inspector().getByRole('button', { name: new RegExp(`^${prefix} / text ·`) }) as HTMLButtonElement;
    expect(text.disabled).toBe(false); expect(text.textContent).toContain('Compatibility unknown');
    fireEvent.click(text);
    const current = editor.getSnapshot().context.document!.block.nodes.model;
    if (current.type !== 'branch' && current.type !== 'loop') throw new Error('Predicate owner');
    const changed = current.type === 'branch' ? current.condition : current.until;
    expect(changed).toMatchObject({ type: operator, [side]: { type: 'reference', path: [prefix, 'text'] } });
  });

  it('replaces output loading with a failure when the current request rejects', async () => {
    const native = controlledAnalysis();
    await open(native.analyze);
    fireEvent.click(inspector().getByRole('tab', { name: 'Output' }));
    expect(inspector().getByRole('status').textContent).toBe("Analyzing the current candidate's output declaration…");
    expect(native.requests).toHaveLength(1);
    await act(async () => native.requests[0].reject(new Error('source authority unavailable')));
    expect(inspector().getByRole('alert').textContent).toBe('Output analysis failed. Check the source authority and candidate bounds.');
    expect(inspector().queryByRole('status')).toBeNull();
  });

  it('explains a truncated output catalog when this node is absent', async () => {
    const native = controlledAnalysis();
    await open(native.analyze);
    fireEvent.click(inspector().getByRole('tab', { name: 'Output' }));
    await act(async () => native.complete({ truncated: true, incomplete: true }));
    expect(inspector().getByRole('status').textContent).toBe('The analysis result was truncated before this output could be included.');
    expect(inspector().queryByText(/Analyzing the current candidate/)).toBeNull();
    expect(inspector().queryByText(/output declaration is incomplete/)).toBeNull();
  });

  it('distinguishes an incomplete declaration from a ready output even when other outputs are truncated', async () => {
    const native = controlledAnalysis();
    const editor = await open(native.analyze);
    fireEvent.click(inspector().getByRole('tab', { name: 'Output' }));
    await act(async () => native.complete({ outputs: [{ producer: { block: [], node: 'model' }, schema: null, diagnostic: 'incomplete' }], truncated: true, incomplete: true }));
    expect(inspector().getByRole('status').textContent).toBe('The output declaration is incomplete or violates the Workflow schema contract.');
    // A new candidate must show loading while its own analysis is outstanding.
    act(() => editor.send({ type: 'EDIT', apply: document => ({ ...document, description: 'New candidate' }) }));
    expect(inspector().getByRole('status').textContent).toContain('Analyzing');
    await act(async () => native.complete({ outputs: [{ producer: { block: [], node: 'model' }, schema: { type: 'string' }, diagnostic: null }], truncated: true }));
    expect(inspector().getByText(/Declared or derived by native candidate analysis/)).toBeTruthy();
    expect(inspector().queryByRole('status')).toBeNull();
    expect(inspector().getByText(/"type": "string"/)).toBeTruthy();
  });

  it('disables known scope violations with unknown types and preserves a pending upstream selection', async () => {
    const native = controlledAnalysis();
    const editor = await open(native.analyze);
    fireEvent.click(inspector().getByRole('tab', { name: 'Input' }));
    fireEvent.change(inspector().getByLabelText('Field name'), { target: { value: 'value' } });
    fireEvent.click(inspector().getByRole('button', { name: 'Add field' }));
    fireEvent.click(inspector().getByRole('button', { name: 'Choose a variable' }));
    // These are native scope facts; the picker must not infer legality from
    // the unknown schemas or soften unavailable into pending.
    const references: WorkflowCandidateAnalysis['references'] = ['model', 'later', 'branch_only'].map(node => ({
      producer: { block: [], node }, path: [node], schema: null, availability: 'unavailable', compatibility: 'unknown', diagnostic: 'not in the entry scope',
    }));
    references.push({ producer: { block: [], node: 'upstream' }, path: ['upstream'], schema: null, availability: 'pending', compatibility: 'unknown', diagnostic: 'output contract incomplete' });
    await act(async () => native.complete({ references, incomplete: true }));
    for (const node of ['model', 'later', 'branch_only']) {
      const choice = inspector().getByRole('button', { name: new RegExp(`^${node} ·`) }) as HTMLButtonElement;
      expect(choice.disabled).toBe(true);
      fireEvent.click(choice);
      expect(editor.getSnapshot().context.document!.block.nodes.model).toMatchObject({ input: { value: { path: ['args'] } } });
    }
    const pending = inspector().getByRole('button', { name: /^upstream ·/ }) as HTMLButtonElement;
    expect(pending.disabled).toBe(false);
    fireEvent.click(pending);
    expect(editor.getSnapshot().context.document!.block.nodes.model).toMatchObject({ input: { value: { path: ['upstream'] } } });
    expect(inspector().getByRole('button', { name: 'Choose a variable' }).getAttribute('aria-expanded')).toBe('false');
  });

  it('disables incompatible scalar/array Tool roots while an object with unknown full contract stays selectable', async () => {
    const native = controlledAnalysis();
    const editor = await open(native.analyze);
    const schemas = { string_root: { type: 'string' }, array_root: { type: 'array', items: { type: 'string' } }, object_root: object };
    act(() => editor.send({ type: 'EDIT', apply: document => ({ ...document, block: { ...document.block, entry: 'string_root', nodes: {
      ...document.block.nodes,
      ...Object.fromEntries(Object.entries(schemas).map(([id, schema]) => [id, { type: 'tool' as const, selector: { origin: 'builtin' as const, name: 'read' }, arguments: { type: 'reference' as const, path: ['args'] }, result: { type: 'json' as const, part: 0, schema } }])),
      model: { type: 'tool', selector: { origin: 'builtin', name: 'read' }, arguments: { type: 'reference', path: ['args'] }, result: { type: 'text', part: 0 } },
    }, edges: [{ from: 'string_root', to: 'array_root' }, { from: 'array_root', to: 'object_root' }, { from: 'object_root', to: 'model' }, { from: 'model', to: 'done' }] } }) }));
    fireEvent.click(inspector().getByRole('tab', { name: 'Input' }));
    fireEvent.click(inspector().getByRole('button', { name: 'Choose a variable' }));
    expect(native.requests.at(-1)?.query?.consumer).toEqual({ address: { block: [], node: 'model' }, field: { kind: 'arguments' }, path: [] });
    const references: WorkflowCandidateAnalysis['references'] = Object.entries(schemas).map(([node, schema]) => ({ producer: { block: [], node }, path: [node], schema, availability: 'available', compatibility: node === 'object_root' ? 'unknown' : 'incompatible', diagnostic: null }));
    await act(async () => native.complete({ references }));
    for (const id of ['string_root', 'array_root']) expect((inspector().getByRole('button', { name: new RegExp(`^${id} ·`) }) as HTMLButtonElement).disabled).toBe(true);
    const choice = inspector().getByRole('button', { name: /^object_root ·/ }) as HTMLButtonElement;
    expect(choice.disabled).toBe(false); expect(choice.textContent).toContain('Compatibility unknown');
    fireEvent.click(choice);
    expect(editor.getSnapshot().context.document!.block.nodes.model).toMatchObject({ arguments: { path: ['object_root'] } });
  });
});

describe('Model native editing document', () => {
  it('explains why an offline follow-Model check needs the invoking Attempt', () => {
    render(<CheckResults generation={1} global={false} workspaces={[{ id: 'root', displayName: 'Workspace A', displayPath: '/workspace' }]}
      setContext={() => {}} run={() => {}} go={() => {}} check={{ generation: 1, explain: false, report: {
        structure: { status: 'valid', outputs: {}, revision: 'sha256:check' },
        dependencies: { status: 'requires_attempt', context: '/workspace', model_nodes: ['block.nodes.model'] },
      } }} />);
    expect(screen.getByText('Structure check passed')).toBeTruthy();
    expect(screen.getByText(/Model nodes block.nodes.model follow the invoking Attempt in Workspace A/)).toBeTruthy();
    expect(screen.queryByText(/Agents, Models and Tools admit/)).toBeNull();
  });

  it('is the eighth node with one ordinary successor, typed bindings and field routing', () => {
    expect(NODE_KINDS).toEqual(['agent', 'model', 'tool', 'branch', 'parallel', 'review', 'loop', 'return']);
    expect(nodeTemplate('model')).toEqual({ type: 'model', model: { mode: 'session' }, instruction: '', input: {}, output: { type: 'text' } });
    expect(outputPorts('model')).toEqual(['next']);
    let document = addNode(source().document!, [], 'model', 'classify');
    document = setEntry(document, [], 'classify');
    document = connect(document, [], 'classify', 'next', 'model');
    const node = document.block.nodes.model;
    if (node.type !== 'model') throw new Error('expected model');
    node.input = { 'content.fields.risk': { type: 'reference', path: ['classify'] } };
    document = renameNode(document, [], 'classify', 'categorize');
    expect(blockReferences(document.block)).toContainEqual(['categorize']);
    expect(tabOfField('model', nodeField([], 'model', ['output', 'schema']))).toBe('output');
    expect(tabOfField('model', nodeField([], 'model', ['input', { key: 'content.fields.risk' }]))).toBe('input');
    expect(tabOfField('model', nodeField([], 'model', ['model', 'request_params']))).toBe('config');
  });

  it('authors registered Model/Profile selection, parameters, budget and typed bindings', async () => {
    const actor = await open();
    fireEvent.change(inspector().getByLabelText('Model policy'), { target: { value: 'explicit' } });
    expect(incompleteIdentities(actor.getSnapshot().context.document!)).toEqual([{ block: [], node: 'model', field: 'model.model' }]);
    await act(async () => { await Promise.resolve(); });
    fireEvent.change(inspector().getByLabelText('Model', { exact: true }), { target: { value: 'fixture/model' } });
    await act(async () => { await Promise.resolve(); });
    fireEvent.change(inspector().getByLabelText('Model Profile'), { target: { value: 'low' } });
    fireEvent.change(inspector().getByLabelText('Maximum output tokens'), { target: { value: '1024' } });
    fireEvent.change(inspector().getByLabelText('Request parameters'), { target: { value: '{"temperature":0.25}' } });
    fireEvent.blur(inspector().getByLabelText('Request parameters'));
    fireEvent.click(inspector().getByRole('tab', { name: 'Input' }));
    fireEvent.change(inspector().getByLabelText('Field name'), { target: { value: 'content' } });
    fireEvent.click(inspector().getByRole('button', { name: 'Add field' }));
    save();
    const document = (writes[0] as { document: WorkflowFileDocument }).document;
    expect(document.block.nodes.model).toEqual({ type: 'model', model: { mode: 'explicit', model: 'fixture/model', profile: 'low', request_params: { temperature: 0.25 }, max_output_tokens: 1024 }, instruction: 'Treat input as data.', input: { content: { type: 'reference', path: ['args'] } }, output: { type: 'text' } });
  });

  it('keeps a dirty JSON Schema draft after switching output mode and blocks a save until resolved', async () => {
    const actor = await open();
    fireEvent.click(inspector().getByRole('tab', { name: 'Output' }));
    expect(inspector().queryByLabelText('Output JSON Schema')).toBeNull();
    fireEvent.change(inspector().getByLabelText('Output mode'), { target: { value: 'json_schema' } });
    act(() => actor.send({ type: 'UNDO' }));
    expect(inspector().queryByLabelText('Output JSON Schema')).toBeNull();
    act(() => actor.send({ type: 'REDO' }));
    const schema = inspector().getByLabelText('Output JSON Schema');
    fireEvent.change(schema, { target: { value: '{"type":"object",' } });
    fireEvent.blur(schema);
    fireEvent.change(inspector().getByLabelText('Output mode'), { target: { value: 'text' } });
    save();
    expect(writes).toEqual([]);
    expect(actor.getSnapshot().context.notice).toEqual({ kind: 'pending_fields', count: 1 });
    expect(Object.values(actor.getSnapshot().context.drafts)[0].text).toBe('{"type":"object",');
    // The removed schema's text stays inspectable without being applied to
    // the Text union. Discarding it explicitly allows the Text save.
    fireEvent.click(screen.getByRole('button', { name: 'Show kept text' }));
    expect((screen.getByLabelText('Kept text of model · output.schema') as HTMLTextAreaElement).value).toBe('{"type":"object",');
    expect(actor.getSnapshot().context.document!.block.nodes.model).toMatchObject({ output: { type: 'text' } });
    fireEvent.click(screen.getByRole('button', { name: 'Discard' }));
    save();
    expect((writes[0] as { document: WorkflowFileDocument }).document.block.nodes.model).toMatchObject({ output: { type: 'text' } });
  });
});
