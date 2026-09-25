import { expect, vi } from 'vitest';
import { fireEvent, render, screen, waitFor, within } from '@testing-library/react';
import { createActor } from 'xstate';
import { useLayoutEffect, useRef, useState, type ReactNode } from 'react';
import type { AppServerClient } from '../src/client/app-server';
import { ConnectionController } from '../src/connection/controller';
import type { SourceMutation, SourceSettings } from '../../protocol/app-server/v22';
import { settingsTargetMachine, type SettingsTargetContext } from '../src/app/settings/machines/settings-target';
import type { ConfigurationPort, WriteOutcome } from '../src/app/settings/machines/port';
import { SettingsActorContext } from '../src/app/settings/machines/react';
import { SourceContext } from '../src/app/settings/source-context';
import { settingsTargetKey, userSettingsTarget, workspaceSettingsTarget, type SettingsTarget } from '../src/app/settings/projection';
import { settingsNavigationMachine, type SettingsNavigationActor, type SettingsPage } from '../src/app/settings/machines/navigation';
import { Settings, type SettingsProps } from '../src/app/settings/Settings';
import { cfg3Source } from './cfg3-data';

/** One acknowledgement that changes no revision, so a sequence of saves in one
 * editor keeps submitting against the same exact CAS base the editor pinned. */
export function sameRevision(source: SourceSettings, revision: string): SourceSettings {
  const next = structuredClone(source);
  next.user.revision = revision;
  if (next.workspace) next.workspace.revision = revision;
  next.user_mcp.revision = revision;
  if (next.workspace_mcp) next.workspace_mcp.revision = revision;
  next.agents = next.agents.map(entry => ({ ...entry, source: { ...entry.source, revision } }));
  next.absent_resource_revision = revision;
  return next;
}

/** A real Settings authority actor over an explicit in-test native port.
 *
 * Editors submit intent to this actor exactly as they do inside `Settings`, so
 * an editor test exercises the real transaction machine, the real CAS base and
 * the real submission path rather than a stand-in callback.
 *
 * Every authoritative read answers with exactly the projection under test, so
 * the editor's facts never drift and no timer or sleep is involved anywhere. */
export function settingsAuthority(source: SourceSettings = cfg3Source(), options: {
  write?: (expected: string, mutation: SourceMutation) => Promise<WriteOutcome>;
  /** Answers every read after the first; by default the projection under test. */
  reread?: () => Promise<SourceSettings>;
} = {}) {
  let reads = 0;
  const writes = vi.fn<(mutation: SourceMutation, expected: string) => void>();
  const port: ConfigurationPort = {
    ownsReread: false,
    publication: () => undefined,
    read: () => reads++ && options.reread ? options.reread() : Promise.resolve(structuredClone(source)),
    // By default native refuses the write, so an editor test observes exactly
    // what was submitted — including a sequence of submissions against the same
    // pinned CAS base — without a confirmed commit retiring the draft it is
    // asserting on. A test that needs a definitive commit supplies `write`.
    write: (expected, mutation) => {
      writes(mutation, expected);
      return options.write ? options.write(expected, mutation) : Promise.reject(new Error('native refused this fixture write'));
    },
    reconcile: async () => {},
  };
  const actor = createActor(settingsTargetMachine, {
    input: {
      target: source.target.kind === 'user' ? userSettingsTarget : workspaceSettingsTarget('A', 'A'),
      port, connection: 'connected', generation: 1, publication: undefined,
    },
  });
  actor.start();
  actor.send({ type: 'ATTACH' });
  return { actor, writes, context: () => actor.getSnapshot().context as SettingsTargetContext };
}

export function InSettings({ actor, source, children }: { actor: ReturnType<typeof settingsAuthority>['actor']; source?: SourceSettings; children: ReactNode }) {
  return <SettingsActorContext value={actor}><SourceContext value={source}>{children}</SourceContext></SettingsActorContext>;
}

/** Render one editor under a live Settings authority and wait until its single
 * authoritative read has been adopted, which is what makes authoring legal. */
export async function renderEditor(node: ReactNode, options: {
  source?: SourceSettings; context?: SourceSettings;
  write?: (expected: string, mutation: SourceMutation) => Promise<WriteOutcome>;
  reread?: () => Promise<SourceSettings>;
} = {}) {
  const authority = settingsAuthority(options.source ?? options.context ?? cfg3Source(), { write: options.write, reread: options.reread });
  const view = render(<InSettings actor={authority.actor} source={options.context}>{node}</InSettings>);
  await waitFor(() => expect(authority.context().observation).toBeTruthy());
  const rerender = (next: ReactNode) => view.rerender(<InSettings actor={authority.actor} source={options.context}>{next}</InSettings>);
  return { ...authority, view, rerender };
}

/** A started Settings navigation machine, opened on one exact target exactly
 * as the product shell's `OPEN` decision opens it. The owner lookup is never
 * exercised by a Settings surface test, so it answers `retired`. */
export function openSettingsNavigation(target: SettingsTarget, page?: SettingsPage): SettingsNavigationActor {
  const navigation = createActor(settingsNavigationMachine, { input: { lookup: async () => ({ kind: 'retired' }) } });
  navigation.start();
  navigation.send({ type: 'OPEN', target });
  if (page) navigation.send({ type: 'SELECT', page });
  return navigation;
}

/** `Settings` rendered on its own, outside the product shell, but with exactly
 * the product composition: the real Settings navigation machine owns the
 * target, the displayed page and the detail focused inside it, and a real
 * `ConnectionController` is supplied for every target, as `App` supplies one.
 * No test surface can therefore reach a navigation state the product could
 * not, nor hide one the product could.
 *
 * Rendering it again with another `target` is the product's decision to open
 * that owner's Settings, so it is delivered to the machine as exactly that
 * `OPEN` event. */
export function SettingsSurface({ target, initialPage, navigation, connection, ...props }: Omit<SettingsProps, 'navigation' | 'connection'> & {
  client: AppServerClient; target: SettingsTarget; initialPage?: SettingsPage;
  navigation?: SettingsNavigationActor; connection?: ConnectionController;
}) {
  const [actor] = useState(() => navigation ?? openSettingsNavigation(target, initialPage));
  const [controller] = useState(() => connection ?? new ConnectionController(props.client));
  const opened = useRef(settingsTargetKey(target));
  useLayoutEffect(() => {
    if (opened.current === settingsTargetKey(target)) return;
    opened.current = settingsTargetKey(target);
    actor.send({ type: 'OPEN', target });
  }, [actor, target]);
  return <Settings {...props} navigation={actor} connection={controller} />;
}

/** Move to one primary product page through its actual tab, so a page change
 * in a test is the same interaction a user performs. */
export async function openSettingsPage(name: string) {
  fireEvent.click(screen.getByRole('tab', { name }));
  await screen.findByRole('tab', { name, selected: true });
  if (name === 'Models') {
    for (const title of ['New Provider', 'Default model for new Sessions']) {
      const trigger = screen.queryByRole('button', { name: title, expanded: false });
      if (trigger) fireEvent.click(trigger);
    }
  }
}

/** Open a detail from a resource list through its actual row, so list/detail
 * navigation in a test is the same interaction a user performs. */
export async function openResourceRow(name: string) {
  fireEvent.click(await screen.findByRole('row', { name }));
}

/** Wait until the open Settings surface holds a current authoritative
 * observation. Source paths and revisions are diagnostics and live on
 * Advanced, so readiness is asserted from the lifecycle itself. */
export async function settingsReady() {
  await screen.findByText('Authoritative source observed');
}

/** The primary page currently displayed. */
export function currentSettingsPage(): string {
  return screen.getByRole('tab', { selected: true }).textContent ?? '';
}

/** Native source paths, revisions and raw projections are diagnostics: they
 * live on Advanced, not on the ordinary product pages. These read them there
 * and return to the page the test was on, so asserting a revision never
 * changes what the test is actually exercising. */
export async function findOnAdvanced(pattern: RegExp) {
  const previous = currentSettingsPage();
  if (previous !== 'Advanced') fireEvent.click(screen.getByRole('tab', { name: 'Advanced' }));
  const node = await screen.findByText(pattern);
  if (previous !== 'Advanced') fireEvent.click(screen.getByRole('tab', { name: previous }));
  return node;
}
export function queryOnAdvanced(pattern: RegExp) {
  const previous = currentSettingsPage();
  if (previous !== 'Advanced') fireEvent.click(screen.getByRole('tab', { name: 'Advanced' }));
  const node = screen.queryByText(pattern);
  if (previous !== 'Advanced') fireEvent.click(screen.getByRole('tab', { name: previous }));
  return node;
}

/** Pick a value from a React Aria Select.
 *
 * The trigger's accessible name is its current value followed by its label, and
 * the listbox opens in a portal outside the form, so the trigger is found in
 * `scope` and the option globally. */
export async function chooseOption(label: string, option: string, scope: { getByRole: typeof screen.getByRole } = screen) {
  fireEvent.click(scope.getByRole('button', { name: (accessible: string) => accessible.endsWith(label) }));
  fireEvent.click(await screen.findByRole('option', { name: option }));
}

/** Complete a removal through its React Aria confirmation: the
 * trigger opens the dialog, and only the dialog's own button submits. */
export async function confirmAction(label: string) {
  fireEvent.click(screen.getByRole('button', { name: label }));
  fireEvent.click(within(await confirmation(label)).getByRole('button', { name: label }));
}

/** The confirmation layer a removal opens. Restoring inheritance is an
 * ordinary dialog; a real removal is an alert dialog. Both are titled by the
 * question they ask, which is how they are told apart from the Settings
 * dialog underneath. */
export function confirmation(label: string) {
  return screen.findByRole(label.startsWith('Use global default') ? 'dialog' : 'alertdialog', { name: /\?$/ });
}

/** Open a removal confirmation and dismiss it without confirming. */
export async function cancelAction(label: string) {
  fireEvent.click(screen.getByRole('button', { name: label }));
  const dialog = await confirmation(label);
  fireEvent.click(within(dialog).getByRole('button', { name: 'Cancel' }));
  await waitFor(() => expect(dialog.isConnected).toBe(false));
}
