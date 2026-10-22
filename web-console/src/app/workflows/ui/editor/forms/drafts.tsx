import { createContext, useContext, useEffect, useRef } from 'react';
import { useSelector } from '@xstate/react';
import { encodeField, type FieldAddress } from '../../../document/fields';
import type { FieldCommand, FieldDraft, WorkflowEditorActor } from '../../../machines/editor';

/** The fields currently shown, so Save and Check first ask the editor to
 * apply their visible text. Each registration only requests an
 * application; the editor alone decides whether it was accepted. Renames
 * are requested last, after the other fields of the renamed node. */
export class FieldCommits {
  private readonly commits = new Set<{ readonly rename: boolean; readonly apply: () => void }>();
  register(rename: boolean, apply: () => void) {
    const entry = { rename, apply };
    this.commits.add(entry);
    return () => { this.commits.delete(entry); };
  }
  /** Request the application of every shown field's text now. */
  flush() {
    const entries = [...this.commits];
    for (const entry of entries) if (!entry.rename) entry.apply();
    for (const entry of entries) if (entry.rename) entry.apply();
  }
}

export const DraftOwner = createContext<{ editor: WorkflowEditorActor; commits: FieldCommits } | undefined>(undefined);

export function draftAt(drafts: Readonly<Record<string, FieldDraft>>, field: string): FieldDraft | undefined {
  return Object.hasOwn(drafts, field) ? drafts[field] : undefined;
}

/** One deferred-commit field. Its unapplied text belongs to the editor
 * under the field's address, never to the component: it is dirty and
 * survives selection, navigation and remounts. Blur, Enter, Save and
 * Check request its application through `command`; the editor applies it
 * against the current document or keeps it with the error. `onApplied`
 * runs only after the editor accepted exactly the shown text. */
export function useFieldDraft({ address, canonical, command, onApplied }: {
  address: FieldAddress; canonical: string; command: FieldCommand; onApplied?: (text: string) => void;
}) {
  const owner = useContext(DraftOwner);
  if (!owner) throw new Error('A deferred-commit field needs its editor');
  const { editor, commits } = owner;
  const field = encodeField(address);
  const draft = useSelector(editor, snapshot => draftAt(snapshot.context.drafts, field));
  const latest = useRef({ address, command, onApplied });
  latest.current = { address, command, onApplied };
  const change = (text: string) => editor.send({ type: 'DRAFT', address: latest.current.address, text });
  /** Request the application of the current version, read from the editor
   * (a keystroke may not have rendered yet when Save runs). */
  const apply = useRef(() => {});
  apply.current = () => {
    const current = draftAt(editor.getSnapshot().context.drafts, field);
    if (!current) return;
    editor.send({ type: 'APPLY_DRAFT', field, version: current.version, command: latest.current.command });
    const outcome = editor.getSnapshot().context.applied;
    if (outcome?.field === field && outcome.version === current.version && outcome.result.status === 'applied') latest.current.onApplied?.(current.text);
  };
  const rename = address.role === 'key';
  useEffect(() => commits.register(rename, () => apply.current()), [commits, field, rename]);
  const error = draft?.conflict ? { kind: draft.conflict } as const : draft?.error;
  return {
    text: draft?.text ?? canonical, pending: !!draft, error, conflict: draft?.conflict,
    change, apply: () => apply.current(),
    reset: () => editor.send({ type: 'DISCARD_DRAFT', field }),
    keep: () => editor.send({ type: 'KEEP_DRAFT', field }),
  };
}

/** The number contract of Workflow authoring (`unrepresentable_number` in
 * native): an authored number is accepted only when this browser's JSON
 * round trip returns the same native number — an integer spelled without a
 * fraction or exponent within ±(2^53 − 1), or a non-integral finite float
 * (or an integral one from 1e21, printed in exponent form). Anything else —
 * `9007199254740993`, `1.0`, `1e3`, `-0` — would be silently rounded,
 * retyped or re-signed by `JSON.parse` and `JSON.stringify`, so it is
 * refused before conversion, from the authored token itself. */
export type JsonReading = { ok: true; value: unknown } | { ok: false; reason: 'syntax' | 'number' };

export function readJson(text: string): JsonReading {
  let exact = true;
  let value: unknown;
  try {
    value = JSON.parse(text, function (this: unknown, _key: string, parsed: unknown, context?: { source?: string }) {
      if (typeof parsed === 'number') {
        const source = context?.source;
        if (source === undefined || !representable(source, parsed)) exact = false;
      }
      return parsed;
    } as (this: unknown, key: string, value: unknown) => unknown);
  } catch { return { ok: false, reason: 'syntax' }; }
  return exact ? { ok: true, value } : { ok: false, reason: 'number' };
}

function representable(token: string, value: number): boolean {
  // Negative zero matches the integer spelling too (`-0`), yet serializes
  // as `0`: it is refused before any integer rule can accept it.
  if (!Number.isFinite(value) || Object.is(value, -0)) return false;
  if (/^-?\d+$/.test(token)) return Number.isSafeInteger(value);
  return !(Number.isInteger(value) && Math.abs(value) < 1e21);
}
