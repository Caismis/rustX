import test from 'node:test';
import assert from 'node:assert/strict';
import { comparePayloads } from './compare-http.mjs';

function makeTools() {
  return [
    {
      type: 'function',
      function: {
        name: 'read',
        description: 'Read file.\n\nPass business tool inputs inside the required arguments object.',
        parameters: {
          $schema: 'http://json-schema.org/draft-07/schema#',
          $id: 'urn:rustx:invocation',
          type: 'object',
          properties: {
            arguments: { $ref: 'json-schema:///' },
            elevation_reason: { type: ['string', 'null'] },
          },
          required: ['arguments'],
          additionalProperties: false,
        },
      },
    },
  ];
}

function makeToolPairRecord(workspace, timestamp, toolCallArgs, extraBody = {}) {
  return {
    measurement_version: 3,
    case: 'tool',
    wire_requests: [
      {
        index: 0,
        method: 'POST',
        path: '/v1/chat/completions',
        protocol: 'openai_chat_completions',
        model: 'chat-model',
        body: {
          model: 'chat-model',
          stream: true,
          max_completion_tokens: 1024,
          tools: makeTools(),
          messages: [
            {
              role: 'system',
              content: `Core prompt\n\nCurrent working directory: ${workspace}\n\nExecution permission mode: full_access.`,
              tool_calls: null,
            },
            { role: 'user', content: [{ type: 'text', text: 'benchmark turn 0000' }], tool_calls: null },
            {
              role: 'user',
              content: [{ type: 'text', text: `<system-reminder>\nTimezone: UTC\nCurrent time: ${timestamp}\n</system-reminder>` }],
              tool_calls: null,
            },
          ],
          ...extraBody,
        },
      },
      {
        index: 1,
        method: 'POST',
        path: '/v1/chat/completions',
        protocol: 'openai_chat_completions',
        model: 'chat-model',
        body: {
          model: 'chat-model',
          stream: true,
          max_completion_tokens: 1024,
          tools: makeTools(),
          messages: [
            {
              role: 'system',
              content: `Core prompt\n\nCurrent working directory: ${workspace}`,
              tool_calls: null,
            },
            { role: 'user', content: [{ type: 'text', text: 'benchmark turn 0000' }], tool_calls: null },
            {
              role: 'user',
              content: [{ type: 'text', text: `<system-reminder>\nTimezone: UTC\nCurrent time: ${timestamp}\n</system-reminder>` }],
              tool_calls: null,
            },
            {
              role: 'assistant',
              content: [],
              tool_calls: [
                {
                  type: 'function',
                  id: 'benchmark-read',
                  function: {
                    name: 'read',
                    arguments: JSON.stringify(toolCallArgs),
                  },
                },
              ],
            },
            {
              role: 'tool',
              tool_call_id: 'benchmark-read',
              content: [{ type: 'text', text: 'deterministic-note-payload-6d41' }],
              tool_calls: null,
            },
          ],
          ...extraBody,
        },
      },
    ],
  };
}

test('same-engine comparator accepts nested arguments.path and distinct temp workspaces', () => {
  const before = makeToolPairRecord('/tmp/before-run/workspace', '2026-11-15 05:07:20', {
    arguments: { path: '/tmp/before-run/workspace/note.txt' },
  });
  const after = makeToolPairRecord('/tmp/after-run/workspace', '2026-11-15 06:08:21', {
    arguments: { path: '/tmp/after-run/workspace/note.txt' },
  });
  const result = comparePayloads(before, after);
  assert.equal(result.case, 'tool');
  assert.equal(result.checked.length, 2);
  assert.equal(result.checked[0].fullRequestEqual, true);
  assert.equal(result.checked[1].fullRequestEqual, true);
});

test('same-engine comparator rejects flat legacy read arguments without envelope', () => {
  const before = makeToolPairRecord('/tmp/before-run/workspace', '2026-11-15 05:07:20', {
    path: '/tmp/before-run/workspace/note.txt',
  });
  const after = makeToolPairRecord('/tmp/after-run/workspace', '2026-11-15 05:07:20', {
    path: '/tmp/after-run/workspace/note.txt',
  });
  assert.throws(() => comparePayloads(before, after), /invocation arguments envelope/);
});

test('same-engine comparator rejects changed semantic envelope metadata', () => {
  const before = makeToolPairRecord('/tmp/before-run/workspace', '2026-11-15 05:07:20', {
    arguments: { path: '/tmp/before-run/workspace/note.txt' },
  });
  const after = makeToolPairRecord('/tmp/after-run/workspace', '2026-11-15 05:07:20', {
    arguments: { path: '/tmp/after-run/workspace/note.txt' },
    elevation_reason: 'explicit escalation',
  });
  assert.throws(() => comparePayloads(before, after));
});

test('same-engine comparator rejects unknown extra request fields and changed tool schemas', () => {
  const before = makeToolPairRecord('/tmp/before-run/workspace', '2026-11-15 05:07:20', {
    arguments: { path: '/tmp/before-run/workspace/note.txt' },
  });
  const afterUnknown = makeToolPairRecord(
    '/tmp/after-run/workspace',
    '2026-11-15 05:07:20',
    { arguments: { path: '/tmp/after-run/workspace/note.txt' } },
    { temperature: 0.7 },
  );
  assert.throws(() => comparePayloads(before, afterUnknown));

  const afterToolDrift = makeToolPairRecord('/tmp/after-run/workspace', '2026-11-15 05:07:20', {
    arguments: { path: '/tmp/after-run/workspace/note.txt' },
  });
  afterToolDrift.wire_requests[0].body.tools[0].function.parameters.additionalProperties = true;
  assert.throws(() => comparePayloads(before, afterToolDrift));
});

test('same-engine comparator rejects workspace mismatch between system cwd and read tool call', () => {
  const before = makeToolPairRecord('/tmp/before-run/workspace', '2026-11-15 05:07:20', {
    arguments: { path: '/tmp/other-dir/workspace/note.txt' },
  });
  const after = makeToolPairRecord('/tmp/after-run/workspace', '2026-11-15 05:07:20', {
    arguments: { path: '/tmp/after-run/workspace/note.txt' },
  });
  assert.throws(() => comparePayloads(before, after));
});
