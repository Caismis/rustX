import { expect, it } from 'vitest';
import { WorkspaceSelectionPreference } from '../src/workspaces/selection';
function storage() {
  const values = new Map<string, string>();
  return { getItem: (key: string) => values.get(key) ?? null, setItem: (key: string, value: string) => { values.set(key, value); } };
}
it('persists a choice immediately and restores it in a new owner, scoped to normalized endpoints', () => {
  const disk = storage(), first = new WorkspaceSelectionPreference(disk);
  first.select('ws://LOCALHOST:80', 'registered-a'); first.select('ws://other:8080', 'registered-b');
  const restarted = new WorkspaceSelectionPreference(disk);
  expect(restarted.read('ws://localhost/')).toBe('registered-a');
  expect(restarted.read('ws://other:8080/')).toBe('registered-b');
  expect(restarted.read('ws://different:8080')).toBeUndefined();
});
it('invalid storage does not block a later usable choice', () => {
  const disk = storage(); disk.setItem('rustx-workspace-selection-v1', 'broken JSON');
  const first = new WorkspaceSelectionPreference(disk); expect(first.read('ws://localhost/')).toBeUndefined();
  first.select('ws://localhost/', 'registered-a'); expect(new WorkspaceSelectionPreference(disk).read('ws://localhost/')).toBe('registered-a');
});
it('storage denial leaves the current selection usable', () => {
  const first = new WorkspaceSelectionPreference({ getItem() { throw new Error('denied'); }, setItem() { throw new Error('denied'); } });
  first.select('ws://localhost/', 'registered-a'); expect(first.read('ws://localhost/')).toBe('registered-a');
});
