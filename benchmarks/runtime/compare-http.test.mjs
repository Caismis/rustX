import test from 'node:test';
import assert from 'node:assert/strict';
import { MODEL_TIME } from './pi-oracle.mjs';
import { comparePayloads } from './compare-http.mjs';

const CORE_GUIDANCE = 'You are an expert coding assistant operating inside rustX.';
const PERMISSION_REMINDER = 'Execution permission mode: full_access.';

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

function makeShortOrCancelRecord(caseName, workspace, promptCwd, timestamp, guidance = CORE_GUIDANCE, extraBody = {}) {
  return {
    workload_version: 3,
    model_time: MODEL_TIME,
    measurement_version: 3,
    case: caseName,
    workspace,
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
              content: promptCwd === null
                ? guidance
                : `${guidance}\n\nCurrent working directory: ${promptCwd}\n\n${PERMISSION_REMINDER}`,
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
    ],
  };
}

function makeToolPairRecord(workspace, timestamp, toolCallArgs, promptCwd = workspace, guidance = CORE_GUIDANCE, extraBody = {}) {
  return {
    workload_version: 3,
    model_time: MODEL_TIME,
    measurement_version: 3,
    case: 'tool',
    workspace,
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
              content: `${guidance}\n\nCurrent working directory: ${promptCwd}\n\n${PERMISSION_REMINDER}`,
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
              content: `${guidance}\n\nCurrent working directory: ${promptCwd}`,
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

test('same-engine comparator accepts short-small and cancel without Read across distinct declared workspaces', () => {
  for (const caseName of ['short-small', 'cancel']) {
    const before = makeShortOrCancelRecord(
      caseName,
      '/tmp/before-run/workspace',
      '/tmp/before-run/workspace',
      MODEL_TIME,
    );
    const after = makeShortOrCancelRecord(
      caseName,
      '/tmp/after-run/workspace',
      '/tmp/after-run/workspace',
      MODEL_TIME,
    );
    const result = comparePayloads(before, after);
    assert.equal(result.case, caseName);
    assert.equal(result.checked.length, 1);
    assert.equal(result.checked[0].fullRequestEqual, true);
  }
});

test('same-engine comparator accepts nested arguments.path and distinct declared workspaces on tool case', () => {
  const before = makeToolPairRecord('/tmp/before-run/workspace', MODEL_TIME, {
    arguments: { path: '/tmp/before-run/workspace/note.txt' },
  });
  const after = makeToolPairRecord('/tmp/after-run/workspace', MODEL_TIME, {
    arguments: { path: '/tmp/after-run/workspace/note.txt' },
  });
  const result = comparePayloads(before, after);
  assert.equal(result.case, 'tool');
  assert.equal(result.checked.length, 2);
  assert.equal(result.checked[0].fullRequestEqual, true);
  assert.equal(result.checked[1].fullRequestEqual, true);
});

test('same-engine comparator rejects wrong CWD relative to declared workspace even when both sides agree', () => {
  for (const [declared, promptCwd] of [
    ['/tmp/run-a/workspace', '/tmp/wrong-cwd/workspace'],
    ['/tmp/run-a/workspace', '/tmp/run-a/workspace/nested'],
  ]) {
    const before = makeShortOrCancelRecord('cancel', declared, promptCwd, MODEL_TIME);
    const after = makeShortOrCancelRecord(
      'cancel',
      '/tmp/run-b/workspace',
      promptCwd,
      MODEL_TIME,
    );
    assert.throws(() => comparePayloads(before, after), /workspace cwd|section boundary/);
  }
});

test('same-engine comparator rejects missing workspace field and missing CWD line', () => {
  const noWorkspace = makeShortOrCancelRecord(
    'short-small',
    '/tmp/run-a/workspace',
    '/tmp/run-a/workspace',
    MODEL_TIME,
  );
  delete noWorkspace.workspace;
  const good = makeShortOrCancelRecord(
    'short-small',
    '/tmp/run-b/workspace',
    '/tmp/run-b/workspace',
    MODEL_TIME,
  );
  assert.throws(() => comparePayloads(noWorkspace, good), /missing immutable fixture workspace/);

  const noCwdLeft = makeShortOrCancelRecord('cancel', '/tmp/a/workspace', null, MODEL_TIME);
  const noCwdRight = makeShortOrCancelRecord('cancel', '/tmp/b/workspace', null, MODEL_TIME);
  assert.throws(() => comparePayloads(noCwdLeft, noCwdRight), /missing exact workspace cwd/);
});

test('same-engine comparator rejects changed system guidance', () => {
  const before = makeShortOrCancelRecord(
    'short-small',
    '/tmp/before-run/workspace',
    '/tmp/before-run/workspace',
    MODEL_TIME,
    CORE_GUIDANCE,
  );
  const after = makeShortOrCancelRecord(
    'short-small',
    '/tmp/after-run/workspace',
    '/tmp/after-run/workspace',
    MODEL_TIME,
    'Altered system guidance instruction.',
  );
  assert.throws(() => comparePayloads(before, after));
});

test('same-engine comparator rejects flat legacy read arguments without envelope', () => {
  const before = makeToolPairRecord('/tmp/before-run/workspace', MODEL_TIME, {
    path: '/tmp/before-run/workspace/note.txt',
  });
  const after = makeToolPairRecord('/tmp/after-run/workspace', MODEL_TIME, {
    path: '/tmp/after-run/workspace/note.txt',
  });
  assert.throws(() => comparePayloads(before, after), /invocation arguments envelope/);
});

test('same-engine comparator rejects changed semantic envelope metadata', () => {
  const before = makeToolPairRecord('/tmp/before-run/workspace', MODEL_TIME, {
    arguments: { path: '/tmp/before-run/workspace/note.txt' },
  });
  const after = makeToolPairRecord('/tmp/after-run/workspace', MODEL_TIME, {
    arguments: { path: '/tmp/after-run/workspace/note.txt' },
    elevation_reason: 'explicit escalation',
  });
  assert.throws(() => comparePayloads(before, after));
});

test('same-engine comparator rejects unknown extra request fields and changed tool schemas', () => {
  const before = makeToolPairRecord('/tmp/before-run/workspace', MODEL_TIME, {
    arguments: { path: '/tmp/before-run/workspace/note.txt' },
  });
  const afterUnknown = makeToolPairRecord(
    '/tmp/after-run/workspace',
    MODEL_TIME,
    { arguments: { path: '/tmp/after-run/workspace/note.txt' } },
    '/tmp/after-run/workspace',
    CORE_GUIDANCE,
    { temperature: 0.7 },
  );
  assert.throws(() => comparePayloads(before, afterUnknown));

  const afterToolDrift = makeToolPairRecord('/tmp/after-run/workspace', MODEL_TIME, {
    arguments: { path: '/tmp/after-run/workspace/note.txt' },
  });
  afterToolDrift.wire_requests[0].body.tools[0].function.parameters.additionalProperties = true;
  assert.throws(() => comparePayloads(before, afterToolDrift));
});

test('same-engine comparator rejects outside-fixture read path even when both sides agree', () => {
  const before = makeToolPairRecord('/tmp/before-run/workspace', MODEL_TIME, {
    arguments: { path: '/etc/passwd' },
  });
  const after = makeToolPairRecord('/tmp/after-run/workspace', MODEL_TIME, {
    arguments: { path: '/etc/passwd' },
  });
  assert.throws(() => comparePayloads(before, after), /read tool path must target declared fixture workspace/);
});

test('comparator rejects a wrong fixed clock even when both sides agree', () => {
  const before = makeShortOrCancelRecord('cancel', '/tmp/before-run/workspace', '/tmp/before-run/workspace', '2026-10-19 00:00:01');
  const after = makeShortOrCancelRecord('cancel', '/tmp/after-run/workspace', '/tmp/after-run/workspace', '2026-10-19 00:00:01');
  assert.throws(() => comparePayloads(before, after), /fixed fixture clock/);
});

test('comparator rejects wrong workload version and model_time identity', () => {
  const before = makeShortOrCancelRecord('short-small', '/tmp/before-run/workspace', '/tmp/before-run/workspace', MODEL_TIME);
  const after = makeShortOrCancelRecord('short-small', '/tmp/after-run/workspace', '/tmp/after-run/workspace', MODEL_TIME);
  before.workload_version = 2;
  assert.throws(() => comparePayloads(before, after), /workload_version/);
  before.workload_version = 3;
  after.model_time = '2026-10-19 00:00:01';
  assert.throws(() => comparePayloads(before, after), /model_time/);
});

test('comparator rejects unknown, unpaired, and inconsistent observer modes', () => {
  const left = makeShortOrCancelRecord('cancel', '/tmp/before-run/workspace', '/tmp/before-run/workspace', MODEL_TIME);
  const right = makeShortOrCancelRecord('cancel', '/tmp/after-run/workspace', '/tmp/after-run/workspace', MODEL_TIME);
  left.observer_mode = 'background';
  assert.throws(() => comparePayloads(left, right), /not allowed/);

  left.observer_mode = 'live-subscriber';
  right.observer_mode = 'no-live-subscriber';
  left.samples = [{observer_mode: 'live-subscriber'}];
  right.samples = [{observer_mode: 'no-live-subscriber'}];
  assert.throws(() => comparePayloads(left, right), /paired observer mode/);

  right.observer_mode = 'live-subscriber';
  right.samples = [{observer_mode: 'native-workflow'}];
  assert.throws(() => comparePayloads(left, right), /per-turn observer mode/);

  right.samples = [{observer_mode: 'live-subscriber'}];
  const matched = comparePayloads(left, right);
  assert.equal(matched.observer_mode, 'live-subscriber');
  assert.equal(matched.model_time, MODEL_TIME);
  assert.equal(matched.workload_version, 3);
});
