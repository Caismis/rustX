import type { ScheduleCommandReceipt, ScheduleSnapshot } from '../../../../protocol/app-server/v61';
import { sameTiming, timingDraft, type TimingDraft } from './timing';
import { hasUnrepresentableProperty } from '../../../shared/json-numbers';

export interface TaskDraft { title: string; content: string; timing: TimingDraft }
interface SavedDraft { revision: string; values: TaskDraft; unsafeArguments: boolean }
interface EditorState {
  baseline: SavedDraft; draft: TaskDraft; incoming?: SavedDraft;
  argumentsEdited: boolean;
  saving: boolean; error?: 'conflict' | 'uncertain' | 'invalid'; savedRefreshError: boolean;
}
const valuesOf = (value: ScheduleSnapshot): TaskDraft => ({
  title: value.title,
  content: value.target.type === 'conversation_inbound' ? value.target.text : JSON.stringify(value.target.arguments, null, 2),
  timing: timingDraft(value.spec),
});
const same = (a: TaskDraft, b: TaskDraft) => a.title === b.title && a.content === b.content && sameTiming(a.timing, b.timing);
const unsafeArguments = (value: ScheduleSnapshot) => value.target.type === 'workflow' && hasUnrepresentableProperty(value.target, 'arguments');

/** UI intent outlives a generation-keyed Workbench. Native revisions still
 * control rebasing and saving; reconnect never admits a mutation. */
export class TaskEditor {
  private listeners = new Set<() => void>();
  private state: EditorState;
  private submitted?: { commandId: string; draft: TaskDraft };
  private readonly workflow: boolean;
  constructor(value: ScheduleSnapshot, private release: () => void) {
    this.workflow = value.target.type === 'workflow';
    const draft = valuesOf(value);
    this.state = { baseline: { revision: value.revision, values: draft, unsafeArguments: unsafeArguments(value) }, draft, argumentsEdited: false, saving: false, savedRefreshError: false };
  }
  getSnapshot = () => this.state;
  subscribe = (listener: () => void) => {
    this.listeners.add(listener);
    return () => { this.listeners.delete(listener); this.releaseUnused(); };
  };
  private releaseUnused() {
    queueMicrotask(() => { if (!this.listeners.size && !this.state.saving && !this.dirty) this.release(); });
  }
  private publish(patch: Partial<EditorState>) {
    this.state = { ...this.state, ...patch };
    for (const listener of this.listeners) listener();
    this.releaseUnused();
  }
  get dirty() { return this.state.argumentsEdited || !same(this.state.draft, this.state.baseline.values); }
  observe(value: ScheduleSnapshot) {
    if (BigInt(value.revision) <= BigInt(this.state.baseline.revision)) return;
    if (this.state.incoming && BigInt(value.revision) < BigInt(this.state.incoming.revision)) return;
    const saved = { revision: value.revision, values: valuesOf(value), unsafeArguments: unsafeArguments(value) };
    if (!this.dirty && !this.state.saving || same(this.state.draft, saved.values) && !(this.state.argumentsEdited && saved.unsafeArguments)) this.publish({ baseline: saved, draft: saved.values, argumentsEdited: false, incoming: undefined, error: undefined, savedRefreshError: false });
    else this.publish({ incoming: saved });
  }
  edit(draft: TaskDraft) {
    if (this.state.saving) return;
    const argumentsEdited = draft.content === this.state.draft.content ? this.state.argumentsEdited
      : this.workflow && (this.state.baseline.unsafeArguments || draft.content !== this.state.baseline.values.content);
    this.publish({ draft, argumentsEdited });
  }
  discard = () => {
    const baseline = this.state.incoming ?? this.state.baseline;
    this.publish({ baseline, draft: baseline.values, argumentsEdited: false, incoming: undefined, error: undefined });
  };
  rebase = () => {
    if (!this.state.incoming) return;
    // Authored arguments survive successive native revisions, even when an
    // unsafe intermediate value has the same rounded display text. Only
    // untouched arguments follow the new authoritative value.
    const untouched = this.workflow && !this.state.argumentsEdited;
    const draft = untouched ? { ...this.state.draft, content: this.state.incoming.values.content } : this.state.draft;
    const argumentsEdited = this.state.argumentsEdited && (this.state.incoming.unsafeArguments || draft.content !== this.state.incoming.values.content);
    this.publish({ baseline: this.state.incoming, draft, argumentsEdited, incoming: undefined, error: undefined });
  };
  beginSave() {
    if (this.state.saving || this.state.incoming || !this.dirty) return;
    const ticket = { baseline: this.state.baseline, draft: this.state.draft, argumentsEdited: this.state.argumentsEdited };
    this.publish({ saving: true, error: undefined, savedRefreshError: false });
    return ticket;
  }
  applied(receipt: ScheduleCommandReceipt, draft: TaskDraft) {
    this.submitted = undefined;
    if (BigInt(receipt.revision) < BigInt(this.state.baseline.revision)) { this.publish({ saving: false, error: undefined }); return; }
    const unchanged = !this.dirty || same(this.state.draft, draft);
    const values = { ...draft, title: draft.title.trim(), content: this.workflow ? JSON.stringify(JSON.parse(draft.content), null, 2) : draft.content, timing: timingDraft(receipt.spec, draft.timing.zone) };
    const baseline = { revision: receipt.revision, values, unsafeArguments: this.state.baseline.unsafeArguments && draft.content === this.state.baseline.values.content };
    const incoming = this.state.incoming && BigInt(this.state.incoming.revision) > BigInt(receipt.revision) ? this.state.incoming : undefined;
    const argumentsEdited = !unchanged && this.workflow && (this.state.draft.content !== values.content || baseline.unsafeArguments && this.state.argumentsEdited);
    this.publish({ baseline, draft: unchanged ? values : this.state.draft, argumentsEdited, incoming, saving: false, error: undefined, savedRefreshError: false });
  }
  dispatched(commandId: string) { if (this.state.saving) this.submitted = { commandId, draft: this.state.draft }; }
  recovered(receipt: ScheduleCommandReceipt, commandId: string) { if (this.submitted?.commandId === commandId) this.applied(receipt, this.submitted.draft); }
  failed(error: EditorState['error']) { if (error !== 'uncertain') this.submitted = undefined; this.publish({ saving: false, error }); }
  refreshFailed() { this.publish({ savedRefreshError: true }); }
}
