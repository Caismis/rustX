import { cleanup, render } from '@testing-library/react';
import { afterEach, expect, it, vi } from 'vitest';
import { WorkspaceFile, fileTabView, type WorkbenchCall } from '../src/app/components/WorkbenchFiles';
import { WorkspaceHostError } from '../src/workspaces/host';
import { localeController } from '../src/locale/controller';

afterEach(cleanup);
it.each([
  ['en', 'archive_rejected', 'OOXML archive rejected'],
  ['zh', 'archive_rejected', 'OOXML 压缩包被拒绝'],
  ['en', 'converter_timeout', 'timed out'],
  ['zh', 'internal diagnostic that must not be shown', '文档预览失败'],
] as const)('localizes Office failures in %s: %s', async (locale, reason, message) => {
  localeController.setLocale(locale);
  const call = vi.fn<WorkbenchCall>(async request => {
    if (request.kind === 'office') throw new WorkspaceHostError(`Workspace Host: Error: ${reason}`);
    return {};
  });
  const view = render(<WorkspaceFile path="report.pptx" call={call} view={fileTabView()} onOpen={() => {}}/>);
  const alert = await view.findByRole('alert');
  expect(alert.textContent).toContain(message);
  expect(alert.textContent).not.toContain('WorkspaceHostError');
  expect(alert.textContent).not.toContain('internal diagnostic');
});
