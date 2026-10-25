import { assign, enqueueActions, fromPromise, setup, type ActorRefFrom, type SnapshotFrom } from 'xstate';
import type { WorkflowCheckReport, WorkflowSource } from '../../../../../protocol/app-server/v57';
import type { WorkflowCommit } from '../../../workspaces/host';
import { CommandRejected, type RejectionCode } from '../document/commands';
import { encodeField, fieldText, invertArrayEdit, remapAddress, renameAddress, sameField, type ArrayEdit, type FieldAddress } from '../document/fields';
import { incompleteIdentities, type IncompleteIdentity } from '../document/identities';
import { existingPath, sameDocument, ROOT, type BlockPath, type WorkflowDocument } from '../document/model';
import { classifyFailure, type WorkflowFailure, type WorkflowScopePort, type WorkflowTarget } from '../port';

const HISTORY_LIMIT = 100;

/** One correlated native check of exactly one candidate generation. */
export interface CheckResult {
  readonly generation: number;
  readonly report: WorkflowCheckReport;
  readonly explain: boolean;
  /** The Workspace registration a Global file's dependencies were checked in. */
  readonly context?: string;
}

/** The last committed save: its file revision and the generation it wrote. */
export interface SaveReceipt {
  readonly revision: string;
  readonly configurationRevision: string | null;
  readonly generation: number;
  /** Whether an authoritative reread observed exactly this commit. */
  readonly confirmed: boolean;
}

/** Facts reported to the author. Each is distinct; none is a green light. */
export type EditorNotice =
  | { kind: 'failure'; operation: 'check' | 'save' | 'reconcile'; failure: WorkflowFailure }
  /** Saved, but the reread failed: committed and unconfirmed. Never "unsaved". */
  | { kind: 'committed_unconfirmed' }
  /** Saved, and the reread found a different revision: the file changed again. */
  | { kind: 'changed_after_save'; revision: string }
  /** Reconciliation found the uncertain save did not commit. */
  | { kind: 'not_committed' }
  /** Reconciliation found the uncertain save committed. */
  | { kind: 'reconciled' }
  /** Reconciliation found a revision this editor did not write. */
  | { kind: 'changed_externally'; revision: string }
  /** Typed identities still to choose; nothing was sent. */
  | { kind: 'incomplete'; identities: readonly IncompleteIdentity[] }
  /** Field text that could not be applied (it is invalid, or its field is
   * not shown); nothing was sent and the text is kept. */
  | { kind: 'pending_fields'; count: number };

/** Why a field's text was not applied. Its text is kept with the error. */
export type FieldError =
  | { kind: 'syntax' } | { kind: 'number' } | { kind: 'empty' }
  | { kind: 'rejected'; code: RejectionCode }
  /** The field's value changed under the text (an undo, for instance). */
  | { kind: 'changed' }
  /** The field the text was typed into no longer exists. */
  | { kind: 'missing' }
  /** A rename would place this field's text where other kept text is. */
  | { kind: 'occupied' };

/** Unapplied text of one deferred-commit field, under its address. It
 * never enters the document until an `APPLY_DRAFT` of this exact version
 * is accepted, and it survives selection, navigation and remounts. */
export interface FieldDraft {
  readonly address: FieldAddress;
  readonly text: string;
  /** Increments on every text change, so an older application request
   * never applies or clears newer text. */
  readonly version: number;
  /** The field's text when this draft began. */
  readonly base: string;
  /** The error of the latest application attempt of this text. */
  readonly error?: FieldError;
  /** The document changed the field under this text. Sticky: it is
   * resolved only by the author (keep the text, or discard it). */
  readonly conflict?: 'changed' | 'missing';
}

/** The outcome of one field application, as the editor determined it. */
export type ApplyResult =
  | { status: 'applied' }
  /** The field already holds exactly this value; nothing changed. */
  | { status: 'unchanged' }
  | { status: 'rejected'; error: FieldError };

/** What applying a field's text does to the current document. A refused
 * structured command may also throw `CommandRejected`. */
export type FieldCommand = (document: WorkflowDocument, text: string) =>
  | { status: 'applied'; document: WorkflowDocument }
  | { status: 'unchanged' }
  | { status: 'rejected'; error: FieldError };

/** Browser-local node coordinates of one block, by node id. */
export type ViewPositions = Readonly<Record<string, { readonly x: number; readonly y: number }>>;

/** An accepted node/branch identity change, including its exact inverse. */
export interface IdentityRename { readonly from: FieldAddress; readonly to: FieldAddress }
type PresentationEvent =
  | { type: 'view.rename'; rename: IdentityRename }
  | { type: 'view.restore'; block: string; positions: ViewPositions };

/** One undoable step. A view step moves nodes of one block and never
 * changes the document; both kinds share one timeline. */
export type HistoryEntry =
  /** `array` is the array edit that led from `document` to the next state. */
  | { readonly kind: 'document'; readonly document: WorkflowDocument; readonly array?: ArrayEdit; readonly rename?: IdentityRename }
  | { readonly kind: 'view'; readonly block: string; readonly path: BlockPath; readonly before: ViewPositions; readonly after: ViewPositions };

/** How an uncertain save was settled by observation alone. */
type Reconciliation = { kind: 'not_committed' | 'committed' | 'changed'; source: WorkflowSource };

export interface EditorContext {
  readonly port: WorkflowScopePort;
  readonly target: WorkflowTarget;
  /** The last authoritative observation of the file and its document. */
  base?: { revision: string; source: WorkflowSource; document?: WorkflowDocument };
  /** The one working document; absent while loading or for an unreadable file. */
  document?: WorkflowDocument;
  /** Increments on every semantic change of `document`. */
  generation: number;
  past: HistoryEntry[];
  future: HistoryEntry[];
  /** The coalescing key of the latest edit, so one field session is one
   * undo step. Navigation, selection, saving, checking, a node move and
   * undo or redo all end the session. */
  coalesce?: string;
  /** Unapplied field text by encoded address: dirty, never written. */
  drafts: Readonly<Record<string, FieldDraft>>;
  /** The source of draft versions. */
  draftVersion: number;
  /** The outcome of the latest `APPLY_DRAFT` that matched its draft. */
  applied?: { readonly field: string; readonly version: number; readonly result: ApplyResult };
  /** The node positions the latest undo or redo of a view step restores. */
  view?: { readonly seq: number; readonly block: string; readonly positions: ViewPositions };
  block: BlockPath;
  check?: CheckResult;
  /** The input of the latest check; in flight only while `ready.checking`. */
  checking?: { generation: number; document: WorkflowDocument; explain: boolean; context?: string };
  saving?: { generation: number; document: WorkflowDocument };
  /** Set by an uncertain save; a new save requires reconciliation first. */
  uncertain?: { generation: number; document: WorkflowDocument };
  receipt?: SaveReceipt;
  notice?: EditorNotice;
  rejection?: RejectionCode;
  loadFailure?: WorkflowFailure;
}

export type EditorEvent =
  /** A semantic edit. `array` describes it when it removes or inserts one
   * array item, so drafts of the other items keep their identity. */
  | { type: 'EDIT'; apply: (document: WorkflowDocument) => WorkflowDocument; coalesce?: string; array?: ArrayEdit }
  | { type: 'UNDO' } | { type: 'REDO' }
  /** One completed drag of nodes in one block: a view step. */
  | { type: 'MOVE'; block: string; path: BlockPath; before: ViewPositions; after: ViewPositions }
  /** A field's text changed. Text equal to the field's value is no draft. */
  | { type: 'DRAFT'; address: FieldAddress; text: string }
  /** Apply exactly version `version` of the draft of `field`. */
  | { type: 'APPLY_DRAFT'; field: string; version: number; command: FieldCommand }
  /** Keep a conflicting draft's text over the field's current value. */
  | { type: 'KEEP_DRAFT'; field: string }
  | { type: 'DISCARD_DRAFT'; field: string }
  | { type: 'DISCARD_DRAFTS' }
  /** The author moved on (another selection): the edit session ends. */
  | { type: 'BOUNDARY' }
  | { type: 'NAVIGATE'; block: BlockPath }
  | { type: 'CHECK'; explain: boolean; context?: string }
  | { type: 'SAVE' }
  | { type: 'RECONCILE' }
  | { type: 'RELOAD' }
  | { type: 'DISMISS' };

export interface EditorInput { readonly port: WorkflowScopePort; readonly target: WorkflowTarget }

/** Unsaved: the document differs from its base, or a field holds text it
 * has not applied. */
export function isDirty(context: Pick<EditorContext, 'document' | 'base' | 'drafts'>): boolean {
  return Object.keys(context.drafts).length > 0
    || (!!context.document && !!context.base?.document && context.document !== context.base.document && !sameDocument(context.document, context.base.document));
}

function failure(event: unknown, write: boolean) {
  return classifyFailure((event as { error: unknown }).error, write);
}

function committed(context: EditorContext, saving: NonNullable<EditorContext['saving']>, commit: WorkflowCommit): Partial<EditorContext> {
  const { revision, configuration_revision: configurationRevision } = commit.acknowledgement;
  const reread = commit.reread;
  const confirmed = reread.status === 'observed' && reread.source.revision === revision;
  const notice: EditorNotice | undefined = reread.status === 'failed' ? { kind: 'committed_unconfirmed' }
    : reread.source.revision !== revision ? { kind: 'changed_after_save', revision: reread.source.revision } : undefined;
  return {
    // The commit's own revision is the next CAS base; the working document
    // is never replaced by a reread, so later edits survive.
    base: { revision, source: confirmed && reread.status === 'observed' ? reread.source : { ...context.base!.source, revision, present: true }, document: saving.document },
    receipt: { revision, configurationRevision, generation: saving.generation, confirmed },
    saving: undefined, uncertain: undefined, notice,
  };
}

/** Restore a view step's positions, in its own block. */
function restore(context: EditorContext, entry: Extract<HistoryEntry, { kind: 'view' }>, positions: ViewPositions): Partial<EditorContext> {
  return {
    view: { seq: (context.view?.seq ?? 0) + 1, block: entry.block, positions },
    block: context.document ? existingPath(context.document, entry.path) : context.block,
  };
}

function draftOf(context: Pick<EditorContext, 'drafts'>, field: string): FieldDraft | undefined {
  return Object.hasOwn(context.drafts, field) ? context.drafts[field] : undefined;
}

/** The record key of a draft whose field no longer exists. It is never
 * an address any field shows, so such text stays listed for the author but
 * is never taken over by a field created or renamed into that place later,
 * and never occupies the key a live draft moves to. */
function detachedKey(draft: FieldDraft): string {
  return JSON.stringify(['detached', draft.version, encodeField(draft.address)]);
}

/** Drafts after the document changed from `before` to `after`. Every
 * attached draft, whatever its conflict, follows its own field: `moved`
 * drafts followed a rename and `array` is the array edit performed, so the
 * drafts it accounts for keep their identity. A draft whose field is gone
 * (or whose array item cannot be identified) keeps its text and is
 * detached; one whose field changed keeps its text and is marked, and that
 * mark stays until the author resolves it. Detached drafts stay as they
 * are, and no draft ever replaces another under one key. */
function settleDrafts(drafts: EditorContext['drafts'], before: WorkflowDocument | undefined, after: WorkflowDocument, { moved, array }: { moved?: ReadonlySet<string>; array?: ArrayEdit } = {}): EditorContext['drafts'] {
  const next = new Map<string, FieldDraft>();
  const detach = (draft: FieldDraft) => { const missing = { ...draft, conflict: 'missing' as const }; next.set(detachedKey(missing), missing); };
  const attached: [string, FieldDraft][] = [];
  for (const [field, draft] of Object.entries(drafts)) {
    if (draft.conflict === 'missing') { next.set(field, draft); continue; }
    const remapped = array ? remapAddress(draft.address, array) : { address: draft.address, covered: false };
    if (!remapped) { detach(draft); continue; }
    const { address, covered } = remapped;
    const text = fieldText(after, address);
    // A draft that followed a rename, or whose array edit is known, is
    // identified already; any other array resize leaves identity unknown.
    const shifted = !!before && !covered && !moved?.has(field) && !sameField(before, after, draft.address);
    if (text === undefined || shifted) { detach(draft); continue; }
    const current = address === draft.address ? draft : { ...draft, address };
    attached.push([encodeField(address), draft.conflict || text !== draft.base ? { ...current, conflict: 'changed' } : current]);
  }
  for (const [key, draft] of attached) {
    // Two drafts never share one field: neither is given the other's place.
    if (next.has(key) || attached.filter(([other]) => other === key).length > 1) detach(draft);
    else next.set(key, draft);
  }
  return Object.fromEntries(next);
}

function renamedPath(path: BlockPath, rename: IdentityRename): BlockPath {
  const name = rename.to.path.length === 0 ? rename.to.node! : (rename.to.path[1] as { key: string }).key;
  return renameAddress({ block: path, path: [], role: 'value' }, rename.from, name).block;
}

/** A semantic change of the working document: one undo step, or part of
 * the open session `coalesce`. */
function changeDocument(context: EditorContext, next: WorkflowDocument, coalesce: string | undefined, { drafts = context.drafts, moved, array, rename }: { drafts?: EditorContext['drafts']; moved?: ReadonlySet<string>; array?: ArrayEdit; rename?: IdentityRename } = {}): Partial<EditorContext> {
  const coalesced = coalesce !== undefined && coalesce === context.coalesce && context.past.at(-1)?.kind === 'document' && !array && !rename;
  return {
    document: next, generation: context.generation + 1, rejection: undefined,
    past: coalesced ? context.past : [...context.past, { kind: 'document' as const, document: context.document!, ...(array ? { array } : {}), ...(rename ? { rename } : {}) }].slice(-HISTORY_LIMIT),
    future: [], coalesce: array || rename ? undefined : coalesce, block: existingPath(next, rename ? renamedPath(context.block, rename) : context.block),
    drafts: settleDrafts(drafts, context.document, next, { moved, array }),
  };
}

/** Apply one draft version within one context update: the command runs
 * against the current document; only an accepted change clears exactly
 * that draft, and a rejection keeps its text with the error. */
function applyDraft(context: EditorContext, event: Extract<EditorEvent, { type: 'APPLY_DRAFT' }>): Partial<EditorContext> {
  const draft = draftOf(context, event.field);
  if (!draft || draft.version !== event.version || !context.document) return {};
  const report = (result: ApplyResult) => ({ field: event.field, version: event.version, result });
  const { [event.field]: _applied, ...rest } = context.drafts;
  if (draft.conflict) {
    const error: FieldError = { kind: draft.conflict };
    return { drafts: { ...rest, [event.field]: { ...draft, error } }, applied: report({ status: 'rejected', error }) };
  }
  let outcome: ReturnType<FieldCommand>;
  try { outcome = event.command(context.document, draft.text); }
  catch (cause) {
    if (!(cause instanceof CommandRejected)) throw cause;
    outcome = { status: 'rejected', error: { kind: 'rejected', code: cause.code } };
  }
  if (outcome.status === 'rejected') return { drafts: { ...rest, [event.field]: { ...draft, error: outcome.error } }, applied: report(outcome) };
  if (outcome.status === 'unchanged') return { drafts: rest, applied: report(outcome) };
  // A renamed node or branch takes its other fields' drafts along.
  let others: EditorContext['drafts'] = rest;
  const moved = new Set<string>();
  if (draft.address.role === 'key') {
    const to = draft.text.trim();
    const entries = Object.entries(rest).map(([key, other]): [string, FieldDraft] => {
      // A detached draft keeps its own identity; it never follows a rename.
      if (other.conflict === 'missing') return [key, other];
      const address = renameAddress(other.address, draft.address, to);
      if (address === other.address) return [key, other];
      moved.add(encodeField(address));
      return [encodeField(address), { ...other, address }];
    });
    others = Object.fromEntries(entries);
    // Every retained draft keeps its own place: a rename that would put two
    // under one field is refused, with nothing changed.
    if (Object.keys(others).length !== entries.length) {
      const error: FieldError = { kind: 'occupied' };
      return { drafts: { ...rest, [event.field]: { ...draft, error } }, applied: report({ status: 'rejected', error }) };
    }
  }
  const renamed = draft.address.role === 'key' ? renameAddress(draft.address, draft.address, draft.text.trim()) : draft.address;
  const rename = renamed !== draft.address ? { from: draft.address, to: renamed } : undefined;
  return { ...changeDocument(context, outcome.document, event.field, { drafts: others, moved, rename }), applied: report({ status: 'applied' }) };
}

/** One editing transaction of one exact Workflow file. React subscribes and
 * submits intent; this machine alone arbitrates the file target, the base
 * revision, edit generations and correlated native operations. */
export const workflowEditorMachine = setup({
  types: { context: {} as EditorContext, events: {} as EditorEvent, input: {} as EditorInput, emitted: {} as PresentationEvent },
  actors: {
    read: fromPromise<WorkflowSource, { port: WorkflowScopePort; id: string }>(({ input }) => input.port.read(input.id)),
    check: fromPromise<WorkflowCheckReport, { port: WorkflowScopePort; id: string; document: WorkflowDocument; explain: boolean; context?: string }>(
      ({ input }) => input.port.check(input.id, input.document, input.explain, input.context)),
    save: fromPromise<WorkflowCommit, { port: WorkflowScopePort; id: string; revision: string; document: WorkflowDocument }>(
      ({ input }) => input.port.write(input.id, input.revision, { operation: 'replace', document: input.document })),
    // Observation only: the uncertain write is never sent again. Native
    // names the revision the attempted candidate commits as, so its own
    // normalized commit is recognized by identity, not by shape.
    reconcile: fromPromise<Reconciliation, { port: WorkflowScopePort; id: string; base: string; document: WorkflowDocument }>(async ({ input }) => {
      const source = await input.port.read(input.id);
      if (source.revision === input.base) return { kind: 'not_committed', source };
      const report = await input.port.check(input.id, input.document, false);
      const committed = report.structure.status === 'valid' && report.structure.revision === source.revision;
      return { kind: committed ? 'committed' : 'changed', source };
    }),
  },
  guards: {
    editable: ({ context }) => !!context.document && !!context.base?.document,
    savable: ({ context }) => !!context.document && !!context.base?.document && !context.uncertain,
    incomplete: ({ context }) => !!context.document && incompleteIdentities(context.document).length > 0,
    canUndo: ({ context }) => context.past.length > 0,
    canRedo: ({ context }) => context.future.length > 0,
    pendingFields: ({ context }) => Object.keys(context.drafts).length > 0,
  },
  actions: {
    edit: assign(({ context, event }) => {
      if (event.type !== 'EDIT' || !context.document) return {};
      let next: WorkflowDocument;
      try { next = event.apply(context.document); }
      catch (cause) {
        if (cause instanceof CommandRejected) return { rejection: cause.code };
        throw cause;
      }
      if (next === context.document) return { rejection: undefined };
      return changeDocument(context, next, event.coalesce, { array: event.array });
    }),
    applyDraft: enqueueActions(({ context, event, enqueue }) => {
      if (event.type !== 'APPLY_DRAFT') return;
      const update = applyDraft(context, event);
      enqueue.assign(update);
      const entry = update.document && update.past?.at(-1);
      if (entry && entry.kind === 'document' && entry.rename) enqueue.emit({ type: 'view.rename', rename: entry.rename });
    }),
    move: assign(({ context, event }) => {
      if (event.type !== 'MOVE') return {};
      const entry: HistoryEntry = { kind: 'view', block: event.block, path: event.path, before: event.before, after: event.after };
      return { past: [...context.past, entry].slice(-HISTORY_LIMIT), future: [], coalesce: undefined };
    }),
    undo: enqueueActions(({ context, enqueue }) => {
      const entry = context.past.at(-1)!, past = context.past.slice(0, -1);
      if (entry.kind === 'view') {
        enqueue.assign({ past, future: [entry, ...context.future], coalesce: undefined, rejection: undefined, ...restore(context, entry, entry.before) });
        enqueue.emit({ type: 'view.restore', block: entry.block, positions: entry.before });
        return;
      }
      const rename = entry.rename && { from: entry.rename.to, to: entry.rename.from };
      // Undoing an array edit performs its inverse on the drafts as well.
      enqueue.assign({ document: entry.document, past, future: [{ kind: 'document' as const, document: context.document!, ...(entry.array ? { array: entry.array } : {}), ...(entry.rename ? { rename: entry.rename } : {}) }, ...context.future], generation: context.generation + 1, coalesce: undefined, rejection: undefined, block: existingPath(entry.document, rename ? renamedPath(context.block, rename) : context.block), drafts: settleDrafts(context.drafts, context.document, entry.document, { array: entry.array && invertArrayEdit(entry.array) }) });
      if (rename) enqueue.emit({ type: 'view.rename', rename });
    }),
    redo: enqueueActions(({ context, enqueue }) => {
      const [entry, ...future] = context.future;
      if (entry.kind === 'view') {
        enqueue.assign({ past: [...context.past, entry], future, coalesce: undefined, rejection: undefined, ...restore(context, entry, entry.after) });
        enqueue.emit({ type: 'view.restore', block: entry.block, positions: entry.after });
        return;
      }
      enqueue.assign({ document: entry.document, past: [...context.past, { kind: 'document' as const, document: context.document!, ...(entry.array ? { array: entry.array } : {}), ...(entry.rename ? { rename: entry.rename } : {}) }], future, generation: context.generation + 1, coalesce: undefined, rejection: undefined, block: existingPath(entry.document, entry.rename ? renamedPath(context.block, entry.rename) : context.block), drafts: settleDrafts(context.drafts, context.document, entry.document, { array: entry.array }) });
      if (entry.rename) enqueue.emit({ type: 'view.rename', rename: entry.rename });
    }),
    navigate: assign(({ context, event }) => event.type === 'NAVIGATE' && context.document ? { block: existingPath(context.document, event.block), coalesce: undefined } : {}),
    boundary: assign({ coalesce: undefined }),
    // The pending-field notice ends with the last pending field.
    settleNotice: assign(({ context }) => context.notice?.kind === 'pending_fields' && !Object.keys(context.drafts).length ? { notice: undefined } : {}),
    draft: assign(({ context, event }) => {
      if (event.type !== 'DRAFT' || !context.document) return {};
      const field = encodeField(event.address), existing = draftOf(context, field);
      const { [field]: _previous, ...rest } = context.drafts;
      if (!existing) {
        // Text can only be typed into a field that exists now.
        const base = fieldText(context.document, event.address);
        if (base === undefined || event.text === base) return {};
        return { drafts: { ...rest, [field]: { address: event.address, text: event.text, version: context.draftVersion + 1, base } }, draftVersion: context.draftVersion + 1 };
      }
      // Typing back to the starting value leaves nothing unapplied.
      if (!existing.conflict && event.text === existing.base) return { drafts: rest };
      return { drafts: { ...rest, [field]: { ...existing, text: event.text, version: context.draftVersion + 1, error: undefined } }, draftVersion: context.draftVersion + 1 };
    }),
    keepDraft: assign(({ context, event }) => {
      if (event.type !== 'KEEP_DRAFT' || !context.document) return {};
      const draft = draftOf(context, event.field);
      if (draft?.conflict !== 'changed') return {};
      const base = fieldText(context.document, draft.address);
      // The field has gone meanwhile: the text cannot be kept for it.
      if (base === undefined) {
        const { [event.field]: _gone, ...others } = context.drafts;
        const missing = { ...draft, conflict: 'missing' as const };
        return { drafts: { ...others, [detachedKey(missing)]: missing } };
      }
      const { conflict: _resolved, error: _error, ...kept } = draft;
      const { [event.field]: _previous, ...rest } = context.drafts;
      return { drafts: kept.text === base ? rest : { ...rest, [event.field]: { ...kept, base, version: context.draftVersion + 1 } }, draftVersion: context.draftVersion + 1 };
    }),
    discardDraft: assign(({ context, event }) => {
      if (event.type !== 'DISCARD_DRAFT') return {};
      const { [event.field]: _discarded, ...rest } = context.drafts;
      return { drafts: rest };
    }),
    discardDrafts: assign({ drafts: {}, notice: undefined }),
    pendingFields: assign(({ context }) => ({ notice: { kind: 'pending_fields', count: Object.keys(context.drafts).length } as EditorNotice })),
    dismiss: assign({ notice: undefined, rejection: undefined }),
    incomplete: assign(({ context }) => ({ notice: { kind: 'incomplete', identities: incompleteIdentities(context.document!) } as EditorNotice })),
  },
}).createMachine({
  id: 'workflowEditor',
  context: ({ input }) => ({ port: input.port, target: input.target, generation: 0, past: [], future: [], block: ROOT, drafts: {}, draftVersion: 0 }),
  initial: 'loading',
  states: {
    loading: {
      entry: assign({ loadFailure: undefined, notice: undefined }),
      invoke: {
        src: 'read', input: ({ context }) => ({ port: context.port, id: context.target.id }),
        onDone: { target: 'ready', actions: assign(({ context, event }) => {
          const document = event.output.document ?? undefined;
          return { base: { revision: event.output.revision, source: event.output, document }, document, generation: context.generation + 1, past: [], future: [], coalesce: undefined, drafts: {}, applied: undefined, block: ROOT, check: undefined, receipt: undefined, uncertain: undefined };
        }) },
        onError: { target: 'failed', actions: assign(({ event }) => ({ loadFailure: failure(event, false) })) },
      },
    },
    failed: { on: { RELOAD: 'loading' } },
    ready: {
      initial: 'idle',
      on: {
        EDIT: { guard: 'editable', actions: 'edit' },
        MOVE: { guard: 'editable', actions: 'move' },
        UNDO: { guard: 'canUndo', actions: 'undo' },
        REDO: { guard: 'canRedo', actions: 'redo' },
        NAVIGATE: { actions: 'navigate' },
        BOUNDARY: { actions: 'boundary' },
        DRAFT: { guard: 'editable', actions: ['draft', 'settleNotice'] },
        APPLY_DRAFT: { guard: 'editable', actions: ['applyDraft', 'settleNotice'] },
        KEEP_DRAFT: { actions: 'keepDraft' },
        DISCARD_DRAFT: { actions: ['discardDraft', 'settleNotice'] },
        DISCARD_DRAFTS: { actions: 'discardDrafts' },
        DISMISS: { actions: 'dismiss' },
      },
      states: {
        idle: {
          on: {
            CHECK: [{ guard: 'pendingFields', actions: 'pendingFields' }, { guard: 'incomplete', actions: 'incomplete' }, { guard: 'editable', target: 'checking' }],
            SAVE: [{ guard: 'pendingFields', actions: 'pendingFields' }, { guard: 'incomplete', actions: 'incomplete' }, { guard: 'savable', target: 'saving' }],
            RECONCILE: { guard: ({ context }) => !!context.uncertain, target: 'reconciling' },
            RELOAD: '#workflowEditor.loading',
          },
        },
        checking: {
          // A newer check supersedes the older one by stopping it, so the
          // older result has no completion path at all.
          // `checking` is the exact input of the current check. It stays set
          // through the transition that consumes its result.
          entry: assign(({ context, event }) => ({ checking: { generation: context.generation, document: context.document!, explain: event.type === 'CHECK' && event.explain, context: event.type === 'CHECK' ? event.context : undefined }, notice: undefined, coalesce: undefined })),
          invoke: {
            src: 'check',
            input: ({ context }) => ({ port: context.port, id: context.target.id, document: context.checking!.document, explain: context.checking!.explain, context: context.checking!.context }),
            // The report names the generation it checked. A later edit never
            // inherits it: presentation compares it with the current one.
            onDone: { target: 'idle', actions: assign(({ context, event }) => ({ check: { generation: context.checking!.generation, report: event.output, explain: context.checking!.explain, context: context.checking!.context } })) },
            onError: { target: 'idle', actions: assign(({ event }) => ({ notice: { kind: 'failure', operation: 'check', failure: failure(event, false) } as EditorNotice })) },
          },
          on: {
            CHECK: [{ guard: 'pendingFields', target: 'idle', actions: 'pendingFields' }, { guard: 'incomplete', target: 'idle', actions: 'incomplete' }, { guard: 'editable', target: 'checking', reenter: true }],
            SAVE: [{ guard: 'pendingFields', target: 'idle', actions: 'pendingFields' }, { guard: 'incomplete', target: 'idle', actions: 'incomplete' }, { guard: 'savable', target: 'saving' }],
          },
        },
        saving: {
          entry: assign(({ context }) => ({ saving: { generation: context.generation, document: context.document! }, notice: undefined, coalesce: undefined })),
          invoke: {
            src: 'save',
            input: ({ context }) => ({ port: context.port, id: context.target.id, revision: context.base!.revision, document: context.saving!.document }),
            onDone: { target: 'idle', actions: assign(({ context, event }) => committed(context, context.saving!, event.output)) },
            onError: [
              {
                guard: ({ event }) => failure(event, true).kind === 'uncertain', target: 'idle',
                actions: assign(({ context, event }) => ({ uncertain: context.saving, saving: undefined, notice: { kind: 'failure', operation: 'save', failure: failure(event, true) } as EditorNotice })),
              },
              { target: 'idle', actions: assign(({ event }) => ({ saving: undefined, notice: { kind: 'failure', operation: 'save', failure: failure(event, true) } as EditorNotice })) },
            ],
          },
        },
        reconciling: {
          // Reconciliation observes. It never resends the uncertain write,
          // and the working document (any later generation) is untouched.
          invoke: {
            src: 'reconcile',
            input: ({ context }) => ({ port: context.port, id: context.target.id, base: context.base!.revision, document: context.uncertain!.document }),
            onDone: { target: 'idle', actions: assign(({ context, event }) => {
              const { kind, source } = event.output, attempt = context.uncertain!;
              if (kind === 'not_committed') return { uncertain: undefined, notice: { kind: 'not_committed' } as EditorNotice };
              if (kind === 'committed') {
                return { uncertain: undefined, base: { revision: source.revision, source, document: attempt.document }, receipt: { revision: source.revision, configurationRevision: null, generation: attempt.generation, confirmed: true }, notice: { kind: 'reconciled' } as EditorNotice };
              }
              return { uncertain: undefined, notice: { kind: 'changed_externally', revision: source.revision } as EditorNotice };
            }) },
            onError: { target: 'idle', actions: assign(({ event }) => ({ notice: { kind: 'failure', operation: 'reconcile', failure: failure(event, false) } as EditorNotice })) },
          },
        },
      },
    },
  },
});

export type WorkflowEditorActor = ActorRefFrom<typeof workflowEditorMachine>;
export type WorkflowEditorSnapshot = SnapshotFrom<typeof workflowEditorMachine>;
