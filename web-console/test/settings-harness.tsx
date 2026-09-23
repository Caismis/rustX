import { expect, vi } from 'vitest';
import { render, waitFor } from '@testing-library/react';
import { createActor } from 'xstate';
import { useState, type ReactNode } from 'react';
import type { SourceMutation, SourceSettings } from '../../protocol/app-server/v18';
import { settingsTargetMachine, type SettingsTargetContext } from '../src/app/settings/machines/settings-target';
import type { ConfigurationPort, WriteOutcome } from '../src/app/settings/machines/port';
import { SettingsActorContext } from '../src/app/settings/machines/react';
import { SourceContext } from '../src/app/settings/source-context';
import { userSettingsTarget, workspaceSettingsTarget } from '../src/app/settings/projection';
import { Settings, type SettingsProps } from '../src/app/settings/Settings';
import type { SettingsSection } from '../src/app/settings/machines/navigation';
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

/** `Settings` rendered on its own, outside the product shell. In the product
 * the Settings navigation machine owns the displayed section; here the test
 * surface owns it, exactly as a controlled parent would. */
export function SettingsSurface(props: Omit<SettingsProps, 'section' | 'onSelect'>) {
  const [section, setSection] = useState<SettingsSection>('overview');
  return <Settings {...props} section={section} onSelect={setSection} />;
}
