import { cleanup, fireEvent, render, screen, waitFor } from '@testing-library/react';
import { afterEach, expect, it, vi } from 'vitest';
import { trajectoryDurationStore, trajectoryStringWrappingStore } from '../src/app/trajectory/preferences';
import { WorkspaceFiles, fileTabView, type WorkbenchCall } from '../src/app/components/WorkbenchFiles';
import { SubagentsRunningIndicator, SubagentsSection } from '../src/app/agent/SessionPanelSubagents';
import { SubagentContext } from '../src/app/agent/subagent-context';
import type { RuntimeClientAgent } from '../../protocol/app-server/v80';

afterEach(() => {
  cleanup();
  trajectoryDurationStore.set(false);
  trajectoryStringWrappingStore.set(false);
});

it('persists trajectory duration and json string wrapping preferences in localStorage', () => {
  trajectoryDurationStore.set(true);
  trajectoryStringWrappingStore.set(true);
  expect(trajectoryDurationStore.getSnapshot()).toBe(true);
  expect(trajectoryStringWrappingStore.getSnapshot()).toBe(true);
  expect(localStorage.getItem('rustx-trajectory-duration')).toBe('true');
  expect(localStorage.getItem('rustx-trajectory-json-string-wrapping')).toBe('true');
});

it('WorkspaceFiles toggles search bar, searches workspace subdirectories, and opens matched files', async () => {
  const onOpen = vi.fn();
  const call: WorkbenchCall = vi.fn(async request => {
    if (request.kind === 'applications') return { applications: { available: true as const, applications: [] } };
    if (request.kind === 'files' && request.path === '') {
      return {
        cwd: '/repo',
        entries: [
          { name: 'README.md', directory: false, link: false },
          { name: 'src', directory: true, link: false },
          { name: 'node_modules', directory: true, link: false },
        ],
      };
    }
    if (request.kind === 'files' && request.path === 'src') {
      return {
        cwd: '/repo',
        entries: [
          { name: 'index.ts', directory: false, link: false },
          { name: 'helper.ts', directory: false, link: false },
        ],
      };
    }
    return { cwd: '/repo', entries: [] };
  });
  const view = fileTabView();
  render(<WorkspaceFiles call={call} view={view} onOpen={onOpen} />);
  await screen.findByRole('button', { name: 'README.md' });

  const toggle = screen.getByRole('button', { name: 'Search files' });
  fireEvent.click(toggle);
  const input = screen.getByRole('searchbox', { name: 'Search files' });
  fireEvent.change(input, { target: { value: 'helper' } });

  const match = await screen.findByRole('button', { name: /helper\.ts/ });
  fireEvent.click(match);
  expect(onOpen).toHaveBeenCalledWith('src/helper.ts');
  await waitFor(() => {
    expect(call).not.toHaveBeenCalledWith({ kind: 'files', path: 'node_modules' }, expect.anything());
  });
});

it('SubagentsSection lists running agents first, opens on click, and SubagentsRunningIndicator shows when closed', () => {
  const open = vi.fn();
  const openAside = vi.fn();
  const onOpened = vi.fn();
  const agent = (id: string, title: string, state: RuntimeClientAgent['state']): RuntimeClientAgent => ({
    agent_id: id,
    activation_id: `act-${id}`,
    parent_agent_id: null,
    child_conversation_id: `conv-${id}`,
    agent: 'worker',
    title,
    state,
    activation_state: state === 'inactive' ? 'succeeded' : 'running',
    observation: { activity: { type: 'thinking' } },
    task: 'task',
    created_at: '2026-11-20T00:00:00Z',
  } as unknown as RuntimeClientAgent);

  const agents = [
    agent('a1', 'Done Agent', 'inactive'),
    agent('a2', 'Live Agent', 'active'),
  ];
  const scope = {
    client: {} as never,
    sessionId: 's1',
    agents,
    metrics: {},
    metricErrors: {},
    opened: [],
    open,
    openAside,
    close: vi.fn(),
  };

  const ui = render(
    <SubagentContext value={scope}>
      <SubagentsRunningIndicator sessionId="s1" panelShown={false} />
      <SubagentsSection sessionId="s1" onOpened={onOpened} />
    </SubagentContext>,
  );

  expect(ui.container.querySelector('[data-subagents-running]')).not.toBeNull();
  const rows = [...ui.container.querySelectorAll<HTMLElement>('[data-session-panel-row]')];
  expect(rows.map(r => r.dataset.agentId)).toEqual(['a2', 'a1']);

  fireEvent.click(screen.getByRole('button', { name: 'Live Agent Working' }));
  expect(open).toHaveBeenCalledWith('a2');
  expect(onOpened).toHaveBeenCalledTimes(1);

  fireEvent.click(screen.getByRole('button', { name: 'Open Live Agent in sidebar' }));
  expect(openAside).toHaveBeenCalledWith('a2');
  expect(onOpened).toHaveBeenCalledTimes(2);
});
