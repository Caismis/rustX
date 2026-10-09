/**
 * Committed `present` deliveries, as `/files` lists them.
 *
 * The only source is the typed `deliveries` field of a **successfully
 * committed canonical Tool-result message**, read from bounded native
 * transcript pages. Tool arguments, tool output text, generic JSON content,
 * model prose, and foreground settlements never become entries: none of them
 * is a committed canonical result.
 *
 * A delivery's stable address is its committed Tool-result message id and
 * delivery index. The reference inside keeps its original Conversation and
 * root identity across clone/fork/history, so the address is all a client
 * holds; the native side resolves it again on every access.
 */

import type {
  MessageId,
  RuntimeClientTranscriptCursor,
  RuntimeClientTranscriptPage,
  SessionFileReference,
} from "../protocol/app-server.ts";

/** One committed delivery and its stable native address. */
export interface DeliveryRecord {
  /** The committed canonical Tool-result message that declared it. */
  messageId: MessageId;
  /** Its position in that result's ordered `deliveries`. */
  index: number;
  /** Total deliveries of that result, for an `i/n` identity. */
  count: number;
  file: SessionFileReference;
}

/**
 * A native location: an absolute path in the **server's** filesystem
 * namespace and the verified leaf's device/inode. It is never a client path
 * by itself; only a demonstrably shared filesystem may interpret it locally.
 */
export interface DeliveryLocation {
  file: SessionFileReference;
  path: string;
  device: string;
  inode: string;
}

/** One bounded page of committed deliveries, newest result first. */
export interface DeliveryPage {
  records: DeliveryRecord[];
  /** The exclusive cursor for the next older transcript page. */
  next?: RuntimeClientTranscriptCursor;
}

/**
 * The deliveries of one transcript page.
 *
 * Newest committed result first, and within one result its canonical order.
 * Non-success results contribute nothing, even if a field is present.
 */
export function pageDeliveries(page: RuntimeClientTranscriptPage): DeliveryPage {
  const records: DeliveryRecord[] = [];
  for (const entry of [...(page.entries ?? [])].reverse()) {
    if (entry.item.type !== "message") continue;
    const message = entry.item.message;
    if (message.role !== "tool" || message.result.status.type !== "success") continue;
    const files = message.result.deliveries ?? [];
    files.forEach((file, index) => {
      records.push({ messageId: message.id, index, count: files.length, file });
    });
  }
  const next = page.next_cursor ?? undefined;
  return next === undefined ? { records } : { records, next };
}

/** A short delivery identity for display: `<message>#<index+1>/<count>`. */
export function deliveryIdentity(record: DeliveryRecord): string {
  const id = record.messageId.length > 12 ? `…${record.messageId.slice(-12)}` : record.messageId;
  return `${id}#${record.index + 1}/${record.count}`;
}

/** The display file type: the extension, or the inert MIME classification. */
export function deliveryType(file: SessionFileReference): string {
  const dot = file.name.lastIndexOf(".");
  return dot > 0 && dot < file.name.length - 1
    ? file.name.slice(dot + 1).toUpperCase()
    : file.mime_type;
}
