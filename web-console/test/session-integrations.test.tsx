// @vitest-environment jsdom
import { cleanup, fireEvent, render, screen, waitFor } from '@testing-library/react';
import { afterEach, expect, it, vi } from 'vitest';
import type { MethodResult, Request1 } from '../../protocol/app-server/v5';
import { type AppServerClient, RpcFailure } from '../src/client/app-server';
import { SessionIntegrations } from '../src/app/settings/SessionIntegrations';
afterEach(cleanup);
it('retains Session selection draft on CAS conflict while preserving refreshed unrelated settings', async () => {
  let writes = 0;
  const value: Extract<MethodResult, { type: 'settings' }> = { type: 'settings', revision: '1', settings: { cwd: '/workspace', model: { model: 'native/old' }, no_automatic_skills: false, no_direct_tools: false, no_builtin_tools: false } };
  const request = vi.fn(async (operation: Request1) => {
    if (operation.method === 'settings/replace') {
      if (!writes++) { value.revision = '2'; value.settings.model = { model: 'native/new' }; throw new RpcFailure({ code: -32000, message: 'Conflict', data: { kind: 'stale_settings', expected: '1', actual: '2' } }); }
      return { type: 'settings_replaced', revision: '3' };
    }
    return structuredClone(value);
  });
  render(<SessionIntegrations client={{ request } as unknown as AppServerClient} sessionId="A" />);
  fireEvent.click(await screen.findByLabelText('No automatic Skills'));
  fireEvent.change(screen.getByLabelText('Explicit Skill paths (one per line)'), { target: { value: '/existing/SKILL.md' } });
  fireEvent.click(screen.getByRole('button', { name: 'Save Session integrations' }));
  await screen.findByText(/Save refused; draft retained/); await waitFor(() => expect(request).toHaveBeenCalledTimes(3));
  expect((screen.getByLabelText('No automatic Skills') as HTMLInputElement).checked).toBe(true);
  fireEvent.click(screen.getByRole('button', { name: 'Save Session integrations' }));
  await waitFor(() => expect(writes).toBe(2));
  expect(request.mock.calls[3][0]).toMatchObject({ method: 'settings/replace', params: { expected_revision: '2', settings: { model: { model: 'native/new' }, skill_paths: ['/existing/SKILL.md'], no_automatic_skills: true } } });
  expect(JSON.stringify(request.mock.calls)).not.toContain('mcp_servers');
});
