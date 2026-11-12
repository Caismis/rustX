import { useState, type ReactNode } from 'react';
import { useSelector } from '@xstate/react';
import { act, cleanup, fireEvent, render, screen, waitFor, within } from '@testing-library/react';
import { afterEach, expect, it } from 'vitest';
import type { SourceMutation, SourceSettings } from '../../protocol/app-server/v79';
import { ModelsPage } from '../src/app/settings/models/ModelsPage';
import { AgentEditor } from '../src/app/resources/subagents/AgentEditor';
import { McpEditor } from '../src/app/resources/mcp/McpEditor';
import { SettingsFeedback } from '../src/app/settings/SettingsFeedback';
import { cfg3Client } from './cfg3-fixture';
import { cfg3Source } from './cfg3-data';
import { InSettings, settingsAuthority } from './settings-harness';
import { userSettingsTarget } from '../src/app/settings/projection';

const actors: ReturnType<typeof settingsAuthority>['actor'][] = [];
afterEach(() => { cleanup(); actors.splice(0).forEach(actor => actor.stop()); });
function deferred<T>() {
  let resolve!: (value: T) => void;
  const promise = new Promise<T>(done => { resolve = done; });
  return { promise, resolve };
}
type Editor = 'model' | 'new-agent' | 'edit-agent' | 'mcp';

function Surface({ actor, kind }: { actor: ReturnType<typeof settingsAuthority>['actor']; kind: Editor }) {
  const source = useSelector(actor, snapshot => snapshot.context.observation);
  const [client] = useState(() => cfg3Client().client);
  const [closed, setClosed] = useState(false);
  const close = () => setClosed(true);
  if (!source || closed) return <output>{closed ? 'Editor closed' : 'Loading'}</output>;
  let editor: ReactNode;
  if (kind === 'model') editor = <ModelsPage client={client} source={source} scope="user" revision={source.user.revision}
    target={userSettingsTarget} models={[]} profiles={{}} onFocus={() => {}} />;
  else if (kind === 'mcp') editor = <McpEditor source={source} scope="user" focus={{ kind: 'mcp' }}
    scopeControl={null} close={close} onEditorDraftChange={() => {}} />;
  else editor = <AgentEditor source={source} target={userSettingsTarget} models={[]} profiles={{}}
    name={kind === 'edit-agent' ? 'helper' : undefined} discover={false} onClose={close} />;
  return <InSettings actor={actor} source={source}><SettingsFeedback>{editor}</SettingsFeedback></InSettings>;
}

function McpCreations({ actor }: { actor: ReturnType<typeof settingsAuthority>['actor'] }) {
  const source = useSelector(actor, snapshot => snapshot.context.observation);
  const [open, setOpen] = useState(false);
  if (!source) return null;
  return <InSettings actor={actor} source={source}><SettingsFeedback>
    {open ? <McpEditor source={source} scope="user" focus={{ kind: 'mcp' }} scopeControl={null}
      close={() => setOpen(false)} onEditorDraftChange={() => {}} />
      : <button onClick={() => setOpen(true)}>New MCP</button>}
  </SettingsFeedback></InSettings>;
}

it.each(['earlier editor', 'native source'] as const)('MCP preserves a rejected creation renamed to a server from %s', async origin => {
  const source = cfg3Source();
  const server = { definition: { type: 'stdio' as const, command: 'command-a' }, retained_env: [], retained_headers: [] };
  if (origin === 'native source') source.user_mcp.authored!['server-a'] = server;
  let revision = 1;
  const authority = settingsAuthority(source, { write: async (_expected, mutation) => {
    if (mutation.kind !== 'mcp' || !mutation.authored) throw new Error('Expected MCP authoring');
    if (mutation.id === 'server-b') throw new Error('Native rejected B');
    source.user_mcp.authored![mutation.id] = { ...mutation.authored,
      retained_env: mutation.authored.retained_env ?? [], retained_headers: mutation.authored.retained_headers ?? [] };
    source.user_mcp.revision = `mcp-${++revision}`;
    return { acknowledgement: { revision: source.user_mcp.revision } };
  } });
  actors.push(authority.actor);
  render(<McpCreations actor={authority.actor} />);
  if (origin === 'earlier editor') {
    fireEvent.click(await screen.findByRole('button', { name: 'New MCP' }));
    fireEvent.change(screen.getByLabelText('Name'), { target: { value: 'server-a' } });
    fireEvent.change(screen.getByLabelText('Command'), { target: { value: 'command-a' } });
    fireEvent.click(screen.getByRole('button', { name: 'Save' }));
    await screen.findByRole('button', { name: 'New MCP' });
    expect(Object.values(authority.context().completions)).toHaveLength(1);
  }
  const successful = authority.writes.mock.calls.length;
  fireEvent.click(await screen.findByRole('button', { name: 'New MCP' }));
  fireEvent.change(screen.getByLabelText('Name'), { target: { value: 'server-b' } });
  fireEvent.change(screen.getByLabelText('Command'), { target: { value: 'unsaved-command-b' } });
  fireEvent.click(screen.getByRole('button', { name: 'Save' }));
  await screen.findByText(/Native rejected B/);
  await waitFor(() => expect((screen.getByLabelText('Name') as HTMLInputElement).disabled).toBe(false));
  fireEvent.change(screen.getByLabelText('Name'), { target: { value: 'server-a' } });
  expect(screen.getByRole('form')).toBeTruthy();
  expect((screen.getByLabelText('Command') as HTMLInputElement).value).toBe('unsaved-command-b');
  expect((screen.getByRole('button', { name: 'Save' }) as HTMLButtonElement).disabled).toBe(true);
  expect((screen.getByLabelText('Name') as HTMLInputElement).disabled).toBe(false);
  expect(authority.writes).toHaveBeenCalledTimes(successful + 1);
  expect(Object.values(authority.context().units)).toHaveLength(0);
  fireEvent.change(screen.getByLabelText('Name'), { target: { value: 'server-c' } });
  await waitFor(() => expect((screen.getByRole('button', { name: 'Save' }) as HTMLButtonElement).disabled).toBe(false));
  fireEvent.click(screen.getByRole('button', { name: 'Save' }));
  await screen.findByRole('button', { name: 'New MCP' });
  expect(authority.writes).toHaveBeenCalledTimes(successful + 2);
  expect(source.user_mcp.authored!['server-c'].definition).toMatchObject({ command: 'unsaved-command-b' });
});

it.each<Editor>(['model', 'new-agent', 'edit-agent', 'mcp'])('%s closes only after the exact committed revision is observed', async kind => {
  await exercise(kind, true);
});
it.each<Editor>(['model', 'new-agent', 'edit-agent', 'mcp'])('%s keeps the first post-commit divergence visible without replaying its write', async kind => {
  await exercise(kind, false);
});
it.each<Editor>(['model', 'new-agent', 'edit-agent', 'mcp'])('%s keeps a newer draft open when the previous commit is observed later', async kind => {
  await exercise(kind, false, true);
});

async function exercise(kind: Editor, exact: boolean, newerDraft = false) {
  const source = cfg3Source();
  if (kind === 'edit-agent') source.agents.push({ name: 'helper', scope: 'user', source: {
    path: '/agents/helper.toml', revision: 'agent-1', authored: { description: 'Before' },
  } });
  const acknowledgement = deferred<void>(), observation = deferred<SourceSettings>();
  const delayedObservation = deferred<SourceSettings>();
  let reads = 0;
  let mutation: SourceMutation | undefined;
  const authority = settingsAuthority(source, {
    write: async (_expected, value) => { mutation = value; await acknowledgement.promise; return { acknowledgement: { revision: 'r2' } }; },
    reread: () => reads++ === 0 ? observation.promise : delayedObservation.promise,
  });
  actors.push(authority.actor);
  render(<Surface actor={authority.actor} kind={kind} />);
  if (kind === 'model') {
    fireEvent.click(await screen.findByRole('button', { name: '+ Add Model' }));
    fireEvent.change(screen.getByLabelText('Model ID'), { target: { value: 'new-model' } });
  } else if (kind === 'mcp') {
    fireEvent.change(await screen.findByLabelText('Name'), { target: { value: 'helper' } });
    fireEvent.change(screen.getByLabelText('Command'), { target: { value: 'helper-server' } });
  } else {
    if (kind === 'new-agent') fireEvent.change(await screen.findByLabelText('Name'), { target: { value: 'helper' } });
    fireEvent.change(await screen.findByLabelText('Description'), { target: { value: 'After' } });
  }
  const form = screen.getByRole('form');
  fireEvent.click(within(form).getByRole('button', { name: /^Save/ }));
  await waitFor(() => expect(authority.writes).toHaveBeenCalledTimes(1));
  await act(async () => acknowledgement.resolve());
  expect(screen.getByRole('form')).toBe(form);
  if (kind === 'model') expect(screen.getByRole('dialog', { name: 'Add Model' })).toBeTruthy();
  const next = structuredClone(source);
  if (exact || newerDraft) {
    if (mutation?.kind === 'config' && mutation.mutation.unit === 'model') {
      next.user.revision = 'r2';
      next.user.authored!.models![mutation.mutation.id] = mutation.mutation.authored!;
    } else if (mutation?.kind === 'agent') {
      next.agents = [{ name: mutation.name, scope: 'user', source: { path: '/agents/helper.toml', revision: 'r2', authored: mutation.authored } }];
    } else if (mutation?.kind === 'mcp') {
      next.user_mcp.revision = 'r2';
      next.user_mcp.authored![mutation.id] = { ...mutation.authored!,
        retained_env: mutation.authored!.retained_env ?? [], retained_headers: mutation.authored!.retained_headers ?? [] };
    } else throw new Error('Expected editor mutation');
  }
  await act(async () => observation.resolve(exact ? next : structuredClone(source)));
  if (exact) await waitFor(() => expect(kind === 'model' ? screen.queryByRole('dialog', { name: 'Add Model' }) : screen.queryByRole('form')).toBeNull());
  else {
    expect(screen.getByRole('form')).toBe(form);
    expect(screen.getByText(/saved setting changed elsewhere/)).toBeTruthy();
    const review = screen.getByRole('button', { name: 'Use latest setting' });
    expect((review as HTMLButtonElement).disabled).toBe(false);
    // Review accepts the observed base; it cannot turn a diverged commit into
    // proof that our write is still present or automatically submit it again.
    fireEvent.click(review);
    expect(screen.getByRole('form')).toBe(form);
    if (kind === 'model') expect(screen.getByRole('dialog', { name: 'Add Model' })).toBeTruthy();
    if (newerDraft) {
      const label = kind === 'model' ? 'Model ID' : kind === 'mcp' ? 'Command' : 'Description';
      const value = kind === 'model' ? 'newer-model' : kind === 'mcp' ? 'newer-server' : 'Newer description';
      if (kind === 'new-agent') fireEvent.change(screen.getByLabelText('Name'), { target: { value: 'helper' } });
      fireEvent.change(screen.getByLabelText(label), { target: { value } });
      const unit = Object.values(authority.context().units)[0];
      expect(unit.getSnapshot().matches({ intent: 'dirty' })).toBe(true);
      act(() => authority.actor.send({ type: 'REFRESH' }));
      await waitFor(() => expect(reads).toBe(2));
      await act(async () => delayedObservation.resolve(next));
      expect(unit.getSnapshot().matches({ mutation: 'settled' })).toBe(true);
      expect(unit.getSnapshot().matches({ intent: 'dirty' })).toBe(true);
      expect(unit.getSnapshot().matches({ lifetime: 'live' })).toBe(true);
      expect(screen.getByRole('form')).toBe(form);
      expect((screen.getByLabelText(label) as HTMLInputElement).value).toBe(value);
      if (kind === 'model') expect(screen.getByRole('dialog', { name: 'Add Model' })).toBeTruthy();
    }
  }
  expect(authority.writes).toHaveBeenCalledTimes(1);
}
