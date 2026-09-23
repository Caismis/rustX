import { createActor, type Actor, type Subscription } from 'xstate';
import type { AppServerClient, ClientView } from '../../../client/app-server';
import type { ProductHostWorkspaces } from '../../../workspaces/host';
import { settingsTargetKey, type SettingsTarget } from '../projection';
import { createConfigurationPort } from './port';
import { adoptionInFlight, createSessionConfigurationPort, sessionConfigurationMachine } from './session-configuration';
import { mutationInFlight, settingsTargetMachine } from './settings-target';

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

interface SessionEntry {
  readonly actor: SessionConfigurationActor;
  holders: number;
  /** Present exactly while no presentation holds the actor and an adoption
   * transaction is still in flight: the subscription that releases the actor at
   * that transaction's terminal point. */
  settlement?: Subscription;
}

/** One App Server authority lifetime: (endpoint, authority revision). */
function authorityLifetime(transport: ClientView): string {
  return `${transport.endpoint ?? ''}|${transport.authorityRevision ?? 0}`;
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
 * addressed by (endpoint, authority revision, subject). The system owns every
 * actor's lifetime: it observes the client's authority and retires a replaced
 * authority's lifetime at that replacement itself, and it releases an actor
 * retained only for a transaction in flight by observing that transaction's
 * terminal state — never by polling, and never by waiting for some later,
 * unrelated lifetime change or actor lookup. The actors own their transaction
 * state and know nothing of the system. */
export class ConfigurationSystem {
  /** The current authority lifetime's target actors, keyed by target. Kept for
   * that whole lifetime, so drafts, pinned CAS bases and in-flight settlement
   * survive the dialog closing, the section changing and the target being
   * switched. */
  private readonly targets = new Map<string, SettingsTargetActor>();
  /** The current authority lifetime's Session actors, keyed by Session and
   * released when no presentation holds them and no adoption is in flight. */
  private readonly sessions = new Map<string, SessionEntry>();
  /** Target actors a replaced authority left behind with a native mutation in
   * flight, each with the subscription that releases it.
   *
   * A retired lifetime is inert: it is detached, so it reads nothing and
   * converges nothing. It is kept — not stopped — only because a mutation that
   * already crossed the native submission boundary must still settle the exact
   * transaction that submitted it, under its own old authority and never under
   * the replacement. The moment that mutation settles, the actor is stopped and
   * dropped. Nothing else of a replaced authority survives it: an unsaved draft
   * belongs to the authority it was authored against and is never migrated. */
  private readonly retired = new Map<SettingsTargetActor, Subscription>();
  private readonly owners = new WeakMap<SettingsTargetActor, TransactionOwner>();
  /** The App Server authority lifetime every live actor belongs to. */
  private lifetime: string;

  /** The system observes the client's authority itself, so an authority
   * replacement retires the old lifetime at the replacement's own publication:
   * nothing of the old authority waits for some later actor lookup to discover
   * that its authority has ended, and no old actor survives into the
   * replacement to start new work through the live client. The system lives
   * exactly as long as its client (see `configurationSystem`), so the
   * subscription is never released. */
  constructor(private readonly client: AppServerClient) {
    this.lifetime = authorityLifetime(client.getSnapshot());
    client.subscribe(() => this.observeAuthority());
  }

  /** A connection generation replacement inside one authority is not an
   * authority replacement: it is the actors' own generation fencing to handle,
   * and it retires nothing here. */
  private observeAuthority() {
    const lifetime = authorityLifetime(this.client.getSnapshot());
    if (this.lifetime === lifetime) return;
    this.lifetime = lifetime;
    for (const actor of this.targets.values()) this.retireTarget(actor);
    this.targets.clear();
    // A Session actor owns no durable browser intent, so a replaced authority
    // ends it at once — an adoption still in flight included, whose late
    // response then has no completion path at all.
    for (const [key, entry] of this.sessions) this.dropSession(key, entry);
  }

  /** Retire one target actor of a replaced authority.
   *
   * With no native mutation in flight it owns nothing that may outlive its
   * authority, so it is stopped and dropped now, drafts included. With one in
   * flight it is detached and retained until that mutation leaves flight — its
   * settlement point, at which the transaction that submitted it has recorded
   * the outcome — and is then stopped and dropped at once. */
  private retireTarget(actor: SettingsTargetActor) {
    if (!mutationInFlight(actor.getSnapshot())) {
      actor.stop();
      return;
    }
    actor.send({ type: 'DETACH' });
    const release = () => {
      this.retired.get(actor)?.unsubscribe();
      this.retired.delete(actor);
      actor.stop();
    };
    this.retired.set(actor, actor.subscribe({
      next: snapshot => { if (!mutationInFlight(snapshot)) release(); },
      error: release,
    }));
  }

  /** The Settings authority actor of one exact target, created on demand. */
  settingsTarget(target: SettingsTarget, host: () => ProductHostWorkspaces | undefined): SettingsTargetActor {
    const transport = this.client.getSnapshot();
    const key = settingsTargetKey(target);
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
    const key = sessionId;
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
    for (const entry of this.sessions.values()) {
      if (entry.actor !== actor) continue;
      entry.holders += 1;
      // A holder owns the actor again; nothing waits on settlement to release it.
      entry.settlement?.unsubscribe();
      entry.settlement = undefined;
    }
  }
  /** The last holder leaving releases the actor — at once, or, when an adoption
   * transaction is still in flight, exactly at that transaction's terminal
   * point, so the adoption settles under its own Session lifetime. */
  releaseSession(actor: SessionConfigurationActor) {
    for (const [key, entry] of this.sessions) {
      if (entry.actor !== actor) continue;
      entry.holders -= 1;
      if (entry.holders > 0) continue;
      if (!adoptionInFlight(entry.actor.getSnapshot())) {
        this.dropSession(key, entry);
        continue;
      }
      entry.settlement = entry.actor.subscribe({
        next: snapshot => { if (!adoptionInFlight(snapshot)) this.dropSession(key, entry); },
        error: () => this.dropSession(key, entry),
      });
    }
  }
  private dropSession(key: string, entry: SessionEntry) {
    entry.settlement?.unsubscribe();
    entry.settlement = undefined;
    entry.actor.stop();
    this.sessions.delete(key);
  }

  /** Test-only inspection of every live transaction owner, oldest lifetime
   * first. Production code never reads it. */
  transactionOwners(): readonly TransactionOwner[] {
    return [...this.retired.keys(), ...this.targets.values()].map(actor => {
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
