import { isPlainMessageKind } from '../../../../protocol/app-server/message-kind';
import type { ConversationReadCut1, PendingInboundRef, UserContentBlock, UserInputBlock } from '../../../../protocol/app-server/v80';
import { contentPreview, inboundOrigin, type InboundRow } from '../../bindings/composer-context';
import type { InboundControlOutcome, Submission } from '../../client/contracts';
import type { UploadAttachmentKey, UploadedAttachmentMetadata } from '../../client/session-uploads';
import { message } from '../../locale/translation';
import { useTranslation, useNotice } from '../../locale/react';
/* Copyright (c) 2026 DeepSeek. MIT. Source-derived; see PROVENANCE.md. */
// Adapted from DeepSeek Harness ui-conversation QueueDock: one row renders
// directly, several rows default to a collapsible count header, sending and
// accepted submissions stay visibly provisional until exact native readback,
// and human rows expose CAS-bound Edit / Remove / Steer controls.
import { useContext, useEffect, useId, useLayoutEffect, useRef, useState } from 'react';
import { FileTypeIcon } from '../../presentation/primitives/FileTypeIcon';
import { writeClipboard } from '../../presentation/primitives/clipboard';
import { fileSizeText } from '../../presentation/primitives/file-size';
import {
 IconCheckOutline14, IconChevronDownOutline14, IconChevronUpOutline14, IconCloseOutline16, IconEditOutline16, IconQueueOutline14, IconSendOutline14, IconTrashOutline16,
} from '../../presentation/primitives/icons';
import { Tooltip } from '../../presentation/primitives/Tooltip';
import { ArtifactContext } from '../components/Artifact';
import { Textarea } from '@/components/ui/textarea';

type OperationKind = 'edit' | 'remove' | 'steer';

type QueueAttachment =
 | { readonly type: 'image'; readonly artifactId?: string; readonly file?: File; readonly alt?: string | null }
 | { readonly type: 'file'; readonly name: string; readonly bytes?: number };

const QueueGlyph = () => <span className="grid flex-none place-items-center [color:var(--dsw-alias-label-tertiary)]" aria-hidden><IconQueueOutline14 /></span>;

function queueAttachments(
 content: readonly UserContentBlock[],
 attachmentMetadata?: (key: UploadAttachmentKey) => UploadedAttachmentMetadata | undefined,
): QueueAttachment[] {
 const items: QueueAttachment[] = [];
 for (const block of content) {
 if (block.type === 'image' && typeof block.artifact_id === 'string' && block.artifact_id) {
 items.push({ type: 'image', artifactId: block.artifact_id, alt: block.alt });
 } else if (block.type === 'file' && typeof block.artifact_id === 'string' && block.artifact_id) {
 items.push({ type: 'file', name: block.name ?? block.artifact_id });
 } else if (block.type === 'uploaded_file' && typeof block.name === 'string' && block.name) {
 const meta = attachmentMetadata?.({ file: { batch_id: block.batch_id, name: block.name } });
 if (meta?.imageFile) items.push({ type: 'image', file: meta.imageFile, alt: meta.name });
 else items.push({ type: 'file', name: meta?.name ?? block.name, bytes: meta?.bytes });
 }
 }
 return items;
}

function submissionAttachments(
 content: readonly UserInputBlock[],
 attachmentMetadata?: (key: UploadAttachmentKey) => UploadedAttachmentMetadata | undefined,
): { attachments: QueueAttachment[]; previewBlocks: UserInputBlock[] } {
 const attachments: QueueAttachment[] = [];
 const previewBlocks: UserInputBlock[] = [];
 for (const block of content) {
 if (block.type === 'upload') {
 const meta = attachmentMetadata?.({ receipt: { session_id: block.session_id, batch_id: block.batch_id, token: block.token } });
 if (meta) {
 if (meta.imageFile) attachments.push({ type: 'image', file: meta.imageFile, alt: meta.name });
 else attachments.push({ type: 'file', name: meta.name, bytes: meta.bytes });
 continue;
 }
 }
 previewBlocks.push(block);
 }
 return { attachments, previewBlocks };
}

function rowTextPreview(content: readonly UserContentBlock[]): string {
 return content.flatMap(block => block.type === 'text' ? [block.text] : []).join(' ').replace(/\s+/g, ' ').trim();
}

function QueueFile({ name, bytes, label }: { name: string; bytes?: number; label: string }) {
 return (
 <span className="inline-flex flex-[0_1_180px] items-center gap-[4px] min-w-[74px] h-[24px] px-[6px] py-0 overflow-hidden [border:0.5px_solid_var(--dsw-alias-border-l1)] rounded-[var(--dsw-radius-sm)] bg-[var(--dsw-alias-bg-base)] box-border" aria-label={label} title={name}>
 <span className="inline-flex flex-none w-[16px] h-[16px]" aria-hidden><FileTypeIcon path={name} size={16} /></span>
 <span className="min-w-0 overflow-hidden [color:var(--dsw-alias-label-primary-dimmed)] [font:var(--dsw-font-xs-13)] text-ellipsis whitespace-nowrap">{name}</span>
 {bytes !== undefined && <span className="flex-none [color:var(--dsw-alias-label-tertiary)] text-[10px] whitespace-nowrap">{fileSizeText(bytes)}</span>}
 </span>
 );
}

function QueueThumb({ artifactId, file, label }: {
 artifactId?: string;
 file?: File;
 label: string;
}) {
 const resources = useContext(ArtifactContext);
 const [url, setUrl] = useState<string | null>(null);
 useEffect(() => {
 if (file) {
 const objectUrl = URL.createObjectURL(file);
 setUrl(objectUrl);
 return () => {
 URL.revokeObjectURL(objectUrl);
 };
 }
 if (!artifactId || !resources) { setUrl(null); return; }
 let alive = true;
 let owned: string | undefined;
 setUrl(null);
 void resources.read(artifactId).then(
 resolved => {
 if (!alive) {
 resources.release(resolved);
 return;
 }
 owned = resolved;
 setUrl(resolved);
 },
 () => { /* placeholder retained; the durable transcript surfaces read errors */ },
 );
 return () => {
 alive = false;
 if (owned) resources.release(owned);
 };
 }, [artifactId, file, resources]);
 return url === null
 ? <span className="flex-none w-[24px] h-[24px] [border:0.5px_solid_var(--dsw-alias-border-l1)] rounded-[var(--dsw-radius-xs)] bg-[var(--dsw-alias-bg-base)] object-cover" aria-hidden />
 : <img className="flex-none w-[24px] h-[24px] [border:0.5px_solid_var(--dsw-alias-border-l1)] rounded-[var(--dsw-radius-xs)] bg-[var(--dsw-alias-bg-base)] object-cover" src={url} alt={label} />;
}

export function QueueDock({ rows, submissions, running, disabled = false, edit, remove, steer, attachmentMetadata, observation, readCut }: {
 rows: readonly InboundRow[];
 /** Sending or accepted submissions; settled by exact MessageId or sendingInputId. */
 submissions: readonly Submission[];
 running: boolean;
 disabled?: boolean;
 observation?: object;
 readCut?: ConversationReadCut1;
 edit?: (expected: PendingInboundRef, text: string) => Promise<InboundControlOutcome>;
 remove?: (expected: PendingInboundRef) => Promise<InboundControlOutcome>;
 steer?: (expected: PendingInboundRef) => Promise<InboundControlOutcome>;
 attachmentMetadata?: (key: UploadAttachmentKey) => UploadedAttachmentMetadata | undefined;
}) {
 const tx = useTranslation();
 const [collapsed, setCollapsed] = useState(true);
 const listId = useId();
 const alive = useRef(true);
 const latestObservation = useRef(observation);
 latestObservation.current = observation;
 useEffect(() => { alive.current = true; return () => { alive.current = false; }; }, []);
 const [draft, setDraft] = useState<{ expected: PendingInboundRef; messageId: string; text: string; notPending?: boolean }>();
 const [operation, setOperation] = useState<{ status: 'pending' | 'uncertain' | 'readback'; observation?: object; editReceipt?: ConversationReadCut1; returnSequence?: string }>();
 const [copyStatus, setCopyStatus] = useState<'copied' | 'failed'>();
 const [notice, setNotice] = useNotice();
 const editButtons = useRef(new Map<string, HTMLButtonElement>());
 const editorNode = useRef<HTMLTextAreaElement>(null);
 const returnFocus = useRef<string | undefined>(undefined);
 const copySequence = useRef(0);
 const clearCopyState = () => {
 copySequence.current += 1;
 setCopyStatus(undefined);
 };
 useEffect(() => {
 if (!operation || operation.status === 'pending') return;
 const receipt = operation.editReceipt;
 if (receipt) {
 if (!readCut || receipt.conversation_id !== readCut.conversation_id
 || BigInt(readCut.mutation_revision) < BigInt(receipt.mutation_revision)) return;
 if (operation.returnSequence) returnFocus.current = operation.returnSequence;
 copySequence.current += 1;
 setCopyStatus(undefined);
 setDraft(undefined);
 } else if (observation === operation.observation) {
 return;
 }
 setOperation(undefined);
 setNotice('');
 }, [observation, operation, readCut, setNotice]);
 useEffect(() => {
 if (draft || !returnFocus.current) return;
 const sequence = returnFocus.current;
 returnFocus.current = undefined;
 editButtons.current.get(sequence)?.focus({ preventScroll: true });
 }, [draft]);
 const currentDraft = draft && rows.find(row => row.sequence === draft.expected.sequence && row.message.id === draft.messageId);
 const detachedDraft = !!draft && (!currentDraft || !!draft.notPending);
 const staleDraft = !!draft && (detachedDraft || !currentDraft || !!currentDraft.deferred || currentDraft.revision !== draft.expected.revision);
 const pendingOperation = operation?.status === 'pending';
 const locked = disabled || !!operation;
 const expected = (row: InboundRow): PendingInboundRef => ({ sequence: row.sequence, revision: row.revision });
 const failureNotice: Record<OperationKind, string> = {
 edit: tx('agent:queue-dock.edit-failed'),
 remove: tx('agent:queue-dock.remove-failed'),
 steer: tx('agent:queue-dock.steer-failed'),
 };
 const apply = async (work: () => Promise<InboundControlOutcome>, kind: OperationKind) => {
 if (locked) return;
 const editing = kind === 'edit';
 const sequence = draft?.expected.sequence;
 setOperation({ status: 'pending', observation }); setNotice('');
 let result: InboundControlOutcome;
 try {
 result = await work();
 } catch {
 if (!alive.current) return;
 setOperation(undefined);
 setNotice(failureNotice[kind]);
 return;
 }
 if (!alive.current) return;
 if (result.status === 'obsolete') {
 setOperation(undefined);
 return;
 }
 if (result.status === 'uncertain') {
 setOperation({ status: 'uncertain', observation: latestObservation.current });
 setNotice(message('agent:copy.outcome-uncertain-do-not-resend-reconnect-to-read-the-authoritative-queue'));
 return;
 }
 const editReceipt = editing && result.status === 'known' && result.outcome.status === 'applied' ? result.outcome.read_cut : undefined;
 setOperation(result.observed ? undefined : {
 status: 'readback',
 observation: latestObservation.current,
 editReceipt,
 returnSequence: editReceipt ? sequence : undefined,
 });
 if (result.status === 'rejected') { setNotice(result.reason); return; }
 if (result.outcome.status === 'applied') {
 if (editing && result.observed) {
 if (sequence) returnFocus.current = sequence;
 clearCopyState();
 setDraft(undefined);
 }
 setNotice(result.observed ? '' : message('agent:copy.committed-awaiting-authoritative-readback'));
 } else {
 if (editing && result.outcome.status === 'not_pending') {
 setDraft(current => current ? { ...current, notPending: true } : current);
 return;
 }
 setNotice(result.outcome.status === 'conflict' ? message('agent:copy.this-item-changed-your-draft-is-preserved-compare-it-with-the-current-row')
 : result.outcome.status === 'not_pending' ? failureNotice[kind]
 : message('agent:copy.this-item-cannot-be-edited-with-the-text-editor'));
 }
 };
 // Settle provisional submissions when an authoritative native row matches
 // either their server MessageId or their sending input_id.
 const admitted = new Set(rows.map(row => row.message.id));
 const admittedInputIds = new Set(
 rows.flatMap(row => row.input_id ? [row.input_id] : []),
 );
 const echoes = submissions.filter(item =>
 !admitted.has(item.messageId)
 && (!item.sendingInputId || !admittedInputIds.has(item.sendingInputId)),
 );
 const count = rows.length + echoes.length;
 useEffect(() => {
 if (count === 0) setCollapsed(true);
 }, [count]);
 useLayoutEffect(() => {
 const node = editorNode.current;
 if (!node) return;
 const fit = () => {
 node.style.height = 'auto';
 // scrollHeight excludes the border that the border-box height includes.
 node.style.height = `${node.scrollHeight + node.offsetHeight - node.clientHeight}px`;
 };
 fit();
 const observer = new ResizeObserver(fit);
 observer.observe(node);
 return () => observer.disconnect();
 }, [draft?.text, detachedDraft, staleDraft]);
 if (count === 0 && !draft) return null;
 const listed = count === 1 || !collapsed || !!draft || !!operation;
 const cancelEdit = () => {
 if (draft) returnFocus.current = draft.expected.sequence;
 clearCopyState();
 setDraft(undefined);
 setNotice('');
 };
 const discardDraft = () => {
 if (pendingOperation) return;
 clearCopyState();
 setDraft(undefined);
 if (!operation) setNotice('');
 };
 const copyDraft = () => {
 if (!draft?.text) return;
 const token = ++copySequence.current;
 setCopyStatus(undefined);
 void writeClipboard(draft.text).then(ok => {
 if (alive.current && copySequence.current === token) setCopyStatus(ok ? 'copied' : 'failed');
 });
 };
 const saveDisabled = !draft || locked || staleDraft || !draft.text.trim() || !edit;
 const discardDisabled = pendingOperation;
 const editor = draft ? <div className="flex items-center flex-1 gap-[8px] flex-wrap min-w-0 p-0 text-[13px]">
 <div className="flex-1 min-w-0">
 <Textarea unstyled ref={editorNode} className="box-border block w-full min-w-0 min-h-[28px] max-h-[127px] m-0 px-[8px] py-[3px] [border:0.5px_solid_var(--dsw-alias-border-l4)] rounded-[var(--dsw-radius-sm)] outline-none bg-[var(--dsw-alias-bg-base)] [color:var(--dsw-alias-label-primary)] [font:var(--dsw-font-xs-13)] [font-family:Inter,var(--dsw-font-family)] overflow-y-auto resize-none focus:[border-color:var(--dsw-alias-state-business-primary)]" autoFocus rows={1} aria-label={tx('agent:queue-dock.edit-queued-message')} value={draft.text} disabled={pendingOperation || (!detachedDraft && locked)} readOnly={!!operation?.editReceipt}
 onChange={event => {
 const text = event.currentTarget.value;
 clearCopyState();
 setDraft(current => current ? { ...current, text } : current);
 }}
 onKeyDown={event => {
 if (event.key === 'Escape' && !detachedDraft && !locked) {
 event.preventDefault();
 cancelEdit();
 }
 if (event.key === 'Enter' && !event.shiftKey && !event.nativeEvent.isComposing && event.nativeEvent.keyCode !== 229 && !saveDisabled && edit) {
 event.preventDefault();
 void apply(() => edit(draft.expected, draft.text), 'edit');
 }
 }} />
 </div>
 <div className="flex flex-none items-center gap-[8px] flex-wrap">
 <Tooltip label={tx('agent:queue-dock.save')} side="bottom" delayMs={500} disabled={saveDisabled}>
 <button aria-label={tx('agent:queue-dock.save')} type="button" className="min-h-[32px] px-[8px] py-[4px] border-0 rounded-[var(--dsw-radius-sm)] bg-transparent [color:var(--dsw-alias-label-primary)] text-[12px] cursor-pointer [&[aria-label]]:grid [&[aria-label]]:flex-none [&[aria-label]]:place-items-center [&[aria-label]]:w-[28px] [&[aria-label]]:h-[28px] [&[aria-label]]:min-h-0 [&[aria-label]]:p-0 [&[aria-label]]:rounded-full [&[aria-label]]:[corner-shape:round]! [&[aria-label]]:[color:var(--dsw-alias-label-tertiary)] hover:not-disabled:bg-[var(--dsw-alias-interactive-bg-hover)] disabled:opacity-45 disabled:cursor-default focus-visible:[outline:var(--dsw-focus-ring-width)_solid_var(--dsw-focus-ring-color,var(--dsw-alias-state-business-primary))] focus-visible:-outline-offset-2" disabled={saveDisabled} onClick={() => { if (edit && !saveDisabled) void apply(() => edit(draft.expected, draft.text), 'edit'); }}><IconCheckOutline14 size={14} /></button>
 </Tooltip>
 {staleDraft ? <button type="button" className="min-h-[32px] px-[8px] py-[4px] border-0 rounded-[var(--dsw-radius-sm)] bg-transparent [color:var(--dsw-alias-label-primary)] text-[12px] cursor-pointer [&[aria-label]]:grid [&[aria-label]]:flex-none [&[aria-label]]:place-items-center [&[aria-label]]:w-[28px] [&[aria-label]]:h-[28px] [&[aria-label]]:min-h-0 [&[aria-label]]:p-0 [&[aria-label]]:rounded-full [&[aria-label]]:[corner-shape:round]! [&[aria-label]]:[color:var(--dsw-alias-label-tertiary)] hover:not-disabled:bg-[var(--dsw-alias-interactive-bg-hover)] disabled:opacity-45 disabled:cursor-default focus-visible:[outline:var(--dsw-focus-ring-width)_solid_var(--dsw-focus-ring-color,var(--dsw-alias-state-business-primary))] focus-visible:-outline-offset-2" disabled={!draft.text} onClick={copyDraft}>{tx('agent:queue-dock.copy-draft')}</button> : null}
 {detachedDraft ? (
 <button type="button" className="min-h-[32px] px-[8px] py-[4px] border-0 rounded-[var(--dsw-radius-sm)] bg-transparent [color:var(--dsw-alias-label-primary)] text-[12px] cursor-pointer [&[aria-label]]:grid [&[aria-label]]:flex-none [&[aria-label]]:place-items-center [&[aria-label]]:w-[28px] [&[aria-label]]:h-[28px] [&[aria-label]]:min-h-0 [&[aria-label]]:p-0 [&[aria-label]]:rounded-full [&[aria-label]]:[corner-shape:round]! [&[aria-label]]:[color:var(--dsw-alias-label-tertiary)] hover:not-disabled:bg-[var(--dsw-alias-interactive-bg-hover)] disabled:opacity-45 disabled:cursor-default focus-visible:[outline:var(--dsw-focus-ring-width)_solid_var(--dsw-focus-ring-color,var(--dsw-alias-state-business-primary))] focus-visible:-outline-offset-2" disabled={discardDisabled} onClick={discardDraft}>{tx('agent:queue-dock.discard-draft')}</button>
 ) : (
 <Tooltip label={tx('agent:queue-dock.cancel-edit')} side="bottom" delayMs={500} disabled={locked}>
 <button aria-label={tx('agent:queue-dock.cancel-edit')} type="button" className="min-h-[32px] px-[8px] py-[4px] border-0 rounded-[var(--dsw-radius-sm)] bg-transparent [color:var(--dsw-alias-label-primary)] text-[12px] cursor-pointer [&[aria-label]]:grid [&[aria-label]]:flex-none [&[aria-label]]:place-items-center [&[aria-label]]:w-[28px] [&[aria-label]]:h-[28px] [&[aria-label]]:min-h-0 [&[aria-label]]:p-0 [&[aria-label]]:rounded-full [&[aria-label]]:[corner-shape:round]! [&[aria-label]]:[color:var(--dsw-alias-label-tertiary)] hover:not-disabled:bg-[var(--dsw-alias-interactive-bg-hover)] disabled:opacity-45 disabled:cursor-default focus-visible:[outline:var(--dsw-focus-ring-width)_solid_var(--dsw-focus-ring-color,var(--dsw-alias-state-business-primary))] focus-visible:-outline-offset-2" disabled={locked} onClick={cancelEdit}><IconCloseOutline16 size={14} /></button>
 </Tooltip>
 )}
 {!detachedDraft && staleDraft && currentDraft && !currentDraft.deferred ? (
 <button type="button" className="min-h-[32px] px-[8px] py-[4px] border-0 rounded-[var(--dsw-radius-sm)] bg-transparent [color:var(--dsw-alias-label-primary)] text-[12px] cursor-pointer [&[aria-label]]:grid [&[aria-label]]:flex-none [&[aria-label]]:place-items-center [&[aria-label]]:w-[28px] [&[aria-label]]:h-[28px] [&[aria-label]]:min-h-0 [&[aria-label]]:p-0 [&[aria-label]]:rounded-full [&[aria-label]]:[corner-shape:round]! [&[aria-label]]:[color:var(--dsw-alias-label-tertiary)] hover:not-disabled:bg-[var(--dsw-alias-interactive-bg-hover)] disabled:opacity-45 disabled:cursor-default focus-visible:[outline:var(--dsw-focus-ring-width)_solid_var(--dsw-focus-ring-color,var(--dsw-alias-state-business-primary))] focus-visible:-outline-offset-2" disabled={locked} onClick={() => setDraft(current => current ? { ...current, expected: expected(currentDraft), messageId: currentDraft.message.id, notPending: false } : current)}>{tx('agent:queue-dock.use-latest-version')}</button>
 ) : null}
 </div>
 {staleDraft ? (
 <p className="flex-[1_0_100%] m-0 [color:var(--dsw-alias-label-tertiary)] [font:var(--dsw-font-xs-13)]" role="status">{detachedDraft ? tx('agent:queue-dock.this-message-is-no-longer-queued') : tx('agent:queue-dock.this-queued-message-changed-review-the-latest-version-before-sav')}</p>
 ) : null}
 {copyStatus ? (
 <p className="flex-[1_0_100%] m-0 [color:var(--dsw-alias-label-tertiary)] [font:var(--dsw-font-xs-13)]" role="status">{tx(copyStatus === 'copied' ? 'agent:queue-dock.draft-copied' : 'agent:queue-dock.copy-draft-failed')}</p>
 ) : null}
 </div> : null;
 return <section className="box-border flex-none mt-0 mx-auto mb-[calc(0px-var(--dsh-composer-stack-gap)-3px)] w-[calc(100%-2*var(--dsh-composer-side-clearance)-4*var(--dsh-composer-dock-inset))] max-w-[calc(var(--dsh-composer-card-max-width)-4*var(--dsh-composer-dock-inset))]" aria-label={tx('agent:queue-dock.queue')} aria-busy={pendingOperation} data-queue-dock="">
 <div className="overflow-hidden w-full px-0 py-[2px] [border:0.5px_solid_var(--dsw-alias-border-l1)] border-b-0! rounded-t-[var(--dsw-radius-lg)] rounded-b-none bg-[var(--dsw-specific-menu)] [backdrop-filter:var(--dsw-menu-backdrop-filter)] [--dsh-scrollbar-thumb:var(--dsw-alias-scrollbar-bg-l2)] [--dsh-scrollbar-thumb-hover:var(--dsw-alias-scrollbar-hover-l2)]">
 {count > 1 ? <button type="button" className="box-border flex items-center gap-[10px] w-full h-[36px] px-[12px] py-[4px] border-none rounded-[var(--dsw-radius-md)] bg-transparent [color:var(--dsw-alias-label-primary)] text-left cursor-pointer focus-visible:[outline:var(--dsw-focus-ring-width)_solid_var(--dsw-focus-ring-color,var(--dsw-alias-state-business-primary))] focus-visible:-outline-offset-2 disabled:cursor-default" aria-expanded={listed} disabled={!!draft || !!operation} aria-controls={listId} onClick={() => setCollapsed(value => !value)}>
 <QueueGlyph /><span className="flex-auto min-w-0 [font-family:Inter,var(--dsw-font-family)] text-[13px] font-medium leading-[24px]">{count} {tx('agent:queue-dock.queued')}</span>
 {/* A collapsed header keeps sending/accepted provisional submissions visible. */}
 {!listed && echoes.length > 0 ? <span className="flex-none [color:var(--dsw-alias-label-tertiary)] [font:var(--dsw-font-xs-13)] whitespace-nowrap" role="status">{tx('agent:submission.sending')}</span> : null}
 <span className="grid flex-none place-items-center [color:var(--dsw-alias-label-tertiary)] w-[14px] h-[14px]" aria-hidden>{listed ? <IconChevronDownOutline14 /> : <IconChevronUpOutline14 />}</span>
 </button> : null}
 {count > 0 ? <ul id={listId} className="max-h-[180px] m-0 p-0 overflow-y-auto list-none" hidden={!listed}>
 {listed && rows.map(row => {
 const origin = inboundOrigin(tx, row.message);
 const attachments = queueAttachments(row.message.content, attachmentMetadata);
 const previewText = attachments.length > 0 ? rowTextPreview(row.message.content) : contentPreview(tx, row.message.content);
 const editableText = row.message.content.length && row.message.content.every(block => block.type === 'text')
 ? row.message.content.map(block => block.text).join('') : undefined;
 const editBlockedReason = row.deferred
 ? tx('common:activity.loading')
 : editableText === undefined
 ? tx('agent:queue-dock.text-only-editing')
 : draft
 ? tx('agent:queue-dock.finish-or-discard-draft')
 : undefined;
 const editDisabled = locked || !edit || !!editBlockedReason;
 const removeDisabled = locked || !remove;
 const steerDisabled = locked || !steer || !running;
 return <li key={`inbound:${row.sequence}`} className="qd-row box-border flex items-center gap-[10px] w-full min-h-[36px] pt-[4px] pr-[5px] pb-[4px] pl-[12px] rounded-[var(--dsw-radius-md)] [.qd-row+&]:shadow-[inset_0_1px_0_var(--dsw-alias-border-l1)]" data-inbound-sequence={row.sequence} data-message-id={row.message.id}>
 {count === 1 ? <QueueGlyph /> : null}
 {currentDraft === row && !staleDraft ? editor : <>
 {attachments.length > 0 ? (
 <span className="flex flex-none gap-[4px] min-w-0 max-w-[55%] overflow-hidden [.qd-pending_&]:shrink">
 {attachments.map((item, index) => item.type === 'image'
 ? <QueueThumb key={`${item.artifactId ?? item.file?.name ?? ''}:${index}`} artifactId={item.artifactId} file={item.file} label={item.alt || tx('agent:queue-dock.image')} />
 : <QueueFile key={`${item.name}:${index}`} name={item.name} bytes={item.bytes} label={tx('agent:queue-dock.file', { name: item.name })} />)}
 </span>
 ) : null}
 <span className="flex-auto min-w-0 overflow-hidden [font:var(--dsw-font-xs-13)] [font-family:Inter,var(--dsw-font-family)] [color:var(--dsw-alias-label-primary-dimmed)] text-ellipsis whitespace-nowrap">{previewText}</span>
 {origin ? <span className="flex-none [color:var(--dsw-alias-label-tertiary)] [font:var(--dsw-font-xs-13)] whitespace-nowrap">{origin}</span> : null}
 {row.message.source === 'human' && isPlainMessageKind(row.message.kind) ? <div className="flex flex-none items-center gap-[8px] flex-wrap">
 <Tooltip label={tx('agent:queue-dock.edit')} side="bottom" delayMs={500} disabled={editDisabled}>
 <button
 ref={element => {
 if (element) editButtons.current.set(row.sequence, element);
 else editButtons.current.delete(row.sequence);
 }}
 aria-label={tx('agent:queue-dock.edit')}
 type="button"
 className="min-h-[32px] px-[8px] py-[4px] border-0 rounded-[var(--dsw-radius-sm)] bg-transparent [color:var(--dsw-alias-label-primary)] text-[12px] cursor-pointer [&[aria-label]]:grid [&[aria-label]]:flex-none [&[aria-label]]:place-items-center [&[aria-label]]:w-[28px] [&[aria-label]]:h-[28px] [&[aria-label]]:min-h-0 [&[aria-label]]:p-0 [&[aria-label]]:rounded-full [&[aria-label]]:[corner-shape:round]! [&[aria-label]]:[color:var(--dsw-alias-label-tertiary)] hover:not-disabled:bg-[var(--dsw-alias-interactive-bg-hover)] disabled:opacity-45 disabled:cursor-default focus-visible:[outline:var(--dsw-focus-ring-width)_solid_var(--dsw-focus-ring-color,var(--dsw-alias-state-business-primary))] focus-visible:-outline-offset-2"
 disabled={editDisabled}
 title={editBlockedReason}
 onClick={() => {
 clearCopyState();
 setDraft({ expected: expected(row), messageId: row.message.id, text: editableText ?? '' });
 setNotice('');
 }}
 ><IconEditOutline16 size={14} /></button>
 </Tooltip>
 <Tooltip label={tx('agent:queue-dock.remove')} side="bottom" delayMs={500} disabled={removeDisabled}>
 <button aria-label={tx('agent:queue-dock.remove')} type="button" className="min-h-[32px] px-[8px] py-[4px] border-0 rounded-[var(--dsw-radius-sm)] bg-transparent [color:var(--dsw-alias-label-primary)] text-[12px] cursor-pointer [&[aria-label]]:grid [&[aria-label]]:flex-none [&[aria-label]]:place-items-center [&[aria-label]]:w-[28px] [&[aria-label]]:h-[28px] [&[aria-label]]:min-h-0 [&[aria-label]]:p-0 [&[aria-label]]:rounded-full [&[aria-label]]:[corner-shape:round]! [&[aria-label]]:[color:var(--dsw-alias-label-tertiary)] hover:not-disabled:bg-[var(--dsw-alias-interactive-bg-hover)] disabled:opacity-45 disabled:cursor-default focus-visible:[outline:var(--dsw-focus-ring-width)_solid_var(--dsw-focus-ring-color,var(--dsw-alias-state-business-primary))] focus-visible:-outline-offset-2" disabled={removeDisabled} onClick={() => { if (remove) void apply(() => remove(expected(row)), 'remove'); }}><IconTrashOutline16 size={14} /></button>
 </Tooltip>
 <Tooltip label={tx('agent:queue-dock.steer')} side="bottom" delayMs={500} disabled={steerDisabled}>
 <button aria-label={tx('agent:queue-dock.steer')} type="button" className="min-h-[32px] px-[8px] py-[4px] border-0 rounded-[var(--dsw-radius-sm)] bg-transparent [color:var(--dsw-alias-label-primary)] text-[12px] cursor-pointer [&[aria-label]]:grid [&[aria-label]]:flex-none [&[aria-label]]:place-items-center [&[aria-label]]:w-[28px] [&[aria-label]]:h-[28px] [&[aria-label]]:min-h-0 [&[aria-label]]:p-0 [&[aria-label]]:rounded-full [&[aria-label]]:[corner-shape:round]! [&[aria-label]]:[color:var(--dsw-alias-label-tertiary)] hover:not-disabled:bg-[var(--dsw-alias-interactive-bg-hover)] disabled:opacity-45 disabled:cursor-default focus-visible:[outline:var(--dsw-focus-ring-width)_solid_var(--dsw-focus-ring-color,var(--dsw-alias-state-business-primary))] focus-visible:-outline-offset-2" disabled={steerDisabled} title={running ? undefined : tx('agent:queue-dock.steer-unavailable')} onClick={() => { if (steer) void apply(() => steer(expected(row)), 'steer'); }}><IconSendOutline14 size={14} /></button>
 </Tooltip>
 </div> : null}
 </>}
 {currentDraft === row && staleDraft && !detachedDraft ? editor : null}
 </li>;
 })}
 {listed && echoes.map(submission => {
 const isSending = Boolean(submission.sendingInputId);
 const { attachments, previewBlocks } = submissionAttachments(submission.content ?? [], attachmentMetadata);
 const previewText = previewBlocks.length > 0 ? contentPreview(tx, previewBlocks) : '';
 return <li
 key={isSending ? `sending:${submission.sendingInputId}` : `accepted:${submission.messageId}`}
 className={`qd-row box-border flex items-center gap-[10px] w-full min-h-[36px] pt-[4px] pr-[5px] pb-[4px] pl-[12px] rounded-[var(--dsw-radius-md)] [.qd-row+&]:shadow-[inset_0_1px_0_var(--dsw-alias-border-l1)] qd-pending`}
 data-submission-echo=""
 data-sending-input-id={submission.sendingInputId}
 data-accepted-message-id={isSending ? undefined : submission.messageId}
 >
 {count === 1 ? <QueueGlyph /> : null}
 {attachments.length > 0 ? (
 <span className="flex flex-none gap-[4px] min-w-0 max-w-[55%] overflow-hidden [.qd-pending_&]:shrink">
 {attachments.map((item, index) => item.type === 'image'
 ? <QueueThumb key={`${item.artifactId ?? item.file?.name ?? ''}:${index}`} artifactId={item.artifactId} file={item.file} label={item.alt || tx('agent:queue-dock.image')} />
 : <QueueFile key={`${item.name}:${index}`} name={item.name} bytes={item.bytes} label={tx('agent:queue-dock.file', { name: item.name })} />)}
 </span>
 ) : null}
 <span className="flex-auto min-w-0 overflow-hidden [font:var(--dsw-font-xs-13)] [font-family:Inter,var(--dsw-font-family)] [color:var(--dsw-alias-label-primary-dimmed)] text-ellipsis whitespace-nowrap">{previewText}</span>
 <span className="flex-none [color:var(--dsw-alias-label-tertiary)] [font:var(--dsw-font-xs-13)] whitespace-nowrap" role="status">{tx('agent:submission.sending')}</span>
 <div className="flex flex-none items-center gap-[8px] flex-wrap">
 <button type="button" className="min-h-[32px] px-[8px] py-[4px] border-0 rounded-[var(--dsw-radius-sm)] bg-transparent [color:var(--dsw-alias-label-primary)] text-[12px] cursor-pointer [&[aria-label]]:grid [&[aria-label]]:flex-none [&[aria-label]]:place-items-center [&[aria-label]]:w-[28px] [&[aria-label]]:h-[28px] [&[aria-label]]:min-h-0 [&[aria-label]]:p-0 [&[aria-label]]:rounded-full [&[aria-label]]:[corner-shape:round]! [&[aria-label]]:[color:var(--dsw-alias-label-tertiary)] hover:not-disabled:bg-[var(--dsw-alias-interactive-bg-hover)] disabled:opacity-45 disabled:cursor-default focus-visible:[outline:var(--dsw-focus-ring-width)_solid_var(--dsw-focus-ring-color,var(--dsw-alias-state-business-primary))] focus-visible:-outline-offset-2" aria-label={tx('agent:queue-dock.edit')} title={tx('agent:submission.sending')} disabled><IconEditOutline16 size={14} /></button>
 <button type="button" className="min-h-[32px] px-[8px] py-[4px] border-0 rounded-[var(--dsw-radius-sm)] bg-transparent [color:var(--dsw-alias-label-primary)] text-[12px] cursor-pointer [&[aria-label]]:grid [&[aria-label]]:flex-none [&[aria-label]]:place-items-center [&[aria-label]]:w-[28px] [&[aria-label]]:h-[28px] [&[aria-label]]:min-h-0 [&[aria-label]]:p-0 [&[aria-label]]:rounded-full [&[aria-label]]:[corner-shape:round]! [&[aria-label]]:[color:var(--dsw-alias-label-tertiary)] hover:not-disabled:bg-[var(--dsw-alias-interactive-bg-hover)] disabled:opacity-45 disabled:cursor-default focus-visible:[outline:var(--dsw-focus-ring-width)_solid_var(--dsw-focus-ring-color,var(--dsw-alias-state-business-primary))] focus-visible:-outline-offset-2" aria-label={tx('agent:queue-dock.remove')} title={tx('agent:submission.sending')} disabled><IconTrashOutline16 size={14} /></button>
 <button type="button" className="min-h-[32px] px-[8px] py-[4px] border-0 rounded-[var(--dsw-radius-sm)] bg-transparent [color:var(--dsw-alias-label-primary)] text-[12px] cursor-pointer [&[aria-label]]:grid [&[aria-label]]:flex-none [&[aria-label]]:place-items-center [&[aria-label]]:w-[28px] [&[aria-label]]:h-[28px] [&[aria-label]]:min-h-0 [&[aria-label]]:p-0 [&[aria-label]]:rounded-full [&[aria-label]]:[corner-shape:round]! [&[aria-label]]:[color:var(--dsw-alias-label-tertiary)] hover:not-disabled:bg-[var(--dsw-alias-interactive-bg-hover)] disabled:opacity-45 disabled:cursor-default focus-visible:[outline:var(--dsw-focus-ring-width)_solid_var(--dsw-focus-ring-color,var(--dsw-alias-state-business-primary))] focus-visible:-outline-offset-2" aria-label={tx('agent:queue-dock.steer')} title={tx('agent:submission.sending')} disabled><IconSendOutline14 size={14} /></button>
 </div>
 </li>;
 })}
 </ul> : null}
 {detachedDraft ? <div className={count > 0 ? "box-border flex items-center gap-[10px] w-full px-[5px] pl-[12px] py-[6px] shadow-[inset_0_1px_0_var(--dsw-alias-border-l1)]" : "box-border flex items-center gap-[10px] w-full px-[5px] pl-[12px] py-[6px]"} data-queue-draft="detached">
 {count === 0 ? <QueueGlyph /> : null}
 {editor}
 </div> : null}
 {notice && (!detachedDraft || operation) ? <p className="px-[12px] py-[8px] text-[13px]" role="status">{notice}</p> : null}
 </div>
 </section>;
}
