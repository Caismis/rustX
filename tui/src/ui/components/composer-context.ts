import { truncateToWidth } from "@earendil-works/pi-tui";
import type { PresentationState } from "../../presentation/state.ts";
import { isAttemptActive } from "../../presentation/state.ts";
import type { ComposerDraft } from "../composer-draft.ts";
import { pendingText } from "./pending-input.ts";

/** Small native queue viewport above Pi's height-bounded editor. */
export class ComposerContext {
  readonly #facts: () => { state?: PresentationState; draft: ComposerDraft };
  constructor(facts: () => { state?: PresentationState; draft: ComposerDraft }) { this.#facts = facts; }
  invalidate(): void {}
  render(width: number): string[] {
    const { state, draft } = this.#facts();
    if (!state) return [];
    const rows: string[] = [];
    const pending = (state.inbound.pending ?? []).filter(item => item.message.source === "human");
    if (pending.length) {
      rows.push(`Queued ${pending.length} · /queue edit/remove`);
      rows.push(`↳ ${pendingText(pending[0]!).replaceAll("\n", " ↵ ")}`);
    }
    const prefix = draft.tail ? draft.blocks?.filter(block => block.type === "text").map(block => block.text).join("") : "";
    if (draft.uploads || prefix) rows.push(`${draft.uploads} Session attachment(s)${prefix ? ` · preceding text: ${prefix.replaceAll("\n", " ↵ ")}` : ""}`);
    rows.push(isAttemptActive(state) ? "Enter Steer · Tab Queue · Esc Interrupt" : "Enter Send · Ctrl+P commands · Ctrl+R history");
    return rows.map(row => truncateToWidth(row, Math.max(1, width)));
  }
}
