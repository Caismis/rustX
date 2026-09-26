import { translator } from '../src/locale/translation';
// @vitest-environment jsdom
import { afterEach, expect, it, vi } from 'vitest';
import { cleanup, fireEvent, render, screen } from '@testing-library/react';
import { ArtifactPreview } from '../src/presentation/right-panel/ArtifactPreview';
import { readTheme, applyTheme } from '../src/app/appearance';
import { goalActivityLabel } from '../src/app/agent/GoalActivity';
import type { ForegroundToolExecution } from '../../protocol/app-server/v23';
afterEach(() => { cleanup(); localStorage.clear(); document.body.removeAttribute('data-ds-dark-theme'); });
it('keeps preview content inert, wrapping local, and error retries explicit', () => {
 const retry = vi.fn();
 const ui = render(<ArtifactPreview name="report.html" image={false} text="<script>unsafe()</script>" loading={false} retry={retry} />);
 expect(screen.getByText('<script>unsafe()</script>')).toBeTruthy(); expect(ui.container.querySelector('script')).toBeNull();
 fireEvent.click(screen.getByRole('button', { name: 'Wrap lines' })); expect(screen.getByRole('button', { name: 'Wrap lines' }).getAttribute('aria-pressed')).toBe('false');
 ui.rerender(<ArtifactPreview name="report" image={false} loading={false} error="Artifact unavailable" retry={retry} />);
 expect(retry).not.toHaveBeenCalled(); fireEvent.click(screen.getByRole('button', { name: 'Retry preview' })); expect(retry).toHaveBeenCalledOnce();
});
it('persists only the safe Web appearance preference', () => {
 expect(readTheme()).toBe('light'); applyTheme('dark'); expect(readTheme()).toBe('dark'); expect(document.body.hasAttribute('data-ds-dark-theme')).toBe(true);
 expect({ ...localStorage }).toEqual({ 'rustx-appearance-v1': 'dark', 'rustx-locale-v1': 'en' });
 localStorage.setItem('rustx-appearance-v1', 'unexpected'); expect(readTheme()).toBe('light');
});
it('Goal Tool labels describe native outcomes while preserving exact tool identity', () => {
 const tool: ForegroundToolExecution = { call_id: 'goal-call', tool_id: 'native.create_goal', name: 'create_goal', state: { type: 'assembled', arguments: '{}' } } as ForegroundToolExecution;
 expect(goalActivityLabel(translator('en'), tool)).toBe('Starting Goal');
 expect(goalActivityLabel(translator('en'), { ...tool, state: { type: 'settled', arguments: '{}', result: { status: { type: 'success' }, content: [], duration_ms: 1 } } })).toBe('Goal started');
});
