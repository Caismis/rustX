// Deterministic generated-protocol projection, never imported by production.
import { createRoot } from 'react-dom/client';
import { App } from '../../src/app/App';
import { Server, endpoint } from '../fixture';
import { cfg3Effective, cfg3Source } from '../cfg3-data';
import '../../src/presentation/theme/base.css';
import '../../src/presentation/theme/gradient-shadow-text.css';
import '../../src/presentation/theme/design-platform.css';
import '../../src/presentation/theme/scrollbar.css';
import '../../src/presentation/theme/corner-shape.css';
import '../../src/presentation/theme/shiki.css';
import '../../src/presentation/theme/reset.css';
import '../../src/app/console.css';
const server = new Server(), source = cfg3Source(), effective = cfg3Effective();
source.workspace.authored = { agent_id: 'rustx', agent: { description: 'A native coding assistant', instructions: 'Inspect the source, implement the change, and verify native boundaries.', tools: { builtin: ['read', 'glob'], sources: { 'python:analysis': 'all' } }, plugins: { goal: { enabled: true }, todo: { enabled: true } } }, models: effective.document.models, providers: source.user.authored!.providers };
effective.document.providers = source.user.authored!.providers;
effective.provenance = { 'models.main': { kind: 'workspace', base: '/workspace', document: '/workspace/rustx.toml' }, 'providers.transport': { kind: 'user', base: '/bound', document: '/bound/rustx.toml' } };
effective.resources.definitions = [
 { family: 'skill', name: 'review', valid: true, location: { scope: 'workspace', path: '/workspace/.agents/skills/review/SKILL.md', shadowed: '/home/user/rustx/.agents/skills/review/SKILL.md' } },
 { family: 'skill', name: 'incomplete', valid: false, location: { scope: 'workspace', path: '/workspace/.agents/skills/incomplete/SKILL.md' } },
 { family: 'managed_python', name: 'analysis', valid: true, location: { scope: 'user', path: '/home/user/rustx/.agents/python/analysis' } },
];
effective.resources.resource_diagnostics = [{ identity: 'incomplete', file: '/workspace/.agents/skills/incomplete/SKILL.md', reason: 'Missing package description' }];
effective.resources.sources = { 'python:analysis': { status: 'unprepared' } };
source.prospective_resources = effective.resources;
source.agents = [{ name: 'reviewer', scope: 'workspace', source: { path: '/workspace/.agents/agents/reviewer.toml', revision: 'agent-1', authored: { description: 'Independent code review', instructions: 'Inspect changed boundaries and report findings.', tools: { builtin: ['read', 'grep'] }, skills: ['review'] } } }];
server.handlers.set('configuration/sourcesRead', () => ({ type: 'source_settings', projection: source, session_revision: '1' }));
server.handlers.set('configuration/effective', () => ({ type: 'effective_configuration', projection: effective }));
await server.attached('A');
localStorage.setItem('rustx-console-view-v2', JSON.stringify({ endpoint, openViews: ['A'] }));
createRoot(document.getElementById('root')!).render(<App client={server.client} workspaceHost={server.workspaceHost} />);
