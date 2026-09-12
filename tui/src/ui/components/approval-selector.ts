/** Focused approval intent; native state remains the only mode authority. */
import { matchesKey, truncateToWidth, wrapTextWithAnsi } from "@earendil-works/pi-tui";
import { approvalLabel } from "../../presentation/selectors.ts";
import { isAttemptActive, type PresentationState } from "../../presentation/state.ts";
import type { ApprovalMode } from "../../protocol/types.ts";
import { role } from "../theme.ts";
import type { PopupContent } from "./popup-frame.ts";

const choices = [
  { mode: "policy", label: "Policy", description: "Respect per-tool approval policy; ask where required." },
  { mode: "full_access", label: "Full access", description: "Skip ordinary Tool approval prompts for already-admitted Tools." },
] as const;
const warning = "Already-admitted Tools may perform operations that normally require approval without another prompt, including command execution or file-changing operations when those Tools are available. This does not grant unavailable Tools/capabilities, answer Questionnaire or Workflow Review, or define a filesystem/network sandbox profile.";

export class ApprovalSelector implements PopupContent {
  #selected = 0;
  #confirm = false;
  #enable = false;
  #pending = false;
  #closed = false;
  #height = 20;
  #scroll = 0;
  readonly #state: () => PresentationState | undefined;
  readonly #submit: (mode: ApprovalMode) => Promise<void>;
  readonly #close: () => void;
  readonly #change: () => void;

  constructor(options: {
    state: () => PresentationState | undefined;
    submit: (mode: ApprovalMode) => Promise<void>;
    close: () => void;
    change: () => void;
  }) {
    this.#state = options.state;
    this.#submit = options.submit;
    this.#close = options.close;
    this.#change = options.change;
  }
  popupTitle(): string { return this.#confirm ? "Enable full access?" : "Approval mode"; }
  popupFooter(): string[] {
    return [this.#pending ? "Request pending · Esc close" : this.#confirm
      ? "←→/Tab choose · ↑↓ scroll · Enter activate · Esc cancel"
      : "↑↓ navigate · Enter select · Esc cancel"];
  }
  invalidate(): void {}
  setBodyHeight(height: number): void { this.#height = Math.max(1, height); }
  handleInput(data: string): void {
    if (this.#closed) return;
    if (matchesKey(data, "escape")) { this.#closed = true; this.#close(); return; }
    if (this.#pending) return;
    if (this.#confirm) {
      if (matchesKey(data, "tab") || matchesKey(data, "left") || matchesKey(data, "right")) this.#enable = !this.#enable;
      if (matchesKey(data, "up")) this.#scroll = Math.max(0, this.#scroll - 1);
      if (matchesKey(data, "down")) this.#scroll++;
    } else {
      if (matchesKey(data, "up")) this.#selected = 0;
      if (matchesKey(data, "down")) this.#selected = 1;
    }
    if (matchesKey(data, "enter")) {
      if (this.#confirm) {
        if (!this.#enable) { this.#closed = true; this.#close(); return; }
        this.#commit("full_access");
      } else if (this.#selected === 1) {
        this.#confirm = true;
        this.#enable = false;
      } else this.#commit("policy");
    }
    this.#change();
  }
  #commit(mode: ApprovalMode): void {
    // Synchronous latch before calling the single typed native operation.
    // Never copy desired/effective values into presentation state.
    this.#pending = true;
    void this.#submit(mode).finally(() => {
      this.#closed = true;
      this.#close();
    });
  }
  render(width: number): string[] {
    const state = this.#state();
    const facts: string[] = [];
    if (state?.effectiveApprovalMode != null) {
      const lifetime = isAttemptActive(state) ? "Current attempt" : "Effective";
      facts.push(`${lifetime}: ${approvalLabel(state.effectiveApprovalMode)}`);
    }
    if (state?.pendingApprovalMode != null) facts.push(`Next attempt: ${approvalLabel(state.pendingApprovalMode)}`);
    const rows = this.#confirm
      ? [`${this.#enable ? " " : "❯"} Cancel   ${this.#enable ? "❯" : " "} Enable full access`]
      : choices.map((choice, index) => `${index === this.#selected ? "❯" : " "} ${state?.effectiveApprovalMode === choice.mode ? "✓" : " "} ${choice.label}`);
    if (this.#pending) rows.unshift("Request pending");
    if (this.#height === 1) return [truncateToWidth(rows[this.#confirm || this.#pending ? 0 : this.#selected]!, width, "…")];
    const detail = [...facts, "", ...(this.#confirm ? [warning] : [choices[this.#selected]!.description])]
      .flatMap((line) => wrapTextWithAnsi(role.meta(line), Math.max(1, width)));
    const room = Math.max(0, this.#height - rows.length);
    this.#scroll = Math.min(this.#scroll, Math.max(0, detail.length - room));
    return [...rows.map((row) => truncateToWidth(row, width, "…")), ...detail.slice(this.#scroll, this.#scroll + room)];
  }
}
