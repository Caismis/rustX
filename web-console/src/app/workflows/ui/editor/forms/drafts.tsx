import { createContext, useContext, useEffect, useRef } from 'react';
import { useSelector } from '@xstate/react';
import type { WorkflowEditorActor } from '../../../machines/editor';

/** The apply callbacks of the deferred-commit fields currently shown, so
 * Save and Check first apply visible valid text through each field's own
 * commit path (a rename keeps its selection, for instance). */
export class FieldCommits {
  private readonly commits = new Map<string, () => void>();
  register(key: string, commit: () => void) {
    this.commits.set(key, commit);
    return () => { if (this.commits.get(key) === commit) this.commits.delete(key); };
  }
  /** Apply every shown field's valid text now. */
  flush() { for (const commit of [...this.commits.values()]) commit(); }
}

export const DraftOwner = createContext<{ editor: WorkflowEditorActor; commits: FieldCommits } | undefined>(undefined);

/** One deferred-commit field's unapplied text, owned by the editor machine
 * under the field's stable identity `key`, never by the component: it is
 * dirty, survives the field being hidden and shown again, and is applied
 * by Save, Check, blur or Enter. Text that cannot be applied stays visible
 * and blocks Save and Check; it never enters the document.
 *
 * `commit` applies valid text and reports whether the document took it. */
export function useFieldDraft({ key, canonical, valid, commit }: {
  key: string; canonical: string; valid: (text: string) => boolean; commit: (text: string) => boolean;
}) {
  const owner = useContext(DraftOwner);
  if (!owner) throw new Error('A deferred-commit field needs its editor');
  const { editor, commits } = owner;
  const draft = useSelector(editor, snapshot => snapshot.context.drafts[key]);
  // A Save or Check refused because of unapplied text flags every such field.
  const flagged = useSelector(editor, snapshot => snapshot.context.notice?.kind === 'pending_fields');
  const change = (text: string) => editor.send({ type: 'DRAFT', key, draft: text === canonical ? undefined : { text, valid: valid(text) } });
  /** Apply the current draft, read from the machine (a keystroke may not
   * have rendered yet when Save runs). */
  const apply = () => {
    const current = editor.getSnapshot().context.drafts[key];
    if (current?.valid && commit(current.text)) editor.send({ type: 'DRAFT', key, draft: undefined });
  };
  const latest = useRef(apply);
  latest.current = apply;
  useEffect(() => commits.register(key, () => latest.current()), [commits, key]);
  // Another edit replaced the value (undo, rename, reload): the field shows it.
  const seen = useRef(canonical);
  useEffect(() => {
    if (seen.current === canonical) return;
    seen.current = canonical;
    editor.send({ type: 'DRAFT', key, draft: undefined });
  }, [editor, key, canonical]);
  return { text: draft?.text ?? canonical, pending: !!draft, invalid: !!draft && !draft.valid, flagged, change, apply, reset: () => change(canonical) };
}

/** The number contract of Workflow authoring (`unrepresentable_number` in
 * native): an authored number is accepted only when this browser's JSON
 * round trip returns the same native number — an integer within
 * ±(2^53 − 1), or a non-integral finite float (or an integral one from 1e21,
 * printed in exponent form). Anything else would be silently rounded or
 * retyped by `JSON.parse`, so it is refused before conversion, from the
 * authored token itself. */
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
  if (!Number.isFinite(value)) return false;
  if (/^-?\d+$/.test(token)) return Number.isSafeInteger(value);
  return !(Object.is(value, -0) || (Number.isInteger(value) && Math.abs(value) < 1e21));
}
