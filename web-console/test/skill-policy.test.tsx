import { cleanup, fireEvent, render, screen, waitFor, within } from '@testing-library/react';
import { afterEach, expect, it } from 'vitest';
import { cfg3Client, cfg3Host } from './cfg3-fixture';
import { chooseOption, openResourceModule, settingsReady, SettingsSurface } from './settings-harness';
import { userSettingsTarget, workspaceSettingsTarget } from '../src/app/settings/projection';

afterEach(cleanup);
const writes = (subject: ReturnType<typeof cfg3Client>) => subject.request.mock.calls.flatMap(([op]) => op.method === 'configuration/sourceWrite' ? [op.params] : []);

async function skills(workspace = false) {
  let version = 1;
  const subject = cfg3Client(async (op, source) => {
    if (op.method === 'configuration/sourcesRead') {
      const authored = op.params.target.kind === 'workspace' ? source.workspace?.authored?.agent?.skills : undefined;
      source.resolved = { agent: { skills: authored ?? source.user.authored?.agent?.skills ?? 'all' } };
    }
    if (op.method !== 'configuration/sourceWrite' || op.params.mutation.kind !== 'config' || op.params.mutation.mutation.unit !== 'skills') return;
    const document = source[op.params.target.kind]!;
    document.authored!.agent ??= {};
    const value = op.params.mutation.mutation.authored;
    if (value === null) delete document.authored!.agent.skills;
    else document.authored!.agent.skills = value;
    document.revision = `${op.params.target.kind}-${++version}`;
    return { type: 'sources_written', revision: document.revision, configuration_revision: null };
  });
  subject.source.user.authored!.agent = { skills: 'all' };
  subject.source.prospective_resources = { ...subject.effective.resources, definitions: ['a', 'b'].map(name => ({
    family: 'skill', name, valid: true, location: { scope: 'user', path: `/skills/${name}/SKILL.md` },
  })) };
  render(<SettingsSurface client={subject.client} target={workspace ? workspaceSettingsTarget('A', 'A') : userSettingsTarget} host={cfg3Host(subject)} />);
  await settingsReady();
  await openResourceModule('skills');
  await screen.findByRole('listitem', { name: 'a' });
  return subject;
}
const toggle = (name: string) => within(screen.getByRole('listitem', { name })).getByRole('switch');
const policy = () => screen.getByRole('button', { name: /Skill access$/ });
async function settled() { await waitFor(() => expect((policy() as HTMLButtonElement).disabled).toBe(false)); }

it('restores dynamic all after exact toggles and enables skills discovered afterwards', async () => {
  const subject = await skills();
  fireEvent.click(toggle('a'));
  await waitFor(() => expect(writes(subject)).toHaveLength(1)); await settled();
  expect(writes(subject)[0].mutation).toMatchObject({ mutation: { unit: 'skills', authored: ['b'] } });
  fireEvent.click(toggle('a'));
  await waitFor(() => expect(writes(subject)).toHaveLength(2)); await settled();
  expect(writes(subject)[1].mutation).toMatchObject({ mutation: { authored: ['b', 'a'] } });
  expect(policy().textContent).toContain('Selected skills');
  await chooseOption('Skill access', 'All skills, including newly discovered');
  await waitFor(() => expect(writes(subject)).toHaveLength(3)); await settled();
  expect(writes(subject)[2].mutation).toMatchObject({ mutation: { authored: 'all' } });
  subject.source.prospective_resources!.definitions.push({ family: 'skill', name: 'c', valid: true,
    location: { scope: 'user', path: '/skills/c/SKILL.md' } });
  fireEvent.click(screen.getByRole('button', { name: 'Refresh' }));
  await screen.findByRole('listitem', { name: 'c' });
  expect(toggle('c').getAttribute('aria-checked')).toBe('true');
  expect(writes(subject)).toHaveLength(3);
});

it('removes a Workspace override and follows later global selections', async () => {
  const subject = await skills(true);
  expect(policy().textContent).toContain('Use global settings');
  fireEvent.click(toggle('a'));
  await waitFor(() => expect(writes(subject)).toHaveLength(1)); await settled();
  expect(subject.source.workspace!.authored!.agent!.skills).toEqual(['b']);
  await chooseOption('Skill access', 'Use global settings');
  await waitFor(() => expect(writes(subject)).toHaveLength(2)); await settled();
  expect(writes(subject)[1]).toMatchObject({ target: { kind: 'workspace' }, mutation: { mutation: { unit: 'skills', authored: null } } });
  expect(policy().textContent).toContain('Use global settings');
  fireEvent.click(screen.getByRole('tab', { name: 'Global' }));
  await screen.findByRole('listitem', { name: 'a' }); await settled();
  await chooseOption('Skill access', 'None');
  await waitFor(() => expect(writes(subject)).toHaveLength(3)); await settled();
  fireEvent.click(screen.getByRole('tab', { name: 'Workspace' }));
  await screen.findByRole('listitem', { name: 'a' }); await settled();
  expect(policy().textContent).toContain('Use global settings');
  expect(toggle('a').getAttribute('aria-checked')).toBe('false');
  expect(toggle('b').getAttribute('aria-checked')).toBe('false');
  expect(subject.source.workspace!.authored!.agent).not.toHaveProperty('skills');
});

it('keeps explicit none distinct from inherited none and can freeze an exact current selection', async () => {
  const subject = await skills(true);
  await chooseOption('Skill access', 'Selected skills');
  await waitFor(() => expect(writes(subject)).toHaveLength(1)); await settled();
  expect(writes(subject)[0].mutation).toMatchObject({ mutation: { authored: ['a', 'b'] } });
  await chooseOption('Skill access', 'None');
  await waitFor(() => expect(writes(subject)).toHaveLength(2)); await settled();
  expect(writes(subject)[1].mutation).toMatchObject({ mutation: { authored: [] } });
  expect(policy().textContent).toContain('None');
  subject.source.user.authored!.agent!.skills = [];
  await chooseOption('Skill access', 'Use global settings');
  await waitFor(() => expect(writes(subject)).toHaveLength(3)); await settled();
  expect(writes(subject)[2].mutation).toMatchObject({ mutation: { authored: null } });
  expect(policy().textContent).toContain('Use global settings');
  expect(toggle('a').getAttribute('aria-checked')).toBe('false');
  await chooseOption('Skill access', 'None');
  await waitFor(() => expect(writes(subject)).toHaveLength(4)); await settled();
  expect(writes(subject)[3].mutation).toMatchObject({ mutation: { authored: [] } });
  expect(policy().textContent).toContain('None');
});
