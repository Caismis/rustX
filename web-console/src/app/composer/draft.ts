import type { AttachmentTarget, MethodResult, UserInputBlock, UploadReceipt } from '../../../../protocol/app-server/v57';
import type { AttachmentIntake } from '../../client/uploads';
import { editableContent } from './editor-content';

export type ComposerConversation = Pick<AttachmentTarget, 'session_id' | 'conversation_id'>;
function sameConversation(a?: ComposerConversation, b?: ComposerConversation) {
  return a === b || !!a && !!b && a.session_id === b.session_id && a.conversation_id === b.conversation_id;
}

type RestoredUpload = Extract<UserInputBlock, { type: 'upload' }>;
export type InputAcceptance = Extract<MethodResult, { type: 'inbound_accepted' }>;
export type SubmissionOutcome = 'pending' | 'unsent' | 'rejected' | 'accepted' | 'uncertain';
export interface DraftState {
  key: string; incarnation: number; revision: number; text: string;
  conversation?: ComposerConversation; restored: readonly RestoredUpload[]; supported: boolean; receiptAuthority?: string;
  selection?: { start: number; end: number; direction: 'forward' | 'backward' | 'none' };
  submission?: ComposerSubmission;
}
/** Correlates one user revision with its actual native operation. No permission,
 * native identity invention, automatic retry, or durable queue lives here. */
export class ComposerSubmission {
  outcome: SubmissionOutcome = 'pending';
  acceptance?: InputAcceptance;
  target?: AttachmentTarget;
  conversation?: ComposerConversation;
  receiptAuthority?: string;
  receipts: readonly UploadReceipt[];
  constructor(private readonly owner: ComposerDraft, readonly incarnation: number, readonly revision: number,
    readonly text: string, readonly restored: readonly RestoredUpload[], readonly intake: AttachmentIntake,
    readonly files: readonly { id: string; operation: string; receipt?: UploadReceipt }[], readonly delivery: 'send' | 'steer', target?: AttachmentTarget, receiptAuthority?: string, conversation?: ComposerConversation) {
    this.target = target; this.conversation = target ?? conversation; this.receiptAuthority = receiptAuthority;
    this.receipts = [...restored, ...files.flatMap(file => file.receipt ? [file.receipt] : [])];
  }
  /** First Submission acquires its native target/receipts before input dispatch. */
  bind(target: AttachmentTarget, receiptAuthority: string, receipts: readonly UploadReceipt[]) {
    if (this.conversation && !sameConversation(this.conversation, target)) throw new Error('Submission belongs to another Conversation.');
    this.target = target; this.conversation = target; this.receiptAuthority = receiptAuthority; this.receipts = receipts; this.owner.dispatched(this);
  }
  accepted = (acceptance?: InputAcceptance, conversation?: ComposerConversation) => this.owner.accept(this, acceptance, conversation);
  failed = (outcome: Exclude<SubmissionOutcome, 'pending' | 'accepted'>) => this.owner.fail(this, outcome);
}

/** One browser-lifetime editor owner. React only projects this revisioned state;
 * native acceptance and explicit replacement survive disposable rendering. */
export class ComposerDraft {
  private state?: DraftState;
  private incarnation = 0;
  private listeners = new Set<() => void>();
  snapshot = () => this.state;
  subscribe = (listener: () => void) => { this.listeners.add(listener); return () => { this.listeners.delete(listener); }; };
  private publish(state: DraftState) { this.state = state; for (const listener of this.listeners) listener(); }
  /** Initial content is admitted once, with a new logical editor key. Native
   * transition publication supplies that key and validates its Conversation.
   * Repeated props, refocus, history Return and Discard never re-seed this key. */
  activate(key: string, initial: readonly UserInputBlock[], receiptAuthority?: string, conversation?: ComposerConversation) {
    if (!this.state || this.state.key !== key) {
      const supported = editableContent(initial);
      this.publish({ key, incarnation: ++this.incarnation, revision: 0, conversation,
        supported, text: supported ? initial.flatMap(block => block.type === 'text' ? [block.text] : []).join('') : '',
        restored: supported ? initial.flatMap(block => block.type === 'upload' ? [block] : []) : [], receiptAuthority });
    } else {
      // Missing live attachment evidence does not invent a new Conversation.
      // A known replacement does fence old acceptance, without authoring text.
      const next = conversation ?? this.state.conversation;
      if (this.state.receiptAuthority !== receiptAuthority || !sameConversation(this.state.conversation, next)) {
        this.publish({ ...this.state, conversation: next, receiptAuthority,
          restored: [], submission: this.state.submission?.conversation && !sameConversation(this.state.submission.conversation, next) ? undefined : this.state.submission });
      }
    }
    return this.state!;
  }
  /** Native acknowledgement delivery may outlive its Composer projection.
   * Reconcile only the originating logical editor, before settling its ticket. */
  private observeConversation(key: string, conversation?: ComposerConversation) {
    const state = this.state;
    if (!state || state.key !== key || !conversation || sameConversation(state.conversation, conversation)) return;
    this.publish({ ...state, conversation, restored: [],
      submission: state.submission?.conversation && !sameConversation(state.submission.conversation, conversation) ? undefined : state.submission });
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
    if (!state || state.incarnation !== incarnation || this.blocked(state) || target && state.conversation && !sameConversation(state.conversation, target)) return;
    const submission = new ComposerSubmission(this, incarnation, state.revision, state.text, state.restored, intake,
      intake.snapshot().map(({ id, operation, receipt }) => ({ id, operation, receipt })), delivery, target, state.receiptAuthority, state.conversation);
    this.publish({ ...state, conversation: state.conversation ?? target, submission }); return submission;
  }
  blocked(state = this.state) {
    return !!state?.submission && state.submission.revision === state.revision && sameConversation(state.conversation, state.submission.conversation) && ['pending', 'uncertain'].includes(state.submission.outcome);
  }
  /** Explicit First Submission continuation may dispatch an input only after
   * its earlier preparation failed without sending. This is not a retry edge. */
  dispatched(submission: ComposerSubmission) {
    if (['unsent', 'rejected'].includes(submission.outcome)) submission.outcome = 'pending';
    if (this.state?.incarnation === submission.incarnation) this.publish({ ...this.state, conversation: this.state.conversation ?? submission.conversation });
  }
  accept(submission: ComposerSubmission, acceptance?: InputAcceptance, conversation?: ComposerConversation) {
    if (!['pending', 'uncertain'].includes(submission.outcome)) return;
    submission.outcome = 'accepted'; submission.acceptance = acceptance;
    if (this.state?.incarnation === submission.incarnation) this.observeConversation(this.state.key, conversation);
    const state = this.state;
    if (!state || state.incarnation !== submission.incarnation || !sameConversation(state.conversation, submission.conversation)) return;
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
