import { expect, it } from 'vitest';
import { modelToolCall, parseModelToolCall } from '../../protocol/app-server/model-tool-call';
import { traceProgram } from '../src/client/trace-content';
import { toolCard } from '../src/bindings/tools';
import { nativeTool } from './native-tool-cases';

it('projects one model envelope while preserving identically named business fields', () => {
  const business = { arguments: { query: 'source' }, elevation_reason: 'business value', execution_mode: 'business mode' };
  const wire = { arguments: business, elevation_reason: 'runtime reason', execution_mode: 'foreground' };
  expect(modelToolCall(wire)?.arguments).toBe(business);
  expect(parseModelToolCall(JSON.stringify(wire))).toEqual(wire);
  expect(modelToolCall({ command: 'flat invocation' })).toBeUndefined();
  for (const raw of ['', '{"arguments":', '{"arguments":null}', '{"arguments":[]}']) expect(parseModelToolCall(raw)).toBeUndefined();
});

it('keeps source and diff fields inside business arguments and metadata outside them', () => {
  const bash = nativeTool('tool-bash', 'bash', { command: 'printf actual', description: 'Actual source', execution_mode: 'background' }, []);
  bash.state = { type: 'running', arguments: JSON.stringify({ arguments: { command: 'printf actual', description: 'Actual source', execution_mode: 'background' }, execution_mode: 'foreground' }) };
  expect(toolCard(bash)).toMatchObject({ input: 'printf actual', summary: 'Actual source', background: false });
  bash.state = { type: 'running', arguments: JSON.stringify({ arguments: { command: 'printf actual' }, execution_mode: 'background' }) };
  expect(toolCard(bash).background).toBe(true);
  const edit = nativeTool('tool-edit', 'edit', { path: 'report.md', edits: [{ oldText: 'before', newText: 'after' }] }, []);
  expect(toolCard(edit)).toMatchObject({ path: 'report.md', diffs: [{ path: 'report.md', oldText: 'before', newText: 'after' }] });
});

it('uses the same canonical envelope for deferred Trace source extraction', () => {
  const wire = { arguments: { command: 'printf canonical' }, command: 'outer metadata is not source' };
  expect(traceProgram('tool-bash', wire)).toMatchObject({ field: 'command', language: 'shell', text: { text: 'printf canonical' } });
  expect(traceProgram('tool-write', { arguments: { content: 'fn main() {}', path: 'main.rs' } })).toMatchObject({ field: 'content', text: { text: 'fn main() {}' } });
  expect(traceProgram('tool-bash', { command: 'flat invocation' })).toBeUndefined();
  expect(traceProgram('mcp-write', wire)).toBeUndefined();
});
