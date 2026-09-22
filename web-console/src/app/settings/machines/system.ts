import { createActor, type Actor } from 'xstate';
import type { AppServerClient } from '../../../client/app-server';
import type { ProductHostWorkspaces } from '../../../workspaces/host';
import { settingsTargetKey, type SettingsTarget } from '../projection';
import { createConfigurationPort } from './port';
import { createSessionConfigurationPort, sessionConfigurationMachine } from './session-configuration';
import { settingsTargetMachine } from './settings-target';

export type SettingsTargetActor = Actor<typeof settingsTargetMachine>;
export type SessionConfigurationActor = Actor<typeof sessionConfigurationMachine>;

/** Test-only view of one transaction owner's retained state, for the regression
 * that proves a confirmed commit leaves no secret-bearing authored payload
 * reachable. Production code never reads it. */
export interface TransactionOwner { retainedState(): readonly unknown[] }

function transactionOwner(actor: SettingsTargetActor): TransactionOwner {
  return {
    retainedState() {
      const { units, submission } = actor.getSnapshot().context;
      return [...Object.values(units).map(unit => unit.getSnapshot().context), ...(submission ? [submission] : [])];
    },
  };
}

/** The configuration actor system of one App Server client.
 *
 * It exists because presentation lifetime, transaction lifetime and App Server
 * authority lifetime are three different things:
 *
 * - a Settings dialog opens and closes many times;
 * - an editing transaction and a definitive acknowledgement must outlive the
 *   dialog that started them;
 * - an authority replacement must retire everything the old authority owned so
 *   that no old state can leak into the replacement.
 *
 * Ownership is therefore explicit and keyed, never ambient: every actor is
 * addressed by (endpoint, authority revision, subject). */
export class ConfigurationSystem {
  /** Keyed by `<endpoint>|<authority>|<target>`. Kept for the whole authority
   * lifetime, so drafts, pinned CAS bases and in-flight settlement survive the
   * dialog closing, the section changing and the target being switched. */
  private readonly targets = new Map<string, SettingsTargetActor>();
  /** Keyed by `<endpoint>|<authority>|<session>`, released when no presentation
   * holds them and no adoption is in flight. */
  private readonly sessions = new Map<string, { actor: SessionConfigurationActor; holders: number }>();
  /** Lifetimes a replaced authority left behind. A retired lifetime is inert:
   * it is detached, so it reads nothing and converges nothing. It is kept — not
   * stopped — precisely because a definitive acknowledgement already in flight
   * must still settle the exact transaction that submitted it, under its own
   * old authority and never under the replacement. */
  private retired: SettingsTargetActor[] = [];
  private readonly owners = new WeakMap<SettingsTargetActor, TransactionOwner>();
  private lifetime = '';

  constructor(private readonly client: AppServerClient) {}

  private reconcileLifetime(lifetime: string) {
    if (this.lifetime === lifetime) return;
    this.lifetime = lifetime;
    // A retired lifetime that has no native mutation left in flight owns
    // nothing at all, so it is dropped at the next authority change.
    this.retired = this.retired.filter(actor => actor.getSnapshot().matches({ mutation: 'submitting' }));
    for (const actor of this.targets.values()) {
      actor.send({ type: 'DETACH' });
      this.retired.push(actor);
    }
    this.targets.clear();
    // A Session actor owns no durable browser intent, so a replaced authority
    // simply ends it.
    for (const entry of this.sessions.values()) entry.actor.stop();
    this.sessions.clear();
  }

  private currentLifetime() {
    const transport = this.client.getSnapshot();
    return `${transport.endpoint ?? ''}|${transport.authorityRevision ?? 0}`;
  }

  /** The Settings authority actor of one exact target, created on demand. */
  settingsTarget(target: SettingsTarget, host: () => ProductHostWorkspaces | undefined): SettingsTargetActor {
    const transport = this.client.getSnapshot();
    const lifetime = this.currentLifetime();
    this.reconcileLifetime(lifetime);
    const key = `${lifetime}|${settingsTargetKey(target)}`;
    const existing = this.targets.get(key);
    if (existing) return existing;
    const actor = createActor(settingsTargetMachine, {
      input: {
        target,
        port: createConfigurationPort({ client: this.client, endpoint: transport.endpoint ?? '', workspaceId: target.kind === 'workspace' ? target.id : undefined, host }),
        connection: transport.connection,
        generation: transport.generation,
        publications: transport.configuration,
      },
    });
    this.targets.set(key, actor);
    this.owners.set(actor, transactionOwner(actor));
    actor.start();
    return actor;
  }

  /** The Session configuration actor of one Session, created on demand. */
  sessionConfiguration(sessionId: string): SessionConfigurationActor {
    const transport = this.client.getSnapshot();
    const lifetime = this.currentLifetime();
    this.reconcileLifetime(lifetime);
    const key = `${lifetime}|session:${sessionId}`;
    const existing = this.sessions.get(key);
    if (existing) return existing.actor;
    const actor = createActor(sessionConfigurationMachine, {
      input: { port: createSessionConfigurationPort(this.client, sessionId), connection: transport.connection, generation: transport.generation },
    });
    this.sessions.set(key, { actor, holders: 0 });
    actor.start();
    return actor;
  }

  /** Session actors are reference counted, so a Session the user merely looked
   * at does not accumulate for the authority's whole lifetime. Settings target
   * actors deliberately are not: their editing transactions are exactly the
   * thing that has to outlive every presentation. */
  retainSession(actor: SessionConfigurationActor) {
    for (const entry of this.sessions.values()) if (entry.actor === actor) entry.holders += 1;
  }
  releaseSession(actor: SessionConfigurationActor) {
    for (const [key, entry] of this.sessions) {
      if (entry.actor !== actor) continue;
      entry.holders -= 1;
      if (entry.holders > 0) continue;
      // An adoption already in flight settles under its own Session lifetime.
      if (entry.actor.getSnapshot().matches({ adoption: 'submitting' })) continue;
      entry.actor.stop();
      this.sessions.delete(key);
    }
  }

  /** Test-only inspection of every live transaction owner, oldest lifetime
   * first. Production code never reads it. */
  transactionOwners(): readonly TransactionOwner[] {
    return [...this.retired, ...this.targets.values()].map(actor => {
      let owner = this.owners.get(actor);
      if (!owner) { owner = transactionOwner(actor); this.owners.set(actor, owner); }
      return owner;
    });
  }
}

const systems = new WeakMap<AppServerClient, ConfigurationSystem>();

/** The one configuration actor system of a client. Keyed by the client object
 * itself, so a replaced client — and every test — starts from nothing. */
export function configurationSystem(client: AppServerClient): ConfigurationSystem {
  let system = systems.get(client);
  if (!system) { system = new ConfigurationSystem(client); systems.set(client, system); }
  return system;
}
