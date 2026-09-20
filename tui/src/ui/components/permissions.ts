import { PasteGuard } from "../paste-guard.ts";
import { matchesKey, Text } from "@earendil-works/pi-tui";
import type { AppServerSession } from "../../app-server/session.ts";
import type { ApprovalMode } from "../../protocol/app-server.ts";
import { isAttemptActive } from "../../presentation/state.ts";
import { approvalLabel } from "../../presentation/selectors.ts";
import type { PopupContent } from "./popup-frame.ts";

type Sources = Awaited<ReturnType<AppServerSession["permissionSources"]>>;
/** A view over native sources and native application authority. */
export class PermissionsView implements PopupContent {
  #source: Sources;
  #selected = 0;
  #busy = false;
  readonly #paste = new PasteGuard();
  #failed = false;
  #height = 12;
  #notice = "";
  #detailOffset: number | undefined;
  readonly session: AppServerSession;
  readonly close: () => void;
  readonly changed: () => void;
  constructor(session: AppServerSession, source: Sources, close: () => void, changed: () => void) { this.session = session; this.close = close; this.changed = changed; this.#source = source; }
  popupTitle(): string { return "Permissions"; }
  popupFooter(): string[] { return ["↑↓ select · Enter save · PgDn details · Esc"]; }
  setBodyHeight(height: number): void { this.#height = height; }
  invalidate(): void {}
  handleInput(data: string): void {
    if (this.#paste.content(data)) { return;
    }
    if (matchesKey(data, "escape")) { this.close(); return; }
    if (matchesKey(data, "pageDown")) {
      this.#detailOffset = this.#detailOffset === undefined ? 0 : this.#detailOffset + this.#height;
      this.changed(); return;
    }
    if (matchesKey(data, "pageUp")) {
      this.#detailOffset = !this.#detailOffset ? undefined : Math.max(0, this.#detailOffset - this.#height);
      this.changed(); return;
    }
    if (this.#busy || this.#failed || this.#detailOffset !== undefined) return;
    if (matchesKey(data, "up") || matchesKey(data, "down")) this.#selected = 1 - this.#selected;
    else if (matchesKey(data, "enter")) void this.save(this.#selected === 0 ? "policy" : "full_access");
    this.changed();
  }
  async save(mode: ApprovalMode): Promise<void> {
    if (this.#busy || this.#failed) return;
    await this.#mutate(() => this.session.writePermission(this.#source.workspace.revision, mode));
  }
  async #mutate(action: () => Promise<Sources>): Promise<void> {
    this.#busy = true; this.#notice = "Waiting for native confirmation…"; this.changed();
    try { this.#source = await action(); this.#notice = "Native configuration reread."; }
    catch (error) { this.#failed = true; this.#notice = `${error instanceof Error ? error.message : String(error)} · Not replayed. Close and reopen to inspect authority.`; }
    finally { this.#busy = false; this.changed(); }
  }
  render(width: number): string[] {
    const state = this.session.state;
    const frozen = isAttemptActive(state) ? state.attempt?.executionSettings?.approval_mode : undefined;
    const desired = this.#source.prospective_approval_mode;
    const application = this.session.application ?? this.#source.application;
    const units = Object.values(application?.units ?? {});
    const status = [
      ...(units.some(unit => unit.status === "preparing") ? ["Preparing configuration…"] : []),
      ...(units.some(unit => unit.status === "applied") ? ["Applied for future independent Attempts."] : []),
      ...(application?.candidate ? ["Context changes await adoption · /configuration"] : []),
      ...(units.some(unit => unit.status === "failed") ? ["Application failed · /configuration retry"] : []),
      ...(units.some(unit => unit.status === "process_restart") ? ["Process restart required."] : []),
    ];
    const details = new Text([
      ...status,
      ...(this.#notice ? [this.#notice] : []),
      "Saved policy applies automatically to future independent Attempts.",
      `Current: ${approvalLabel(state.effectiveApprovalMode)}`,
      ...(frozen === undefined ? [] : [`Running attempt (frozen): ${approvalLabel(frozen)}`]),
      `Desired: ${desired == null ? "unavailable" : approvalLabel(desired)}`,
    ].join("\n"), 0, 0).render(width);
    if (this.#detailOffset !== undefined) {
      this.#detailOffset = Math.min(this.#detailOffset, Math.max(0, details.length - this.#height));
      return details.slice(this.#detailOffset, this.#detailOffset + this.#height);
    }
    const choices = new Text([
      `${this.#selected === 0 ? "❯" : " "} Tool policy`, `${this.#selected === 1 ? "❯" : " "} Full access`,
    ].join("\n"), 0, 0).render(width);
    return [...choices, ...details].slice(0, this.#height);
  }
}
