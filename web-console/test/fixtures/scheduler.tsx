import { createRoot } from 'react-dom/client';
import { App } from '../../src/app/App';
import { schedulerFixture, task, workflowFixture } from '../scheduler-fixture';
import { localeController } from '../../src/locale/controller';
import { cfg3Source } from '../cfg3-data';
import { sourceReceipt } from '../fixture';
import '../../src/presentation/theme/base.css';
import '../../src/presentation/theme/gradient-shadow-text.css';
import '../../src/presentation/theme/design-platform.css';
import '../../src/presentation/theme/reset.css';
import '../../src/app/console.css';
const params = new URLSearchParams(location.search), zh = params.get('locale') === 'zh';
localeController.setLocale(zh ? 'zh' : 'en');
document.body.toggleAttribute('data-ds-dark-theme', params.get('theme') === 'dark');
const daily = task(zh ? '检查仓库最新变更' : 'Review repository changes');
daily.target = { type: 'conversation_inbound', text: zh ? '检查仓库最新变更，汇总需要关注的事项，并给出下一步建议。' : 'Review the latest repository changes, summarize anything that needs attention, and suggest next steps.' };
const paused = task(zh ? '整理研究笔记' : 'Organize research notes', 'A', daily.binding.node_id, '2'); paused.state = 'disabled';
paused.spec = { type: 'interval', every_seconds: 3600, anchor_utc: '2026-10-01T10:00:00Z' };
const once = task(zh ? '准备发布说明' : 'Prepare release notes', 'B', 'node-B', '3'); once.spec = { type: 'at', at_utc: '2028-11-03T02:00:00Z' };
const workflow = workflowFixture(); workflow.task.id.ordinal = '4'; workflow.task.title = zh ? '每周代码评审摘要' : 'Weekly code review summary';
const f = schedulerFixture([daily, paused, once, workflow.task]);
f.server.summaries.set('A', { active_node: daily.binding.node_id });
f.server.nodeSnapshots.set(daily.binding.node_id, f.server.snapshots.get('A')!);
f.server.workspaceHost.classifyLocations = async cwds => cwds.map(cwd => ({ authorized: true, workspaceId: cwd.endsWith('/B') ? 'workspace-b' : 'workspace-a' }));
f.server.workspaceHost.configureWorkspace = async (id, _endpoint, operation) => {
  const projection = { ...cfg3Source(), target: { kind: 'workspace' as const, directory: id === 'workspace-b' ? '/workspace/B' : '/workspace' } };
  if (operation.kind === 'write') return { kind: 'write', commit: { acknowledgement: sourceReceipt(projection), reread: { status: 'observed', projection } } };
  if (operation.kind === 'mcp_probe') return { kind: 'mcp_probe', result: { id: operation.id, revision: operation.expected_revision, outcome: 'reachable' } };
  return { kind: operation.kind, projection };
};
f.server.handlers.set('workflow/scheduled/getRecovery', () => ({ type: 'scheduled_workflow_recovery', recovery: workflow.recovery }));
f.server.handlers.set('workflow/scheduled/reconcile', () => ({ type: 'scheduled_workflow_recovery', recovery: workflow.recovery }));
await f.server.connect();
localStorage.removeItem('rustx-console-view-v2');
Object.assign(window, { schedulerFixture: {
  requests: () => f.server.requests.map(row => row.request),
  changeTitle: () => { const row = f.rows.get(daily.id.ordinal)!; f.rows.set(row.id.ordinal, { ...row, title: zh ? '其他窗口已修改' : 'Changed in another window', revision: String(BigInt(row.revision) + 1n) }); f.notify(); },
} });
createRoot(document.getElementById('root')!).render(<App client={f.server.client} workspaceHost={f.server.workspaceHost}/>);
