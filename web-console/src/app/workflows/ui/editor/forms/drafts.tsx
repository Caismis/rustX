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
