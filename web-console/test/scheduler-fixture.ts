import type { ScheduleHeader, ScheduleSnapshot, ScheduleCommandResult, ScheduledWorkflowRecovery } from '../../protocol/app-server/v72';
import { Server, snapshot } from './fixture';
import { cfg3Source } from './cfg3-data';
export const nodeA = 'node_00000000-0000-7000-8000-000000000001';
export const nodeB = 'node_00000000-0000-7000-8000-000000000002';
export function task(title = 'Review repository changes', session = 'A', node = nodeA, ordinal = '9007199254740993'): ScheduleSnapshot {
  return { id: { conversation_id: 'conversation-' + session, ordinal }, binding: { session_id: session, node_id: node, conversation_id: 'conversation-' + session },
    revision: '9007199254740993', title, target: { type: 'conversation_inbound', text: 'Review the latest changes and summarize anything that needs attention.' },
    spec: { type: 'cron', expression: '0 9 * * 1-5', time_zone: 'Asia/Shanghai' }, state: 'enabled', next_due_utc: '2028-11-01T01:00:00Z',
    last_trigger: null, completed_at: null, completion_reason: null, last_failure: null };
}
export function header(value: ScheduleSnapshot): ScheduleHeader {
  return { ...value, target: value.target.type === 'workflow' ? { type: 'workflow', workflow_id: 'review' } : { type: 'conversation_inbound' } };
}
export function schedulerFixture(values = [task()]) {
  const server = new Server(), rows = new Map(values.map(value => [value.id.ordinal, value]));
  const commands = new Map<string, ScheduleCommandResult>();
  const capabilities = { inputModalities: ['text' as const], outputModalities: ['text' as const], jsonSchemaOutput: false };
  server.workspaceHost.resolveWorkspace = async id => ({ cwd: id === 'workspace-b' ? '/workspace/B' : '/workspace' });
  server.workspaceHost.configureWorkspace = async id => ({ kind: 'read', projection: { ...cfg3Source(), target: { kind: 'workspace', directory: id === 'workspace-b' ? '/workspace/B' : '/workspace' }, prospective_approval_mode: 'policy',
    session_models: { kind: 'available', default_model: { model: 'fixture/native' }, catalog: { models: ['fixture/native', 'fixture/second'].map(model => ({ model, protocol: 'openai_responses', contextWindow: 8192, maxOutputTokens: 1024,
      credentialSource: { type: 'environment', variable: 'KEY' }, declaredCapabilities: capabilities, effectiveCapabilities: capabilities, profiles: [{ id: 'medium' }, { id: 'high' }], defaultProfile: 'medium' })) } } } });
  let sessionSequence = 0;
  server.handlers.set('session/create', request => {
    if (request.method !== 'session/create') throw Error('create');
    const id = 'scheduled-' + ++sessionSequence;
    server.snapshots.set(id, snapshot(id));
    server.summaries.set(id, { cwd: request.params.settings.cwd, active_node: 'node-' + id });
    return { type: 'session_transition', session: { id, active_node: 'node-' + id, active_conversation_id: 'conversation-' + id, node_count: 1, created_at: '0', updated_at: '0' } };
  });
  server.handlers.set('schedule/catalog', request => {
    if (request.method !== 'schedule/catalog') throw Error('catalog');
    const query = request.params.query?.toLocaleLowerCase();
    return { type: 'schedule_catalog', page: { entries: [...rows.values()].filter(row => (!request.params.session_id || row.binding.session_id === request.params.session_id)
      && (!query || JSON.stringify(row).toLocaleLowerCase().includes(query))).map(row => ({ schedule: header(row), session: server.summary(row.binding.session_id) })), next: null, unavailable: [] } };
  });
  server.handlers.set('schedule/list', request => {
    if (request.method !== 'schedule/list') throw Error('list');
    return { type: 'schedules', schedules: [...rows.values()].filter(row => row.binding.session_id === request.params.session_id && row.binding.node_id === request.params.node_id).map(header), next_ordinal: null };
  });
  server.handlers.set('schedule/get', request => {
    if (request.method !== 'schedule/get') throw Error('get');
    return { type: 'schedule', schedule: rows.get(request.params.id.ordinal) ?? null };
  });
  server.handlers.set('schedule/command', request => {
    if (request.method !== 'schedule/command') throw Error('command');
    const command = request.params.command, mutation = command.mutation;
    const held = commands.get(command.command_id); if (held) return { type: 'schedule_command', result: held };
    if (mutation.type === 'create') {
      const ordinal = String([...rows.values()].reduce((max, value) => BigInt(value.id.ordinal) > max ? BigInt(value.id.ordinal) : max, 0n) + 1n);
      const next = task(mutation.title, request.params.session_id, request.params.node_id, ordinal);
      next.target = { type: 'conversation_inbound', text: mutation.text };
      const timing = mutation.timing;
      if (timing.type === 'at') { next.spec = { type: 'at', at_utc: timing.at }; next.next_due_utc = timing.at; }
      if (timing.type === 'cron') next.spec = { type: 'cron', expression: timing.expression, time_zone: timing.time_zone };
      if (timing.type === 'interval') next.spec = { type: 'interval', every_seconds: timing.every_seconds, anchor_utc: new Date().toISOString() };
      rows.set(ordinal, next);
      const result: ScheduleCommandResult = { status: 'applied', receipt: { id: next.id, revision: next.revision, spec: next.spec, state: next.state, next_due_utc: next.next_due_utc } };
      commands.set(command.command_id, result);
      return { type: 'schedule_command', result };
    }
    if (!('id' in mutation)) throw Error('Unsupported task mutation');
    const row = rows.get(mutation.id.ordinal);
    let result: ScheduleCommandResult;
    if (!row) result = { status: 'not_found' };
    else if ('expected_revision' in mutation && row.revision !== mutation.expected_revision) result = { status: 'conflict', current: row };
    else {
      const revision = String(BigInt(row.revision) + 1n);
      if (mutation.type === 'delete') { rows.delete(row.id.ordinal); result = { status: 'deleted', id: row.id, revision }; }
      else {
        const next = { ...row, revision };
        if (mutation.type === 'set_enabled') next.state = mutation.enabled ? 'enabled' : 'disabled';
        if (mutation.type === 'update' || mutation.type === 'update_workflow' || mutation.type === 'update_metadata') {
          if (mutation.title != null) next.title = mutation.title;
          if (mutation.type === 'update' && mutation.text != null) next.target = { type: 'conversation_inbound', text: mutation.text };
          if (mutation.type === 'update_workflow') next.target = { type: 'workflow', pinned: {}, arguments: mutation.arguments ?? {} };
          const timing = mutation.timing;
          if (timing?.type === 'at') { next.spec = { type: 'at', at_utc: timing.at }; next.next_due_utc = timing.at; }
          if (timing?.type === 'cron') next.spec = { type: 'cron', expression: timing.expression, time_zone: timing.time_zone };
          if (timing?.type === 'interval') next.spec = { type: 'interval', every_seconds: timing.every_seconds, anchor_utc: new Date().toISOString() };
        }
        rows.set(row.id.ordinal, next);
        result = { status: 'applied', receipt: { id: next.id, revision, spec: next.spec, state: next.state, next_due_utc: next.next_due_utc } };
      }
    }
    commands.set(command.command_id, result);
    return { type: 'schedule_command', result };
  });
  return { server, rows, notify: () => server.socket.deliver({ jsonrpc: '2.0', method: 'schedule/changed', params: {} }) };
}
export function workflowFixture(): { task: ScheduleSnapshot; recovery: ScheduledWorkflowRecovery } {
  const value = task('Summarize the weekly review'); value.target = { type: 'workflow', pinned: {}, arguments: { branch: 'main' } };
  const startId = 'swf_' + 'a'.repeat(64), time = '2026-10-24T10:00:00Z';
  const run = { conversation_id: value.id.conversation_id, origin: { type: 'scheduled' as const, start_id: startId }, invocation: '1' };
  const trigger = { schedule_id: value.id, admission_revision: value.revision, occurrence_utc: time };
  value.last_trigger = { trigger_id: trigger, accepted_at: time, target: { type: 'workflow', start_id: startId, run_id: run } };
  return { task: value, recovery: { start: { id: startId, run_id: run, trigger_id: trigger, binding: value.binding, accepted_at: time,
    state: { type: 'claimed', owner: 'native-owner', claimed_at: time, effects_started: true }, cancellation_requested_at: time,
    recovery_blocked: 'Original Bash supervisor has no positive physical proof', native_quiescence_proven: false },
    effects: [{ ordinal: '9007199254740993', node: { block: { run, definition: { workflow_id: 'review', blocks: [] }, invocations: [] }, node: 'review', visit: 0 }, kind: 'tool', owner: { type: 'preparing' }, physical_settlement_proven: false }],
    capacity_blocked: true, deletion_blocked: true, next_effect_ordinal: '9007199254740993' } };
}
