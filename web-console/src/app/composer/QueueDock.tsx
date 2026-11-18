import { isPlainMessageKind } from '../../../../protocol/app-server/message-kind';
import type { PendingInboundRef, UserContentBlock, UserInputBlock } from '../../../../protocol/app-server/v80';
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
import { fileSizeText } from '../../presentation/primitives/file-size';
import {
  IconCheckOutline14, IconChevronDownOutline14, IconChevronUpOutline14, IconCloseOutline16, IconEditOutline16, IconQueueOutline14, IconSendOutline14, IconTrashOutline16,
} from '../../presentation/primitives/icons';
import { Tooltip } from '../../presentation/primitives/Tooltip';
import { ArtifactContext } from '../components/Artifact';
import css from './QueueDock.module.css';

type OperationKind = 'edit' | 'remove' | 'steer';

type QueueAttachment =
  | { readonly type: 'image'; readonly artifactId?: string; readonly file?: File; readonly alt?: string | null }
  | { readonly type: 'file'; readonly name: string; readonly bytes?: number };

const QueueGlyph = () => <span className={css.lead} aria-hidden><IconQueueOutline14 /></span>;

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
    <span className={css.file} aria-label={label} title={name}>
      <span className={css.fileIcon} aria-hidden><FileTypeIcon path={name} size={16} /></span>
      <span className={css.fileName}>{name}</span>
      {bytes !== undefined && <span className={css.fileSize}>{fileSizeText(bytes)}</span>}
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
    ? <span className={css.thumb} aria-hidden />
    : <img className={css.thumb} src={url} alt={label} />;
}

export function QueueDock({ rows, submissions, running, disabled = false, edit, remove, steer, attachmentMetadata, observation }: {
  rows: readonly InboundRow[];
  /** Sending or accepted submissions; settled by exact MessageId or sendingInputId. */
  submissions: readonly Submission[];
  running: boolean;
  disabled?: boolean;
  observation?: object;
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
  const [draft, setDraft] = useState<{ expected: PendingInboundRef; messageId: string; text: string }>();
  const [operation, setOperation] = useState<{ status: 'pending' | 'uncertain' | 'readback'; observation?: object; clearDraft?: boolean }>();
  const [notice, setNotice] = useNotice();
  const editButtons = useRef(new Map<string, HTMLButtonElement>());
  const editorNode = useRef<HTMLTextAreaElement>(null);
  const returnFocus = useRef<string | undefined>(undefined);
  useEffect(() => {
    if (operation && operation.status !== 'pending' && observation !== operation.observation) {
      if (operation.clearDraft) setDraft(undefined);
      setOperation(undefined); setNotice('');
    }
  }, [observation, operation, setNotice]);
  useEffect(() => {
    if (draft || !returnFocus.current) return;
    const sequence = returnFocus.current;
    returnFocus.current = undefined;
    editButtons.current.get(sequence)?.focus({ preventScroll: true });
  }, [draft]);
  const currentDraft = draft && rows.find(row => row.sequence === draft.expected.sequence && row.message.id === draft.messageId);
  const staleDraft = !!draft && (!currentDraft || currentDraft.deferred || currentDraft.revision !== draft.expected.revision);
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
    if (!alive.current || result.status === 'obsolete') return;
    if (result.status === 'uncertain') {
      setOperation({ status: 'uncertain', observation: latestObservation.current });
      setNotice(message('agent:copy.outcome-uncertain-do-not-resend-reconnect-to-read-the-authoritative-queue'));
      return;
    }
    setOperation(result.observed ? undefined : { status: 'readback', observation: latestObservation.current, clearDraft: editing && result.status === 'known' && result.outcome.status === 'applied' });
    if (result.status === 'rejected') { setNotice(result.reason); return; }
    if (result.outcome.status === 'applied') {
      if (editing && result.observed) {
        if (draft) returnFocus.current = draft.expected.sequence;
        setDraft(undefined);
      }
      setNotice(result.observed ? '' : message('agent:copy.committed-awaiting-authoritative-readback'));
    } else {
      setNotice(result.outcome.status === 'conflict' ? message('agent:copy.this-item-changed-your-draft-is-preserved-compare-it-with-the-current-row')
        : result.outcome.status === 'not_pending' ? (editing ? message('agent:copy.this-occurrence-is-no-longer-pending-your-draft-is-preserved') : failureNotice[kind])
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
    if (draft && !currentDraft) { setDraft(undefined); setNotice(''); }
  }, [count, draft, currentDraft, setNotice]);
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
  }, [draft?.text]);
  if (count === 0) return null;
  const listed = count === 1 || !collapsed || !!draft || !!operation;
  const cancelEdit = () => {
    if (draft) returnFocus.current = draft.expected.sequence;
    setDraft(undefined);
    setNotice('');
  };
  const editor = draft && <div className={css.editPanel}>
    <div className={css.editorField}>
      <textarea ref={editorNode} className={css.editor} autoFocus rows={1} aria-label={tx('agent:queue-dock.edit-queued-message')} value={draft.text} disabled={locked}
        onChange={event => setDraft({ ...draft, text: event.currentTarget.value })}
        onKeyDown={event => {
          if (event.key === 'Escape' && !locked) {
            event.preventDefault();
            cancelEdit();
          }
          if (event.key === 'Enter' && !event.shiftKey && !event.nativeEvent.isComposing && event.nativeEvent.keyCode !== 229 && !locked && !staleDraft && draft.text.trim() && edit) {
            event.preventDefault();
            void apply(() => edit(draft.expected, draft.text), 'edit');
          }
        }} />
    </div>
    <div className={css.actions}>
      <Tooltip label={tx('agent:queue-dock.save')} side="bottom" delayMs={500} disabled={locked || staleDraft || !draft.text.trim() || !edit}>
        <button aria-label={tx('agent:queue-dock.save')} type="button" className={css.action} disabled={locked || staleDraft || !draft.text.trim() || !edit} onClick={() => { if (edit) void apply(() => edit(draft.expected, draft.text), 'edit'); }}><IconCheckOutline14 size={14} /></button>
      </Tooltip>
      <Tooltip label={tx('agent:queue-dock.cancel-edit')} side="bottom" delayMs={500} disabled={locked}>
        <button aria-label={tx('agent:queue-dock.cancel-edit')} type="button" className={css.action} disabled={locked} onClick={cancelEdit}><IconCloseOutline16 size={14} /></button>
      </Tooltip>
      {staleDraft && currentDraft && <button type="button" className={css.action} disabled={locked} onClick={() => setDraft({ ...draft, expected: expected(currentDraft), messageId: currentDraft.message.id })}>{tx('agent:queue-dock.use-latest-version')}</button>}
    </div>
    {staleDraft && <p role="status">{currentDraft ? tx('agent:queue-dock.this-queued-message-changed-review-the-latest-version-before-sav') : tx('agent:queue-dock.this-message-is-no-longer-queued')}</p>}
  </div>;
  return <section className={css.dock} aria-label={tx('agent:queue-dock.queue')} aria-busy={operation?.status === 'pending'} data-queue-dock="">
    <div className={css.panel}>
      {count > 1 && <button type="button" className={css.header} aria-expanded={listed} disabled={!!draft || !!operation} aria-controls={listId} onClick={() => setCollapsed(value => !value)}>
        <QueueGlyph /><span className={css.count}>{count} {tx('agent:queue-dock.queued')}</span>
        {/* A collapsed header keeps sending/accepted provisional submissions visible. */}
        {!listed && echoes.length > 0 && <span className={css.status} role="status">{tx('agent:submission.sending')}</span>}
        <span className={css.chevron} aria-hidden>{listed ? <IconChevronDownOutline14 /> : <IconChevronUpOutline14 />}</span>
      </button>}
      <ul id={listId} className={css.list} hidden={!listed}>
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
              : undefined;
          const editDisabled = locked || !edit || !!editBlockedReason;
          const removeDisabled = locked || !remove;
          const steerDisabled = locked || !steer || !running;
          return <li key={`inbound:${row.sequence}`} className={css.row} data-inbound-sequence={row.sequence} data-message-id={row.message.id}>
            {count === 1 && <QueueGlyph />}
            {currentDraft === row && !staleDraft ? editor : <>
              {attachments.length > 0 && (
                <span className={css.attachments}>
                  {attachments.map((item, index) => item.type === 'image'
                    ? <QueueThumb key={`${item.artifactId ?? item.file?.name ?? ''}:${index}`} artifactId={item.artifactId} file={item.file} label={item.alt || tx('agent:queue-dock.image')} />
                    : <QueueFile key={`${item.name}:${index}`} name={item.name} bytes={item.bytes} label={tx('agent:queue-dock.file', { name: item.name })} />)}
                </span>
              )}
              <span className={css.preview}>{previewText}</span>
              {origin && <span className={css.status}>{origin}</span>}
              {row.message.source === 'human' && isPlainMessageKind(row.message.kind) && <div className={css.actions}>
                <Tooltip label={tx('agent:queue-dock.edit')} side="bottom" delayMs={500} disabled={editDisabled}>
                  <button
                    ref={element => {
                      if (element) editButtons.current.set(row.sequence, element);
                      else editButtons.current.delete(row.sequence);
                    }}
                    aria-label={tx('agent:queue-dock.edit')}
                    type="button"
                    className={css.action}
                    disabled={editDisabled}
                    title={editBlockedReason}
                    onClick={() => {
                      setDraft({ expected: expected(row), messageId: row.message.id, text: editableText ?? '' });
                      setNotice('');
                    }}
                  ><IconEditOutline16 size={14} /></button>
                </Tooltip>
                <Tooltip label={tx('agent:queue-dock.remove')} side="bottom" delayMs={500} disabled={removeDisabled}>
                  <button aria-label={tx('agent:queue-dock.remove')} type="button" className={css.action} disabled={removeDisabled} onClick={() => { if (remove) void apply(() => remove(expected(row)), 'remove'); }}><IconTrashOutline16 size={14} /></button>
                </Tooltip>
                <Tooltip label={tx('agent:queue-dock.steer')} side="bottom" delayMs={500} disabled={steerDisabled}>
                  <button aria-label={tx('agent:queue-dock.steer')} type="button" className={css.action} disabled={steerDisabled} title={running ? undefined : tx('agent:queue-dock.steer-unavailable')} onClick={() => { if (steer) void apply(() => steer(expected(row)), 'steer'); }}><IconSendOutline14 size={14} /></button>
                </Tooltip>
              </div>}
            </>}
            {currentDraft === row && staleDraft && editor}
          </li>;
        })}
        {listed && echoes.map(submission => {
          const isSending = Boolean(submission.sendingInputId);
          const { attachments, previewBlocks } = submissionAttachments(submission.content ?? [], attachmentMetadata);
          const previewText = previewBlocks.length > 0 ? contentPreview(tx, previewBlocks) : '';
          return <li
            key={isSending ? `sending:${submission.sendingInputId}` : `accepted:${submission.messageId}`}
            className={`${css.row} ${css.pendingRow}`}
            data-submission-echo=""
            data-sending-input-id={submission.sendingInputId}
            data-accepted-message-id={isSending ? undefined : submission.messageId}
          >
            {count === 1 && <QueueGlyph />}
            {attachments.length > 0 && (
              <span className={css.attachments}>
                {attachments.map((item, index) => item.type === 'image'
                  ? <QueueThumb key={`${item.artifactId ?? item.file?.name ?? ''}:${index}`} artifactId={item.artifactId} file={item.file} label={item.alt || tx('agent:queue-dock.image')} />
                  : <QueueFile key={`${item.name}:${index}`} name={item.name} bytes={item.bytes} label={tx('agent:queue-dock.file', { name: item.name })} />)}
              </span>
            )}
            <span className={css.preview}>{previewText}</span>
            <span className={css.status} role="status">{tx('agent:submission.sending')}</span>
            <div className={css.actions}>
              <button type="button" className={css.action} aria-label={tx('agent:queue-dock.edit')} title={tx('agent:submission.sending')} disabled><IconEditOutline16 size={14} /></button>
              <button type="button" className={css.action} aria-label={tx('agent:queue-dock.remove')} title={tx('agent:submission.sending')} disabled><IconTrashOutline16 size={14} /></button>
              <button type="button" className={css.action} aria-label={tx('agent:queue-dock.steer')} title={tx('agent:submission.sending')} disabled><IconSendOutline14 size={14} /></button>
            </div>
          </li>;
        })}
      </ul>
      {notice && <p className={css.notice} role="status">{notice}</p>}
    </div>
  </section>;
}
