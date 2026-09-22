import { createContext } from 'react';
import type { SourceSettings } from '../../../../protocol/app-server/v18';
import { selectedRevision, type RevisionSelector } from './projection';

/** One submitted mutation inside a unit's editing transaction.
 *
 * The token is the mutation's identity: a late acknowledgement carrying an older
 * token can never retire a newer browser intent. The generation captures the
 * browser intent that existed when the mutation was submitted; if the intent
 * moved on, the acknowledgement may advance the CAS base but must keep the newer
 * draft. `savedFrom` is the exact pre-save base, kept only so a projection that
 * has not caught up yet is not mistaken for an external change.
 *
 * The authored payload is deliberately absent. A submitted mutation may carry a
 * Provider literal credential or an MCP literal environment value or header, and
 * settlement never needs any of them: `selector` is the non-sensitive native
 * descriptor that names which source revision settles this mutation. The store
 * therefore never holds a secret-bearing payload, before or after
 * acknowledgement — the live editing draft is the only place an authored secret
 * exists, and acknowledging its commit clears it. */
export interface SubmittedMutation {
  token: number;
  generation: number;
  savedFrom: string;
  selector: RevisionSelector;
  committed?: string;
}

/** Durable per-unit editor transaction state.
 *
 * The UI's editing intent is not the same fact as "a value override exists": a
 * clean Remove can pin the exact CAS base revision while authoring no value.
 * This record owns both, so a remount never silently adopts a newer revision
 * that the user has not reviewed. It is unsaved browser intent plus the exact
 * native revision the next mutation is fenced on; it is never persisted, merged
 * with sources, or used as runtime state. */
export interface UnitEditState {
  /** The browser's value override. Present only after an explicit Override or edit. */
  draft?: { value: unknown };
  /** The exact CAS base revision the next mutation is fenced on. */
  base: string;
  /** True once `base` is pinned by an edit or a mutation attempt. While pinned,
   * an observed revision never advances `base` on its own; only a confirmed
   * commit, Discard, or the explicit reviewed-revision gesture moves it. */
  pinned: boolean;
  /** Monotonic browser-intent generation. Every edit bumps it so an older
   * submitted mutation can never retire a newer intent. */
  generation: number;
  /** The last submitted mutation whose authoritative projection has not been
   * observed yet. It is the only thing settlement may retire. */
  submitting?: SubmittedMutation;
}

type Listener = () => void;

/** The single owner of every Settings unit's editing transaction.
 *
 * Invariant: once mutation M has been confirmed committed and its authoritative
 * projection has been observed, M's submitted browser intent is retired exactly
 * once, independent of whether the originating editor component is still
 * mounted. A later editing intent is never retired by M.
 *
 * Settlement lives here, not in `UnitForm` local state: `Settings` records the
 * confirmed commit from the native acknowledgement and the authoritative
 * revision from the adopted projection, and this store retires exactly the
 * matching submitted mutation. That works whether or not the originating editor
 * component is still mounted. */
export class SettingsTransactionStore {
  private entries = new Map<string, UnitEditState>();
  private observed = new Map<string, string>();
  private listeners = new Set<Listener>();
  private revision = 0;
  private nextToken = 1;

  subscribe = (listener: Listener) => { this.listeners.add(listener); return () => { this.listeners.delete(listener); }; };
  /** Stable snapshot for `useSyncExternalStore`; increments on every mutation. */
  version = () => this.revision;

  read(identity: string): UnitEditState | undefined { return this.entries.get(identity); }

  private changed() { this.revision++; for (const listener of [...this.listeners]) listener(); }

  /** Record one browser editing intent. Keeps any in-flight mutation so its
   * committed revision can still advance the base, but the newer generation
   * guarantees it cannot retire this intent. */
  edit(identity: string, base: string, value: unknown): void {
    const current = this.entries.get(identity);
    this.entries.set(identity, {
      draft: { value }, base: current?.base ?? base, pinned: true,
      generation: (current?.generation ?? 0) + 1,
      submitting: current?.submitting,
    });
    this.changed();
  }

  /** Open one mutation for the current intent (or a clean Remove with none) and
   * return its token. The exact base is frozen for the submission, and only the
   * mutation's non-sensitive revision selector is retained — never its authored
   * payload. */
  beginSubmit(identity: string, base: string, selector: RevisionSelector): number {
    const current = this.entries.get(identity);
    const generation = current?.generation ?? 0;
    const token = this.nextToken++;
    this.entries.set(identity, {
      draft: current?.draft, base, pinned: true, generation,
      submitting: { token, generation, savedFrom: base, selector },
    });
    this.changed();
    return token;
  }

  /** Record native confirmation of exactly one submitted mutation. The committed
   * revision advances the CAS base, and the confirmed draft — the last place a
   * literal credential or MCP literal value still lives — is dropped unless the
   * browser intent has moved on to a newer one the user is still editing. */
  acknowledge(identity: string, token: number, committed: string): void {
    const entry = this.entries.get(identity), submitting = entry?.submitting;
    if (!entry || !submitting || submitting.token !== token) return;
    submitting.committed = committed;
    if (submitting.generation === entry.generation) entry.draft = undefined;
    entry.base = committed;
    entry.pinned = true;
    this.trySettle(identity);
    this.changed();
  }

  /** A write did not commit (conflict, rejection, uncertain outcome, lost
   * target). Keep the browser intent and the exact reviewed base. */
  fail(identity: string, token: number): void {
    const entry = this.entries.get(identity), submitting = entry?.submitting;
    if (!entry || !submitting || submitting.token !== token) return;
    delete entry.submitting;
    this.changed();
  }

  /** Record the authoritative revision one unit's projection currently carries.
   * Retires a matching acknowledged mutation even when its origin is unmounted. */
  observe(identity: string, revision: string): void {
    this.observed.set(identity, revision);
    this.trySettle(identity);
    this.changed();
  }

  /** Adopt one whole authoritative projection and retire every acknowledged
   * mutation whose committed revision it now carries. */
  observeAll(projection: SourceSettings): void {
    for (const [identity, entry] of this.entries) {
      const selector = entry.submitting?.selector;
      if (selector) this.observed.set(identity, selectedRevision(projection, selector));
    }
    for (const identity of this.entries.keys()) this.trySettle(identity);
    this.changed();
  }

  discard(identity: string): void {
    if (!this.entries.delete(identity)) return;
    this.observed.delete(identity);
    this.changed();
  }

  /** The explicit reviewed-revision gesture: advance the base deliberately. */
  review(identity: string, revision: string): void {
    const entry = this.entries.get(identity);
    if (!entry) return;
    entry.base = revision;
    entry.pinned = true;
    this.changed();
  }

  /** Everything this store currently retains, for the regression that proves no
   * secret-bearing authored payload survives a confirmed commit. Production code
   * never reads it. */
  retainedState(): readonly unknown[] { return [...this.entries.entries(), ...this.observed.entries()]; }

  private trySettle(identity: string): void {
    const entry = this.entries.get(identity), submitting = entry?.submitting;
    if (!entry || !submitting?.committed) return;
    if (this.observed.get(identity) !== submitting.committed) return;
    if (submitting.generation === entry.generation) {
      this.entries.delete(identity);
      this.observed.delete(identity);
    } else {
      // A newer intent survives; advance its CAS base to the commit it now
      // replaces and retire only the submitted mutation.
      entry.base = submitting.committed;
      entry.pinned = true;
      delete entry.submitting;
    }
  }
}

export const EditorStateContext = createContext<SettingsTransactionStore | undefined>(undefined);
export const SourceContext = createContext<SourceSettings | undefined>(undefined);
