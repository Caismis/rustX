import type { AttachmentTarget, MethodResult, UserInputBlock, UploadReceipt } from '../../../../protocol/app-server/v46';
import type { AttachmentIntake } from '../../client/uploads';
import { editableContent } from './editor-content';

type RestoredUpload = Extract<UserInputBlock, { type: 'upload' }>;
export type InputAcceptance = Extract<MethodResult, { type: 'inbound_accepted' }>;
export type SubmissionOutcome = 'pending' | 'unsent' | 'rejected' | 'accepted' | 'uncertain';
export interface DraftState {
  key: string; incarnation: number; revision: number; text: string;
  restored: readonly RestoredUpload[]; supported: boolean; receiptAuthority?: string;
  selection?: { start: number; end: number; direction: 'forward' | 'backward' | 'none' };
  submission?: ComposerSubmission;
}
/** Correlates one user revision with its actual native operation. No permission,
 * native identity invention, automatic retry, or durable queue lives here. */
export class ComposerSubmission {
  outcome: SubmissionOutcome = 'pending';
  acceptance?: InputAcceptance;
  target?: AttachmentTarget;
  receiptAuthority?: string;
  receipts: readonly UploadReceipt[];
  constructor(private readonly owner: ComposerDraft, readonly incarnation: number, readonly revision: number,
    readonly text: string, readonly restored: readonly RestoredUpload[], readonly intake: AttachmentIntake,
    readonly files: readonly { id: string; operation: string; receipt?: UploadReceipt }[], readonly delivery: 'send' | 'steer', target?: AttachmentTarget, receiptAuthority?: string) {
    this.target = target; this.receiptAuthority = receiptAuthority;
    this.receipts = [...restored, ...files.flatMap(file => file.receipt ? [file.receipt] : [])];
  }
  /** First Submission acquires its native target/receipts before input dispatch. */
  bind(target: AttachmentTarget, receiptAuthority: string, receipts: readonly UploadReceipt[]) { this.target = target; this.receiptAuthority = receiptAuthority; this.receipts = receipts; this.owner.dispatched(this); }
  accepted = (acceptance?: InputAcceptance) => this.owner.accept(this, acceptance);
  failed = (outcome: Exclude<SubmissionOutcome, 'pending' | 'accepted'>) => this.owner.fail(this, outcome);
}

/** One browser-lifetime editor owner. React only projects this revisioned state;
 * native acceptance and explicit replacement survive disposable rendering. */
export class ComposerDraft {
  private state?: DraftState;
  private incarnation = 0;
  private initial?: readonly UserInputBlock[];
  private listeners = new Set<() => void>();
  snapshot = () => this.state;
  subscribe = (listener: () => void) => { this.listeners.add(listener); return () => { this.listeners.delete(listener); }; };
  private publish(state: DraftState) { this.state = state; for (const listener of this.listeners) listener(); }
  activate(key: string, initial: readonly UserInputBlock[], receiptAuthority?: string) {
    const supported = editableContent([...initial]);
    const content = { supported, text: supported ? initial.flatMap(block => block.type === 'text' ? [block.text] : []).join('') : '', restored: supported ? initial.flatMap(block => block.type === 'upload' ? [block] : []) : [] };
    if (!this.state || this.state.key !== key) {
      this.initial = initial; this.publish({ key, incarnation: ++this.incarnation, revision: 0, ...content, receiptAuthority });
    } else if (this.initial !== initial) {
      this.initial = initial; this.publish({ ...this.state, ...content, revision: this.state.revision + 1, receiptAuthority, selection: undefined });
    } else if (this.state.receiptAuthority !== receiptAuthority) {
      // Native retirement does not author text. It cannot transfer receipts.
      this.publish({ ...this.state, receiptAuthority, restored: [] });
    }
    return this.state!;
  }
  edit(incarnation: number, text: string) {
    if (this.state?.incarnation === incarnation) this.publish({ ...this.state, text, revision: this.state.revision + 1 });
  }
  attachmentsEdited(incarnation: number) {
    if (this.state?.incarnation === incarnation) this.publish({ ...this.state, revision: this.state.revision + 1 });
  }
  removeRestored(incarnation: number, index: number) {
    if (this.state?.incarnation === incarnation) this.publish({ ...this.state, restored: this.state.restored.filter((_, at) => at !== index), revision: this.state.revision + 1 });
  }
  select(incarnation: number, selection: DraftState['selection']) {
    // Selection is imperative editor metadata, not a render dependency. Updating
    // it must not render the Composer during unrelated transcript publication.
    if (this.state?.incarnation === incarnation) this.state.selection = selection;
  }
  replace(incarnation: number) {
    if (this.state?.incarnation !== incarnation) return false;
    this.publish({ ...this.state, incarnation: ++this.incarnation, revision: 0, text: '', restored: [], supported: true, selection: undefined, submission: undefined });
    return true;
  }
  consumeRevision(incarnation: number, revision: number) {
    if (this.state?.incarnation === incarnation && this.state.revision === revision) this.publish({ ...this.state, text: '', revision: revision + 1, selection: undefined });
  }
  submit(incarnation: number, intake: AttachmentIntake, delivery: 'send' | 'steer', target?: AttachmentTarget) {
    const state = this.state;
    if (!state || state.incarnation !== incarnation || this.blocked(state)) return;
    const submission = new ComposerSubmission(this, incarnation, state.revision, state.text, state.restored, intake,
      intake.snapshot().map(({ id, operation, receipt }) => ({ id, operation, receipt })), delivery, target, state.receiptAuthority);
    this.publish({ ...state, submission }); return submission;
  }
  blocked(state = this.state) {
    return !!state?.submission && state.submission.revision === state.revision && ['pending', 'uncertain'].includes(state.submission.outcome);
  }
  /** Explicit First Submission continuation may dispatch an input only after
   * its earlier preparation failed without sending. This is not a retry edge. */
  dispatched(submission: ComposerSubmission) {
    if (!['unsent', 'rejected'].includes(submission.outcome)) return;
    submission.outcome = 'pending';
    if (this.state?.incarnation === submission.incarnation) this.publish({ ...this.state });
  }
  accept(submission: ComposerSubmission, acceptance?: InputAcceptance) {
    if (!['pending', 'uncertain'].includes(submission.outcome)) return;
    submission.outcome = 'accepted'; submission.acceptance = acceptance;
    const state = this.state;
    if (!state || state.incarnation !== submission.incarnation) return;
    const currentRevision = state.revision === submission.revision;
    const sameReceipts = state.receiptAuthority === submission.receiptAuthority;
    // Consumption is selection-specific; later files and replacement operations
    // cannot be cleared by this acknowledgement. No native request is issued.
    if (sameReceipts) submission.intake.consume(submission.files);
    this.publish({ ...state, text: currentRevision ? '' : state.text,
      restored: sameReceipts ? state.restored.filter(receipt => !submission.restored.includes(receipt)) : state.restored,
      revision: currentRevision ? state.revision + 1 : state.revision, selection: currentRevision ? undefined : state.selection });
  }
  fail(submission: ComposerSubmission, outcome: Exclude<SubmissionOutcome, 'pending' | 'accepted'>) {
    if (submission.outcome !== 'pending') return;
    submission.outcome = outcome;
    if (this.state?.incarnation === submission.incarnation) this.publish({ ...this.state });
  }
}
