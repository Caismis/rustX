// @vitest-environment jsdom
import { afterEach, expect, it } from 'vitest';
import { act, cleanup, fireEvent, renderHook, screen, waitFor } from '@testing-library/react';
import type { MethodResult, SourceSettings } from '../../protocol/app-server/v81';
import { ResourceBrowser } from '../src/app/resources/shared/ResourceBrowser';
import { useSkillCatalog } from '../src/app/resources/skills/useSkillCatalog';
import { cfg3Source } from './cfg3-data';
import { cfg3Client } from './cfg3-fixture';
import { chooseOption, renderEditor } from './settings-harness';

afterEach(cleanup);

function skillPage(names: string[]): MethodResult {
  return {
    type: 'skills',
    page: {
      data: names.map(name => ({
        catalog: { name, description: name, location: `/skills/${name}/SKILL.md` },
        provenance: { name, source: 'user' as const, location: `/skills/${name}/SKILL.md`, shadowed: [] },
        metadata_revision: 'skill-1',
        disable_model_invocation: false,
        enabled_by_default: true,
      })),
      revision: 'skills-1',
      diagnostics: [],
      diagnostics_truncated: false,
      next_offset: null,
    },
  } as MethodResult;
}

function userSource(revision: string, skills: 'all' | string[] = 'all'): SourceSettings {
  const source = cfg3Source();
  source.target = { kind: 'user' };
  source.user!.revision = revision;
  source.user!.authored = { agent: { skills } } as never;
  source.resolved = { agent: { skills } } as never;
  return source;
}

function gatedClient(held: 'user' | 'workspace' = 'user') {
  let releaseSecond!: (page: MethodResult) => void;
  const second = new Promise<MethodResult>(resolve => { releaseSecond = resolve; });
  let lists = 0;
  const subject = cfg3Client(async operation => {
    if (operation.method !== 'skills/list') return;
    if (operation.params.target.kind !== held) return skillPage(['a', 'b']);
    lists += 1;
    if (lists === 1) return skillPage(['a', 'b']);
    return second;
  });
  return { ...subject, releaseSecond, lists: () => lists };
}

function browser(client: ReturnType<typeof cfg3Client>['client'], source: SourceSettings, revision: string, scope: 'user' | 'workspace' = 'user') {
  return <ResourceBrowser client={client} source={source} scope={scope} revision={revision} query="" view="cards" refresh={0} onRefresh={() => {}}
    bindDisplayWrite={() => undefined} bindDelete={() => undefined} transferDisabled />;
}

it('keeps Workspace Skill replies scoped to the requested Session at the same path', async () => {
  const source = userSource('rev-1');
  source.target = { kind: 'workspace', directory: '/workspace/A' };
  let resolvePrevious!: (page: MethodResult) => void;
  const previous = new Promise<MethodResult>(resolve => { resolvePrevious = resolve; });
  const sessions: (string | null | undefined)[] = [];
  const { client } = cfg3Client(async operation => {
    if (operation.method !== 'skills/list') return;
    const session = operation.params.execution_session;
    sessions.push(session);
    return session === 'session-a' ? previous : skillPage(['current']);
  });
  const { result, rerender } = renderHook(
    ({ session }) => useSkillCatalog(client, source, true, 'workspace', 0, session),
    { initialProps: { session: 'session-a' } },
  );
  await waitFor(() => expect(sessions).toEqual(['session-a']));
  rerender({ session: 'session-b' });
  await waitFor(() => expect(result.current?.skills.map(skill => skill.catalog.name)).toEqual(['current']));
  await act(async () => resolvePrevious(skillPage(['retired'])));
  expect(result.current?.skills.map(skill => skill.catalog.name)).toEqual(['current']);
  expect(sessions).toEqual(['session-a', 'session-b']);
});

it('does not expand all from a pending catalog, then toggles and freezes the released names at the current revision', async () => {
  const gate = gatedClient();
  const first = userSource('rev-1');
  const { writes, rerender } = await renderEditor(browser(gate.client, first, 'rev-1'), { source: first, context: first });
  await screen.findByRole('switch', { name: 'Allow a for the agent' });
  const next = userSource('rev-2');
  rerender(browser(gate.client, next, 'rev-2'));
  await waitFor(() => expect(gate.lists()).toBe(2));
  const pending = screen.getByRole('switch', { name: 'Allow a for the agent' });
  expect((pending as HTMLButtonElement).disabled).toBe(true);
  fireEvent.click(pending);
  fireEvent.click(screen.getByRole('button', { name: (name: string) => name.endsWith('Skill access') }));
  const selected = await screen.findByRole('option', { name: 'Selected skills' });
  expect(selected.getAttribute('aria-disabled')).toBe('true');
  fireEvent.click(selected);
  fireEvent.keyDown(selected, { key: 'Escape' });
  expect(writes).not.toHaveBeenCalled();

  gate.releaseSecond(skillPage(['a', 'b', 'c']));
  await screen.findByRole('switch', { name: 'Allow c for the agent' });
  fireEvent.click(screen.getByRole('switch', { name: 'Allow a for the agent' }));
  await waitFor(() => expect(writes).toHaveBeenCalledWith({ kind: 'config', mutation: { unit: 'skills', authored: ['b', 'c'] } }, 'rev-2'));

  cleanup();
  const freeze = gatedClient();
  const frozenFirst = userSource('rev-1');
  const frozen = await renderEditor(browser(freeze.client, frozenFirst, 'rev-1'), { source: frozenFirst, context: frozenFirst });
  await screen.findByRole('switch', { name: 'Allow a for the agent' });
  const frozenNext = userSource('rev-2');
  frozen.rerender(browser(freeze.client, frozenNext, 'rev-2'));
  await waitFor(() => expect(freeze.lists()).toBe(2));
  expect(frozen.writes).not.toHaveBeenCalled();
  freeze.releaseSecond(skillPage(['a', 'b', 'c']));
  await waitFor(() => expect((screen.getByRole('switch', { name: 'Allow c for the agent' }) as HTMLButtonElement).disabled).toBe(false));
  await chooseOption('Skill access', 'Selected skills');
  await waitFor(() => expect(frozen.writes).toHaveBeenCalledWith({ kind: 'config', mutation: { unit: 'skills', authored: ['a', 'b', 'c'] } }, 'rev-2'));
});

it('still edits an exact selection and none while the catalog is pending', async () => {
  const gate = gatedClient();
  const first = userSource('rev-1', ['a', 'b']);
  const { writes, rerender } = await renderEditor(browser(gate.client, first, 'rev-1'), { source: first, context: first });
  await screen.findByRole('switch', { name: 'Allow a for the agent' });
  const next = userSource('rev-2', ['a', 'b']);
  rerender(browser(gate.client, next, 'rev-2'));
  await waitFor(() => expect(gate.lists()).toBe(2));
  const exact = screen.getByRole('switch', { name: 'Allow a for the agent' });
  expect((exact as HTMLButtonElement).disabled).toBe(false);
  fireEvent.click(exact);
  await waitFor(() => expect(writes).toHaveBeenCalledWith({ kind: 'config', mutation: { unit: 'skills', authored: ['b'] } }, 'rev-2'));

  cleanup();
  const none = gatedClient();
  const noneFirst = userSource('rev-1');
  const noneEditor = await renderEditor(browser(none.client, noneFirst, 'rev-1'), { source: noneFirst, context: noneFirst });
  await screen.findByRole('switch', { name: 'Allow a for the agent' });
  const noneNext = userSource('rev-2');
  noneEditor.rerender(browser(none.client, noneNext, 'rev-2'));
  await waitFor(() => expect(none.lists()).toBe(2));
  await chooseOption('Skill access', 'None');
  await waitFor(() => expect(noneEditor.writes).toHaveBeenCalledWith({ kind: 'config', mutation: { unit: 'skills', authored: [] } }, 'rev-2'));

  cleanup();
  const all = gatedClient();
  const allFirst = userSource('rev-1', ['a']);
  const allEditor = await renderEditor(browser(all.client, allFirst, 'rev-1'), { source: allFirst, context: allFirst });
  await screen.findByRole('switch', { name: 'Allow a for the agent' });
  const allNext = userSource('rev-2', ['a']);
  allEditor.rerender(browser(all.client, allNext, 'rev-2'));
  await waitFor(() => expect(all.lists()).toBe(2));
  await chooseOption('Skill access', 'All skills, including newly discovered');
  await waitFor(() => expect(allEditor.writes).toHaveBeenCalledWith({ kind: 'config', mutation: { unit: 'skills', authored: 'all' } }, 'rev-2'));

  cleanup();
  const inherited = gatedClient('workspace');
  const inheritedFirst = userSource('rev-1');
  inheritedFirst.target = { kind: 'workspace', directory: '/workspace/A' };
  inheritedFirst.workspace = inheritedFirst.user;
  const inheritedEditor = await renderEditor(browser(inherited.client, inheritedFirst, 'rev-1', 'workspace'), { source: inheritedFirst, context: inheritedFirst });
  await screen.findByRole('switch', { name: 'Allow a for the agent' });
  const inheritedNext = userSource('rev-2');
  inheritedNext.target = inheritedFirst.target;
  inheritedNext.workspace = inheritedNext.user;
  inheritedEditor.rerender(browser(inherited.client, inheritedNext, 'rev-2', 'workspace'));
  await waitFor(() => expect(inherited.lists()).toBe(2));
  await chooseOption('Skill access', 'Use global settings');
  await waitFor(() => expect(inheritedEditor.writes).toHaveBeenCalledWith({ kind: 'config', mutation: { unit: 'skills', authored: null } }, 'rev-2'));
});
