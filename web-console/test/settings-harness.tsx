import { expect, vi } from 'vitest';
import { act, fireEvent, render, screen, waitFor, within } from '@testing-library/react';
import { createActor } from 'xstate';
import { useLayoutEffect, useRef, useState, type ReactNode } from 'react';
import type { AppServerClient } from '../src/client/app-server';
import { ConnectionController } from '../src/connection/controller';
import type { SourceMutation, SourceSettings } from '../../protocol/app-server/v77';
import { mutationOutcome, settingsTargetMachine, type SettingsTargetContext } from '../src/app/settings/machines/settings-target';
import type { ConfigurationPort, WriteOutcome } from '../src/app/settings/machines/port';
import { SettingsActorContext } from '../src/app/settings/machines/react';
import { SourceContext } from '../src/app/settings/source-context';
import { settingsTargetKey, userSettingsTarget, workspaceSettingsTarget, type SettingsTarget } from '../src/app/settings/projection';
import { settingsNavigationMachine, type SettingsNavigationActor, type SettingsPage } from '../src/app/settings/machines/navigation';
import { Settings, type SettingsProps } from '../src/app/settings/Settings';
import {ResourceSurface} from '../src/app/resources/ResourceSurface';
import type {ResourceModule} from '../src/app/resources/types';
import {WorkspaceAuthority} from '../src/workspaces/authority';
import {WorkspaceAssociations} from '../src/workspaces/associations';
import { cfg3Source } from './cfg3-data';
import { configurationSystem, type SettingsTargetActor } from '../src/app/settings/machines/system';

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
  const advanced = screen.queryByRole('dialog', { name: /^Model / });
  if (advanced) fireEvent.click(within(advanced).getByRole('button', { name: 'Advanced configuration' }));
  await waitFor(() => expect(document.querySelector('[data-json-editor-loading]')).toBeNull());
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
let resourceNavigation: ((module:ResourceModule|undefined)=>void)|undefined;
let resourceModule:ResourceModule|undefined;
let contractAuthority: (() => SettingsTargetActor) | undefined;

export function SettingsSurface({ target, initialPage, navigation, connection, ...props }: Omit<SettingsProps, 'navigation' | 'connection'> & {
  client: AppServerClient; target: SettingsTarget; initialPage?: SettingsPage;
  navigation?: SettingsNavigationActor; connection?: ConnectionController;
}) {
  const [actor] = useState(() => navigation ?? openSettingsNavigation(target, initialPage));
  const [controller] = useState(() => connection ?? new ConnectionController(props.client));
  const [module,setModule]=useState<ResourceModule>();
  const [workspaceAuthority]=useState(()=>new WorkspaceAuthority(props.host!));
  const [associations]=useState(()=>new WorkspaceAssociations(props.client,workspaceAuthority));
  useLayoutEffect(()=>{resourceNavigation=setModule;resourceModule=module;return()=>{resourceNavigation=undefined;resourceModule=undefined;};},[module]);
  useLayoutEffect(() => {
    const authority = () => configurationSystem(props.client).settingsTarget(target, () => props.host);
    contractAuthority = authority;
    return () => { if (contractAuthority === authority) contractAuthority = undefined; };
  }, [props.client, props.host, target]);
  const opened = useRef(settingsTargetKey(target));
  useLayoutEffect(() => {
    if (opened.current === settingsTargetKey(target)) return;
    opened.current = settingsTargetKey(target);
    actor.send({ type: 'OPEN', target });
  }, [actor, target]);
  return <><div hidden={!!module}><Settings {...props} visible={props.visible!==false&&!module} navigation={actor} connection={controller}/></div>
    {(['subagents','mcp','skills'] as const).map(kind=><ResourceSurface key={kind} module={kind} visible={module===kind} client={props.client} host={props.host!} authority={workspaceAuthority} associations={associations} initialWorkspace={target.kind==='workspace'?target.id:undefined}/>)}</>;
}

/** Move to one primary product page through its actual tab, so a page change
 * in a test is the same interaction a user performs. */
export async function openSettingsPage(name: string) {
  await act(async()=>resourceNavigation?.(undefined));
  fireEvent.click(screen.getByRole('tab', { name }));
  await screen.findByRole('tab', { name, selected: true });
  if (name === 'Models') {
    for (const title of ['Default model for new Sessions']) {
      const trigger = screen.queryByRole('button', { name: title, expanded: false });
      if (trigger) fireEvent.click(trigger);
    }
  }
}

export function showSettings() { act(()=>resourceNavigation?.(undefined)); }
export async function newSubagent(name: string) {
 fireEvent.click(screen.getByRole('button', { name: 'New subagent' }));
 const form = await screen.findByRole('form', { name: 'New subagent' });
 fireEvent.change(within(form).getByLabelText('Name'), { target: { value: name } });
}

/** Navigate to an independent module, using its real resource surface. */
export async function openResourceModule(module:ResourceModule) {
 await act(async()=>resourceNavigation?.(module));
 await waitFor(()=>expect(document.querySelector(`[data-resource-module="${module}"][data-lifecycle="ready"]`)).toBeTruthy());
}

/** Open a detail from a resource list through its actual row, so list/detail
 * navigation in a test is the same interaction a user performs. */
export async function openResourceRow(name: string) {
  const card = screen.queryByRole('listitem', { name });
  fireEvent.click(card ? within(card).getAllByRole('button')[0] : await screen.findByRole('row', { name }));
  const modelDialog = screen.queryByRole('dialog', { name: /^Model / });
  if (modelDialog) fireEvent.click(within(modelDialog).getByRole('button', { name: 'Advanced configuration', expanded: false }));
  if (resourceModule==='mcp') openMcpContractEditor(name);
}

/** Open the independent permission units alongside the server definition. */
export function openMcpContractEditor(name?: string) {
  void name;
  const permissions=screen.queryByRole('button',{name:'Permissions',expanded:false});
  if(permissions)fireEvent.click(permissions);
}

/** Wait until the open Settings surface holds a current authoritative
 * observation. Readiness is asserted from the lifecycle itself; source
 * paths and revisions are not product copy. */
export async function settingsReady() {
  await waitFor(() => expect(document.querySelector('[data-lifecycle=ready]')).toBeTruthy());
}

/** The primary page currently displayed. */
export function currentSettingsPage(): string {
  return screen.getByRole('tab', { selected: true }).textContent ?? '';
}

/** Coordination tests inspect the actual authority rather than requiring
 * internal revisions and projections in the product UI. */
export function settingsContext() {
  if (!contractAuthority) throw new Error('No Settings surface mounted');
  return contractAuthority().getSnapshot().context;
}
export function settingsOutcome() {
  if (!contractAuthority) throw new Error('No Settings surface mounted');
  return mutationOutcome(contractAuthority().getSnapshot());
}
export async function waitForSettingsSave() {
  await waitFor(() => expect(settingsOutcome().kind).toBe('saved'));
}
export function observedRevision(expected?: string) {
  const { observation, target } = settingsContext();
  const revision = target.kind === 'user' ? observation?.user.revision : observation?.workspace?.revision;
  return revision !== undefined && (expected === undefined || revision === expected) ? revision : null;
}
export async function waitForSettingsRevision(expected?: string) {
  await waitFor(() => expect(observedRevision(expected)).toBeTruthy());
}

/** Pick a value from a React Aria Select.
 *
 * The trigger's accessible name is its current value followed by its label, and
 * the listbox opens in a portal outside the form, so the trigger is found in
 * `scope` and the option globally. */
export async function providerAction(name: string, scope: { getByRole: typeof screen.getByRole } = screen) {
  fireEvent.click(scope.getByRole('button', { name: 'Provider actions' }));
  fireEvent.click(await screen.findByRole('menuitem', { name }));
}

export async function chooseOption(label: string, option: string, scope: { getByRole: typeof screen.getByRole } = screen) {
  fireEvent.click(scope.getByRole('button', { name: (accessible: string) => accessible.endsWith(label) }));
  fireEvent.click(await screen.findByRole('option', { name: option }));
}

/** Complete a removal through its React Aria confirmation: the
 * trigger opens the dialog, and only the dialog's own button submits. */
export async function confirmAction(label: string, scope: { getByRole: typeof screen.getByRole } = screen) {
  fireEvent.click(scope.getByRole('button', { name: label }));
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
