import { createContext } from 'react';
import type { SourceMutation, SourceSettings } from '../../../../protocol/app-server/v18';

/** One submitted mutation inside a unit's editing transaction.
 *
 * The token is the mutation's identity: a late acknowledgement carrying an older
 * token can never retire a newer browser intent. The generation captures the
 * browser intent that existed when the mutation was submitted; if the intent
 * moved on, the acknowledgement may advance the CAS base but must keep the newer
 * draft. `savedFrom` is the exact pre-save base, kept only so a projection that
 * has not caught up yet is not mistaken for an external change. */
export interface SubmittedMutation {
  token: number;
  generation: number;
  savedFrom: string;
  mutation: SourceMutation;
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
   * an observed revision never advances `base`; only Discard or the explicit
   * reviewed-revision gesture moves it. */
  pinned: boolean;
  /** Monotonic browser-intent generation. Every edit/discard bumps it so an
   * older submitted mutation can never retire a newer intent. */
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
   * return its token. The exact base is frozen for the submission. */
  beginSubmit(identity: string, base: string, mutation: SourceMutation): number {
    const current = this.entries.get(identity);
    const generation = current?.generation ?? 0;
    const token = this.nextToken++;
    this.entries.set(identity, {
      draft: current?.draft, base, pinned: true, generation,
      submitting: { token, generation, savedFrom: base, mutation },
    });
    this.changed();
    return token;
  }

  /** Record native confirmation of exactly one submitted mutation. The committed
   * revision advances the CAS base; a literal credential in the submitted draft
   * is dropped as soon as the intent has not moved on. */
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
  observeAll(projection: SourceSettings, revisionOf: (source: SourceSettings, mutation: SourceMutation) => string): void {
    for (const [identity, entry] of this.entries) {
      const mutation = entry.submitting?.mutation;
      if (mutation) this.observed.set(identity, revisionOf(projection, mutation));
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
