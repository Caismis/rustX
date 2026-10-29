import { describe, expect, it } from 'vitest';
import { EXAMPLES, exampleSource } from './workflow-fixtures';
import { addNode, CommandRejected, renameNode, replaceNode, setWorkflow } from '../src/app/workflows/document/commands';
import {
  blockAt, childSteps, sameDocument, ROOT, type BlockPath, type WorkflowDocument
} from '../src/app/workflows/document/model';
import { encodeField, fieldText, nodeField, setField } from '../src/app/workflows/document/fields';
import { projectScene } from '../src/app/workflows/ui/editor/scene';
import { viewId } from '../src/app/workflows/ui/editor/selection';
import { readJson } from '../shared/json-numbers';

/** The six native examples exactly as the App Server returns them. */
function example(id: string): WorkflowDocument {
  return exampleSource(id).document!;
}

function rejection(run: () => unknown) {
  try { run(); } catch (cause) { if (cause instanceof CommandRejected) return cause.code; throw cause; }
  return undefined;
}

/** Every lexical block of a document, depth first. */
function blocks(document: WorkflowDocument, path: BlockPath = ROOT): BlockPath[] {
  const block = blockAt(document, path)!;
  return [path, ...Object.entries(block.nodes).flatMap(([id, node]) => childSteps(id, node).flatMap(step => blocks(document, [...path, step])))];
}

describe('native Workflow editing document', () => {
  it('keeps every example semantically unchanged by projection and inverse edits', () => {
    for (const id of EXAMPLES) {
      const original = example(id);
      for (const path of blocks(original)) {
        const block = blockAt(original, path)!;
        for (const node of Object.keys(block.nodes)) {
          // Rename there and back: identities, edges, entry and references
          // return exactly; nothing else in the program moved.
          const renamed = renameNode(original, path, node, `${node}_renamed`);
          expect(sameDocument(renamed, original)).toBe(false);
          expect(blockAt(renamed, path)!.nodes[node]).toBeUndefined();
          expect(sameDocument(renameNode(renamed, path, `${node}_renamed`, node), original)).toBe(true);
        }
      }
      // Untouched advanced fields survive every structural edit elsewhere.
      const edited = setWorkflow(original, { description: 'changed', timeout_ms: original.timeout_ms });
      expect(sameDocument({ ...edited, description: original.description }, original)).toBe(true);
    }
  });

  it('replaces a node only with a definition of the same native kind', () => {
    const document = example('typed_agent');
    const [id, node] = Object.entries(blockAt(document, ROOT)!.nodes).find(([, node]) => node.type === 'agent')!;
    expect(node.type).toBe('agent');
    expect(rejection(() => replaceNode(document, ROOT, id, { type: 'return', output: { type: 'literal', value: {} } }))).toBe('missing_node');
    const edited = replaceNode(document, ROOT, id, { ...node, task: 'Different task' } as typeof node);
    const replaced = blockAt(edited, ROOT)!.nodes[id];
    expect(replaced.type === 'agent' && replaced.task).toBe('Different task');
  });
});

describe('authored maps and the browser number contract', () => {

  it('accepts exactly the numbers a JSON round trip keeps, judged from the authored token', () => {
    expect(readJson('{"n": 9007199254740991, "f": 1.5, "e": 1e21, "z": 0}')).toEqual({ ok: true, value: { n: 9007199254740991, f: 1.5, e: 1e21, z: 0 } });
    for (const text of ['9007199254740993', '[9007199254740992]', '1.0', '{"a": 2.50e1}', '-0.0', '1e400']) {
      expect(readJson(text), text).toEqual({ ok: false, reason: 'number' });
    }
    expect(readJson('{"a": ')).toEqual({ ok: false, reason: 'syntax' });
    // What is accepted serializes back to the very token native receives.
    const accepted = readJson('[2.5, -0.25, 9007199254740991, 1e21]');
    expect(accepted.ok && JSON.stringify(accepted.value)).toBe('[2.5,-0.25,9007199254740991,1e+21]');
  });

  it('refuses negative zero in every spelling and position before an integer rule accepts it', () => {
    // `-0` is an integer token and `Number.isSafeInteger(-0)` holds, yet it
    // would be written as `0`.
    for (const text of ['-0', '-0.0', '-0e0', '-0E+5', '-0.000e-3', '[1, -0]', '{"a": {"b": [2, -0]}}', '{"n": -0, "ok": 1}']) {
      expect(readJson(text), text).toEqual({ ok: false, reason: 'number' });
    }
    for (const [text, value] of [['0', 0], ['-1', -1], ['-9007199254740991', -9007199254740991], ['-0.5', -0.5], ['3.14', 3.14], ['1e-7', 1e-7], ['{"a": [0, -2.5]}', { a: [0, -2.5] }]] as const) {
      const reading = readJson(text);
      expect(reading, text).toEqual({ ok: true, value });
      expect(reading.ok && Object.is(reading.value, -0)).toBe(false);
    }
    for (const text of ['9007199254740992', '-9007199254740992', '1.0', '1e3', 'NaN', '1e400']) expect(readJson(text).ok, text).toBe(false);
  });

  it('projects inherited-looking identities from own native entries with finite geometry', () => {
    const document = addNode(example('parallel_review'), ROOT, 'return', 'constructor');
    const scene = projectScene(document);
    const own = scene.nodes.find(node => node.id === viewId([], 'node', 'constructor'));
    expect(own?.data.local).toBe('constructor');
    for (const node of scene.nodes) {
      expect(Number.isFinite(node.position.x) && Number.isFinite(node.position.y)).toBe(true);
    }
    expect(new Set(scene.nodes.map(node => node.id)).size).toBe(scene.nodes.length);
  });
});

describe('field addresses', () => {
  const fixture = (): WorkflowDocument => {
    const document = example('parallel_review');
    return replaceNode(document, ROOT, 'ready', {
      type: 'return',
      output: { type: 'object', fields: {
        'a.fields.b': { type: 'literal', value: 1 },
        a: { type: 'object', fields: { b: { type: 'literal', value: 2 } } },
      } },
    });
  };
  const direct = nodeField(ROOT, 'ready', ['output', 'fields', { key: 'a.fields.b' }, 'value']);
  const nested = nodeField(ROOT, 'ready', ['output', 'fields', { key: 'a' }, 'fields', { key: 'b' }, 'value']);

  it('keeps a direct key containing dots apart from the nested path it spells', () => {
    expect(encodeField(direct)).not.toBe(encodeField(nested));
    // An authored key is never a structural component of the same name.
    expect(encodeField(nodeField(ROOT, 'n', ['output', 'fields', { key: 'fields' }]))).not.toBe(encodeField(nodeField(ROOT, 'n', ['output', 'fields', 'fields'])));
    const document = fixture();
    expect(fieldText(document, direct)).toBe('1');
    expect(fieldText(document, nested)).toBe('2');
    const edited = setField(document, direct, 3);
    expect(fieldText(edited, direct)).toBe('3');
    expect(fieldText(edited, nested)).toBe('2');
  });
});
