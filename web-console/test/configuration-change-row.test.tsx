// @vitest-environment jsdom
import { afterEach, expect, it } from 'vitest';
import { cleanup, fireEvent, render, screen } from '@testing-library/react';
import type { BoundedNames, ConfigurationChange } from '../../protocol/app-server/v76';
import { ConfigurationChangeRow, hasConfigurationChangeRow } from '../src/app/agent/ConfigurationChangeRow';
afterEach(cleanup);
const none: BoundedNames = { names: [], total: 0 };
const change = (added: BoundedNames, removed: BoundedNames, domains: ConfigurationChange['domains'] = ['tools']): ConfigurationChange => ({
  reason: 'source', previous_binding_revision: '1', binding_revision: '2', domains,
  tools: { added, removed, changed: none }, skills: { added: none, removed: none, changed: none }, instructions_bytes: '0', catalog_bytes: '0',
});

it('has a Chat row only for Tool additions and removals, as in the Harness hidden-rows contract', () => {
  expect(hasConfigurationChangeRow(change(none, none, ['instructions', 'context_policy']))).toBe(false);
  expect(hasConfigurationChangeRow({ ...change(none, none), tools: { added: none, removed: none, changed: { names: ['bash'], total: 1 } } })).toBe(false);
  expect(hasConfigurationChangeRow(change({ names: ['fetch'], total: 1 }, none))).toBe(true);
  const { container } = render(<ConfigurationChangeRow change={change(none, none, ['instructions'])}/>);
  expect(container.textContent).toBe('');
});

it('a single Tool is one compact row with no pointless expansion', () => {
  render(<ConfigurationChangeRow change={change(none, { names: ['read'], total: 1 })}/>);
  expect(screen.getByText('Tool removed: read')).toBeTruthy();
  expect(screen.queryByRole('button', { expanded: false })).toBeNull();
});

it('a single Tool whose name the native bound omitted shows its count, never an empty name', () => {
  for (const [omitted, summary] of [[change({ names: [], total: 1 }, none), '1 added'], [change(none, { names: [], total: 1 }), '1 removed']] as const) {
    const { container, unmount } = render(<ConfigurationChangeRow change={omitted}/>);
    expect(screen.getByText('Tools updated')).toBeTruthy();
    expect(screen.getByText(summary)).toBeTruthy();
    expect(container.textContent).not.toMatch(/: ?$/m);
    expect(screen.queryByRole('button', { expanded: false })).toBeNull();
    unmount();
  }
});

it('several Tools show counts collapsed and grouped bounded names expanded', () => {
  render(<ConfigurationChangeRow change={change({ names: ['fetch', 'search'], total: 18 }, { names: ['read'], total: 1 })}/>);
  expect(screen.getByText('Tools updated')).toBeTruthy();
  expect(document.querySelector('[data-configuration-change]')!.textContent).toMatch(/18.*1/);
  fireEvent.click(screen.getByText('Tools updated'));
  expect(screen.getByText(/fetch, search/).textContent).toMatch(/16/);
  expect(screen.getByText('Removed: read')).toBeTruthy();
});
