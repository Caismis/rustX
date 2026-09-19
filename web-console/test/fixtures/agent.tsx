// Deterministic native protocol fixture, never part of the production entry.
import { useState } from 'react';
import { AgentComposer } from '../../src/app/agent/AgentComposer';
import { createRoot } from 'react-dom/client';
import { RpcFailure } from '../../src/client/app-server';
import { App } from '../../src/app/App';
import { Server, interaction, snapshot, endpoint } from '../fixture';
import type { CatalogModelView, SourceSettings, SessionModelView } from '../../../protocol/app-server/v9';
import '../../src/presentation/theme/base.css';
import '../../src/presentation/theme/gradient-shadow-text.css';
import '../../src/presentation/theme/design-platform.css';
import '../../src/presentation/theme/scrollbar.css';
import '../../src/presentation/theme/corner-shape.css';
import '../../src/presentation/theme/shiki.css';
import '../../src/presentation/theme/reset.css';
import '../../src/app/console.css';
const mode = new URLSearchParams(location.search).get('mode') ?? 'settled';
const server = new Server();
server.handlers.set('configuration/effective', () => { throw new RpcFailure({ code: -32000, message: 'Appearance fixture' }); });
const capabilities = { inputModalities: ['text' as const], outputModalities: ['text' as const], toolCalls: true, reasoning: true };
const model: SessionModelView = { configured: { model: 'native/coder' }, effective: { model: 'native/coder', protocol: 'openai_responses', contextWindow: 128000, modelMaxOutputTokens: 8192, maxOutputTokens: 8192, reasoningEnabled: true, reasoningProfile: 'deliberate', capabilities, declaredCapabilities: capabilities }, summary: { mode: 'session' } };
const catalog: CatalogModelView[] = [{ model: 'native/coder', protocol: 'openai_responses', contextWindow: 128000, maxOutputTokens: 8192, declaredCapabilities: capabilities, effectiveCapabilities: capabilities, reasoningProfiles: [{ id: 'deliberate', enabled: true }, { id: 'brief', enabled: true }], defaultReasoningProfile: 'deliberate', credentialSource: { type: 'literal' } }];
const source: SourceSettings = { prospective_approval_mode: 'full_access', absent_resource_revision: 'absent', resource_revisions: {}, loaded: { generation: '1', pending_reload: true, changed_sources: ['/workspace/A/rustx.toml'] }, user: { path: '/home/rustx.toml', revision: 'user', authored: {} }, workspace: { path: '/workspace/A/rustx.toml', revision: 'workspace', authored: {} }, user_resource_root: '/home/.agents', workspace_resource_root: '/workspace/A/.agents', runtime_root: '/runtime', user_mcp: { path: '/home/.agents/mcp.toml', revision: 'missing' }, workspace_mcp: { path: '/workspace/A/.agents/mcp.toml', revision: 'missing' }, agents: [] };
server.handlers.set('settings/model', () => ({ type: 'model', model }));
server.handlers.set('settings/models', () => ({ type: 'models', catalog: { models: catalog } }));
server.handlers.set('configuration/sourcesRead', () => ({ type: 'source_settings', projection: source, session_revision: '1' }));
const s = snapshot(); s.model = model;
s.messages = [
 { role: 'user', id: 'user-1', source: 'human', content: [{ type: 'text', text: 'Inspect the native runtime boundary and summarize the change.' }, { type: 'uploaded_file', batch_id: 'batch-1', name: 'runtime-notes.md' }] },
 { role: 'assistant', id: 'answer-1', content: [{ type: 'reasoning', text: 'I will inspect the runtime owner before changing its presentation.' }, { type: 'text', text: '## Native authority, clear presentation\n\nThe Agent view reads the canonical conversation and the runtime’s current projection.\n\n```rust\nlet snapshot = runtime.snapshot();\n```\n\n- History stays ordered by the native transcript.\n- Tool outcomes come from the runtime.\n- Pending interactions survive reconnect.' }] },
];
s.transcript.entries = s.messages.map((message, i) => ({ cursor: String(i + 1), item: { type: 'message', message } }));
if (mode !== 'settled' && mode !== 'composer') s.attempt = { attempt_id: 'attempt-1', phase: { type: 'running' }, turn: 1, execution_settings: { resource_revision: '1', approval_mode: 'policy' } };
if (mode === 'streaming') {
 s.messages.pop(); s.transcript.entries.pop();
 s.attempt!.in_flight = { message_id: 'streaming-1', blocks: [{ type: 'reasoning', block_index: 0, text: 'Checking the native Tool lifecycle…\nThe canonical ledger owns committed output.' }, { type: 'text', block_index: 1, text: 'The browser renders a replaceable projection. It does not reconstruct execution from events.' }] };
}
if (mode === 'tools' || mode === 'error') {
 const calls = [{ id: 'read-1', tool_id: 'tool-read', name: 'read', arguments: { path: 'src/runtime_client/snapshot.rs' } }, { id: 'bash-1', tool_id: 'tool-bash', name: 'bash', arguments: { command: 'cargo test --lib' } }, { id: 'edit-1', tool_id: 'tool-edit', name: 'edit', arguments: { path: 'src/main.rs', edits: [{ oldText: 'legacy_view()', newText: 'native_projection()' }] } }];
 s.messages[1] = { role: 'assistant', id: 'calls-1', content: [{ type: 'reasoning', text: 'Inspect the projection, then validate the native contract.' }, ...calls.map(call => ({ type: 'tool_call' as const, ...call }))] };
 s.transcript.entries[1] = { cursor: '2', item: { type: 'message', message: s.messages[1] }, tool_calls: calls.map((call, index) => ({ message_id: 'calls-1', block_index: index + 1, call_id: call.id, tool_id: call.tool_id, name: call.name, state: index === 1 && mode === 'tools' ? { type: 'running', arguments: JSON.stringify(call.arguments) } : { type: 'settled', arguments: JSON.stringify(call.arguments), result: { status: index === 1 ? { type: 'failed', error: 'Native test failure: assertion failed' } : { type: 'success' }, duration_ms: 12, content: [{ type: 'text', text: index === 0 ? 'pub struct RuntimeClientSnapshot {\n    pub conversation_id: ConversationId,\n}' : index === 2 ? 'Replaced one occurrence.' : 'test native_boundary ... FAILED' }] } } })) };
}
if (mode === 'approval') s.pending_interactions = [interaction('approval')];
if (mode === 'questionnaire') {
 const pending = interaction('questionnaire');
 if (pending.request.kind.type === 'questionnaire') pending.request.kind.questionnaire.questions.push({ header: 'Coverage', question: 'Which checks should be included?', answer: { type: 'multi_choice', min_selected: 1, max_selected: 2, allow_custom: true, options: [{ label: 'Native contracts', description: 'Verify runtime authority.' }, { label: 'Browser references', description: 'Verify the pinned presentation.' }] } });
 s.pending_interactions = [pending];
}
server.snapshots.set('A', s);
await server.attached('A');
localStorage.setItem('rustx-console-view-v2', JSON.stringify({ endpoint, openViews: ['A'] }));
createRoot(document.getElementById('root')!).render(mode === 'restored' ? <RestoredComposerFixture /> : <App client={server.client} workspaceHost={server.workspaceHost}/>);

// Browser-only controls for deterministic composer projection/transport tests.
// No fixture code enters the production bundle.
if (mode === 'composer') {
  server.handlers.set('session/upload', request => {
    if (request.method !== 'session/upload') throw new Error('Wrong method');
    return { type: 'session_uploaded', files: request.params.files.map((file, index) => ({
      receipt: { session_id: 'A', batch_id: 'batch', token: `file-${index}` },
      file: { batch_id: 'batch', name: file.name }, path: `/workspace/A/.agents/uploads/A/batch/${file.name}`,
    })) };
  });
  window.composerFixture = {
    running: async value => { const next = structuredClone(server.snapshots.get('A')!); next.attempt = value ? { attempt_id: 'attempt-A', phase: { type: 'running' }, turn: 1, execution_settings: { resource_revision: '1', approval_mode: 'policy' } } : null; await server.update('A', next); },
    docks: async value => {
      const next = structuredClone(server.snapshots.get('A')!);
      next.todos = value ? { next_id: '2', tasks: [{ id: '1', subject: 'Review the composer', status: 'in_progress', active_form: 'Reviewing the composer' }] } : null;
      next.goal = value ? { current: { reference: { id: 'goal', revision: '1' }, objective: 'Restore the compact input contract', phase: 'paused', autonomous_round_budget: 4, autonomous_rounds_consumed: 1, origin: { kind: 'runtime_control' } } } : null;
      next.inbound = { pending: value ? [{ sequence: '1', revision: '0', message: { id: 'queued', source: 'human', content: [{ type: 'text', text: 'Verify the native queue controls' }] } }] : [] };
      await server.update('A', next);
    },
    submissions: () => server.requests.flatMap(row => row.request.method === 'turn/start' || row.request.method === 'turn/steer' ? [row.request.method] : []),
  };
}
declare global {
  interface Window { composerFixture: { running(value: boolean): Promise<void>; docks(value: boolean): Promise<void>; submissions(): string[] } }
}

function RestoredComposerFixture() {
  const [session, setSession] = useState('A');
  const text = session === 'A' ? Array.from({ length: 40 }, (_, i) => `Restored line ${i}`).join('\n') : 'Other Session draft';
  return <><button onClick={() => setSession(value => value === 'A' ? 'B' : 'A')}>Switch Session</button>
    <AgentComposer key={session} disabled={false} busy={false} active={false} initialContent={[{ type: 'text', text }]}
      onSend={async () => false} onUpload={async () => []} onCancel={() => {}} /></>;
}
