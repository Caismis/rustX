/**
 * Presentation of native automatic `rustx.toml` repair receipts (Issue #464).
 *
 * The App Server owns repair: classification, candidate validation, backup,
 * publication and the receipt itself. This module only words those bounded,
 * redacted receipts for the terminal, orders native observations of one
 * source by their native repair-ledger sequence, and remembers which receipts
 * were announced by the App Server authority and native receipt identity, so
 * a reconnect, a repeated read, or the same receipt from both a notification
 * and a read never announces one repair twice, while an equal content-derived
 * receipt id from another App Server — or a replacement process — is a
 * repair of its own.
 *
 * Nothing here reads a configuration file, applies a rule, or reaches a model:
 * a repair notice is presentation only and never becomes a conversation Turn.
 */

import type {
  ConfigRepairChange,
  ConfigRepairReceipt,
  SourceTarget,
} from "../protocol/app-server.ts";
import { sanitizeField } from "../sanitize.ts";

/**
 * One native observation of a source's latest repair receipt, or of none, at
 * the native repair-ledger `sequence` it is current at: the `sequence` of a
 * `configuration/sourceRepairChanged`, or the `repair_sequence` of a
 * `configuration/sourcesRead`. Within one App Server authority the higher
 * sequence is the newer observation of a source, whichever carried it.
 * Receipt ids identify content, never time, and never order observations.
 */
export interface SourceRepairObservation {
  target: SourceTarget;
  receipt: ConfigRepairReceipt | undefined;
  sequence: string;
}

/** Workspaces whose repair observations a client keeps: as many as the
 * native repair ledger retains (`MAX_LEDGER_WORKSPACES`). The User source has
 * its own slot beside them. */
export const SOURCE_REPAIR_WORKSPACES = 64;

/**
 * The one client retention policy for per-source repair state, applied by
 * each owner to its own map — the client to notices buffered before its first
 * subscriber, the host to its connection's observations, the status line to
 * what it presents: `value` becomes `key`'s entry and its most recently
 * observed one; beyond `SOURCE_REPAIR_WORKSPACES` Workspaces the least
 * recently observed Workspace is forgotten, whether or not the User has an
 * entry, and the User never is. Returns the forgotten entry, whose owner
 * retires whatever it held for that source.
 */
export function retainSource<V>(sources: Map<string, V>, key: string, value: V): V | undefined {
  sources.delete(key);
  sources.set(key, value);
  if (sources.size - (sources.has("user") ? 1 : 0) <= SOURCE_REPAIR_WORKSPACES) return undefined;
  for (const [source, entry] of sources) {
    if (source === "user") continue;
    sources.delete(source);
    return entry;
  }
  return undefined;
}

/** Whether observation sequence `a` is older than `b`. Both are exact
 * decimal ledger sequences of one App Server authority. */
export function olderSequence(a: string, b: string): boolean {
  return BigInt(a) < BigInt(b);
}

/** The stable presentation key of one source target. */
export function sourceKey(target: SourceTarget): string {
  return target.kind === "user" ? "user" : `workspace:${target.directory}`;
}

function sourceLabel(target: SourceTarget): string {
  return target.kind === "user" ? "User" : "Workspace";
}

/** Which receipts this client already announced, by the negotiated App
 * Server authority and native receipt identity. In memory only, and never
 * native authority: it decides what is announced, not what a source holds. */
export class SourceRepairAnnouncements {
  static readonly LIMIT = 256;
  readonly #announced = new Set<string>();

  /** True exactly once per `(authorityId, receipt.id)` within the bound. */
  announce(authorityId: string, receipt: ConfigRepairReceipt): boolean {
    const identity = JSON.stringify([authorityId, receipt.id]);
    if (this.#announced.has(identity)) return false;
    this.#announced.add(identity);
    // Finite: receipts are per source, so a small window suffices.
    if (this.#announced.size > SourceRepairAnnouncements.LIMIT) {
      const oldest = this.#announced.values().next().value;
      if (oldest !== undefined) this.#announced.delete(oldest);
    }
    return true;
  }
}

export type RepairNoticeLevel = "info" | "warning" | "error";

/** One concise status line for newly observed receipts of distinct sources. */
export function repairNotice(receipts: readonly ConfigRepairReceipt[]): { level: RepairNoticeLevel; text: string } | undefined {
  if (receipts.length === 0) return undefined;
  const failed = receipts.filter(receipt => receipt.status !== "repaired");
  if (failed.length > 0) {
    const receipt = failed[0]!;
    const owner = sourceLabel(receipt.target);
    const command = receipt.target.kind === "user" ? "/settings user repairs" : "/settings workspace repairs";
    const detail = receipt.diagnostic ? `: ${sanitizeField(receipt.diagnostic, false)}` : "";
    return receipt.status === "commit_uncertain"
      ? { level: "error", text: `Config: ${owner} rustx.toml repair was not durably confirmed${detail}. Run ${command} for details.` }
      : { level: "warning", text: `Config: ${owner} rustx.toml could not be auto-repaired${detail}. Run ${command} for details.` };
  }
  const counts = receipts.map(receipt => `${receipt.changed_count} ${receipt.changed_count === 1 ? "entry" : "entries"} in ${sourceLabel(receipt.target)}`);
  return { level: "info", text: `Config: Auto-repaired ${counts.join(" and ")} rustx.toml. Run /settings repairs for details.` };
}

const REMOVAL_REASONS: Record<Extract<ConfigRepairChange, { kind: "removed_native_tool" }>["reason"], string> = {
  unknown: "unknown Native Tool",
  duplicate: "duplicate selection",
  agent_control: "Agent control composed automatically",
  plugin_owned: "provided by a Plugin",
};

/** One redacted `+`/`-`/`~` line per change; never a value. */
export function renderRepairChange(change: ConfigRepairChange): string {
  switch (change.kind) {
    case "removed_field":
      return `- ${sanitizeField(change.field_path, false)} (unknown field)`;
    case "removed_native_tool":
      return `- agent.tools.builtin ${JSON.stringify(sanitizeField(change.name, false))} (${REMOVAL_REASONS[change.reason]})`;
    case "updated_schema_version":
      return `~ schema_version ${change.from} → ${change.to}`;
  }
}

const STATUS: Record<ConfigRepairReceipt["status"], string> = {
  repaired: "repaired",
  unresolved: "unresolved — the source is unchanged",
  commit_uncertain: "commit uncertain — reread before editing; nothing is replayed",
};

/** What the listed changes are to the source: committed, never published,
 * or published without durable confirmation — never a confirmed application. */
const CHANGES_HEADING: Record<ConfigRepairReceipt["status"], string> = {
  repaired: "Applied:",
  unresolved: "Eligible, not applied:",
  commit_uncertain: "Possibly published; durability not confirmed:",
};

/** The scrollable `/settings … repairs` inspection body. */
export function renderRepairInspection(observations: readonly Pick<SourceRepairObservation, "target" | "receipt">[]): string {
  const sections = observations.map(({ target, receipt }) => {
    const heading = target.kind === "user"
      ? "## User rustx.toml"
      : `## Workspace rustx.toml (${sanitizeField(target.directory, false)})`;
    if (receipt === undefined) {
      return `${heading}\nNo automatic repair was needed in this App Server process.`;
    }
    const lines = [
      heading,
      `Status: ${STATUS[receipt.status]}`,
      `Changes: ${receipt.changed_count}${receipt.truncated ? ` (first ${receipt.changes.length} shown)` : ""}`,
      `Original backup: ${receipt.backup_available ? "kept in private rustX storage" : "none"}`,
      `Revision: ${receipt.before_revision}${receipt.after_revision ? ` → ${receipt.after_revision}` : ""}`,
    ];
    if (receipt.diagnostic) lines.push(`Diagnostic: ${sanitizeField(receipt.diagnostic, false)}`);
    if (receipt.changes.length > 0) {
      lines.push("", CHANGES_HEADING[receipt.status]);
      lines.push(...receipt.changes.map(renderRepairChange));
    }
    return lines.join("\n");
  });
  return [
    ...sections,
    "A repair proves only that authored bytes changed; runtime application is reported separately by /session settings.",
  ].join("\n\n");
}
