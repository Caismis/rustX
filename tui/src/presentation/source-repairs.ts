/**
 * Presentation of native automatic `rustx.toml` repair receipts (Issue #464).
 *
 * The App Server owns repair: classification, candidate validation, backup,
 * publication and the receipt itself. This module only words those bounded,
 * redacted receipts for the terminal, and deduplicates notices by the native
 * receipt identity so a reconnect, a repeated read, or the same receipt from
 * both a notification and a read never announces one repair twice.
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

/** One source's latest native repair observation, or none. */
export interface SourceRepairObservation {
  target: SourceTarget;
  receipt: ConfigRepairReceipt | undefined;
}

/** The stable presentation key of one source target. */
export function sourceKey(target: SourceTarget): string {
  return target.kind === "user" ? "user" : `workspace:${target.directory}`;
}

function sourceLabel(target: SourceTarget): string {
  return target.kind === "user" ? "User" : "Workspace";
}

/** Which receipts this client already announced, by native receipt identity. */
export class SourceRepairNotices {
  readonly #announced = new Set<string>();

  /** True exactly once per receipt identity. */
  fresh(receipt: ConfigRepairReceipt): boolean {
    if (this.#announced.has(receipt.id)) return false;
    this.#announced.add(receipt.id);
    // Finite: receipts are per source, so a small window suffices.
    if (this.#announced.size > 256) {
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

/** The scrollable `/settings … repairs` inspection body. */
export function renderRepairInspection(observations: readonly SourceRepairObservation[]): string {
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
      lines.push("", receipt.status === "repaired" || receipt.status === "commit_uncertain" ? "Applied:" : "Eligible, not applied:");
      lines.push(...receipt.changes.map(renderRepairChange));
    }
    return lines.join("\n");
  });
  return [
    ...sections,
    "A repair proves only that authored bytes changed; runtime application is reported separately by /session settings.",
  ].join("\n\n");
}
