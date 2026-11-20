import { act, cleanup, fireEvent, render, screen } from '@testing-library/react';
import { afterEach, expect, it, vi } from 'vitest';
import { ToolCard } from '../src/presentation/agent/ToolCard';
import { StepGroup } from '../src/presentation/agent/StepGroup';
import { MarkdownText } from '../src/presentation/markdown/MarkdownText';
import { FileMentionsContext, producedFileMentions } from '../src/presentation/markdown/FileLinks';
import { ChatViewport } from '../src/presentation/layout/ChatViewport';
import { ConversationStats } from '../src/app/agent/UsageStats';
import { conversationPreferences } from '../src/app/conversation-preferences';

afterEach(() => {
  cleanup();
  conversationPreferences().update({ performanceUsage: 'detailed' }, false);
});

it('ToolCard renders inline diff stats and clickable file link while keeping row click to expand', () => {
  const onOpenPath = vi.fn();
  const ui = render(
    <ToolCard
      onOpenPath={onOpenPath}
      tool={{
        title: 'edit',
        summary: 'src/app.ts',
        path: 'src/app.ts',
        variant: 'edit',
        state: 'success',
        diffs: [{ path: 'src/app.ts', oldText: 'const a = 1;\n', newText: 'const a = 2;\nconst b = 3;\n' }],
      }}
    />,
  );
  expect(ui.container.textContent).toContain('+2');
  expect(ui.container.textContent).toContain('-1');
  const link = screen.getByRole('button', { name: 'src/app.ts' });
  fireEvent.click(link);
  expect(onOpenPath).toHaveBeenCalledTimes(1);
  const row = ui.container.querySelector('[data-disclosure-row]') as HTMLElement;
  expect(row.getAttribute('aria-expanded')).toBe('false');
  fireEvent.click(row);
  expect(row.getAttribute('aria-expanded')).toBe('true');
});

it('ToolCard renders ExitCodeBadge on non-zero bash exit while keeping command description', () => {
  const ui = render(
    <ToolCard
      tool={{
        title: 'bash',
        summary: 'Run tests',
        input: 'npm test',
        output: 'FAIL',
        exitCode: 2,
        variant: 'bash',
        state: 'failure',
      }}
    />,
  );
  expect(ui.container.textContent).toContain('Run tests');
  expect(ui.container.textContent).toContain('Exit 2');
});

it('StepGroup shimmers when running, marks collapsed body hidden=until-found, and auto-reveals on beforematch', () => {
  const onReveal = vi.fn();
  const onToggle = vi.fn();
  const ui = render(
    <StepGroup
      id="g1"
      counts={[]}
      open={false}
      running
      liveActivity="read"
      activityTitle="Reading files"
      onReveal={onReveal}
      onToggle={onToggle}
    >
      <div>step details</div>
    </StepGroup>,
  );
  const button = ui.container.querySelector('[data-step-process] > button') as HTMLButtonElement;
  expect(button.getAttribute('data-process-activity')).toBe('read');
  expect(ui.container.querySelector('[data-shimmer="true"]')).not.toBeNull();
  const body = ui.container.querySelector('[data-step-process-body]') as HTMLDivElement;
  expect(body.getAttribute('hidden')).toBe('until-found');
  act(() => {
    body.dispatchEvent(new Event('beforematch'));
  });
  expect(onReveal).toHaveBeenCalledTimes(1);
});

it('MarkdownText turns unique inline-code produced file mentions into clickable buttons', () => {
  const openFile = vi.fn();
  const mentions = producedFileMentions(
    ['src/app/Foo.tsx', 'src/other/Bar.ts', 'a/dup.ts', 'b/dup.ts'],
    openFile,
    path => `Open ${path}`,
  );
  render(
    <FileMentionsContext.Provider value={mentions}>
      <MarkdownText text={'Updated `Foo.tsx`, `src/other/Bar.ts`, ambiguous `dup.ts`, and `unknown.ts`.'} />
    </FileMentionsContext.Provider>,
  );
  const fooBtn = screen.getByRole('button', { name: 'Open src/app/Foo.tsx' });
  fireEvent.click(fooBtn);
  expect(openFile).toHaveBeenCalledWith({ path: 'src/app/Foo.tsx' });
  const barBtn = screen.getByRole('button', { name: 'Open src/other/Bar.ts' });
  fireEvent.click(barBtn);
  expect(openFile).toHaveBeenCalledWith({ path: 'src/other/Bar.ts' });
  expect(screen.queryByRole('button', { name: /dup\.ts/ })).toBeNull();
  expect(screen.queryByRole('button', { name: /unknown\.ts/ })).toBeNull();
});

it('ConversationStats switches between detailed dialogs and compact plain readings', () => {
  const stats = {
    turns: '2',
    steps: '5',
    completed_responses: '2',
    model_requests: '5',
    requests_with_usage: '5',
    timing: { model_ms: 1200, tool_ms: 400, mean_ttft_ms: 150, output_tokens_per_second: 42.5 },
    reported_usage: { input_tokens: '1000', output_tokens: '200', total_tokens: '1200', details: { cached_input_tokens: '600' } },
  };
  const ui = render(<ConversationStats statistics={stats} />);
  expect(ui.container.querySelectorAll('button').length).toBe(2);
  expect(ui.container.textContent).toContain('2 turns');
  act(() => conversationPreferences().update({ performanceUsage: 'compact' }, false));
  expect(ui.container.querySelectorAll('button').length).toBe(0);
  expect(ui.container.textContent).toContain('43 token/s');
  expect(ui.container.textContent).toContain('60%');
  expect(ui.container.textContent).not.toContain('2 turns');
});

it('ChatViewport triggers onReachTop once per top-edge anchor when scrolling upward near the top', () => {
  const onReachTop = vi.fn();
  const ui = render(
    <ChatViewport onReachTop={onReachTop}>
      <div data-chat-anchor-key="m1">Row 1</div>
      <div data-chat-anchor-key="m2">Row 2</div>
    </ChatViewport>,
  );
  const scroller = ui.container.querySelector('.conversation-scroll') as HTMLDivElement;
  Object.defineProperty(scroller, 'clientHeight', { value: 400, configurable: true });
  Object.defineProperty(scroller, 'scrollHeight', { value: 1200, configurable: true });
  scroller.scrollTop = 800;
  fireEvent.scroll(scroller);
  expect(onReachTop).not.toHaveBeenCalled();
  scroller.scrollTop = 100;
  fireEvent.scroll(scroller);
  expect(onReachTop).toHaveBeenCalledTimes(1);
  scroller.scrollTop = 40;
  fireEvent.scroll(scroller);
  expect(onReachTop).toHaveBeenCalledTimes(1);
});

