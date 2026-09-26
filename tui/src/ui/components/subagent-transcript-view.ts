import { matchesKey, truncateToWidth } from "@earendil-works/pi-tui";
import { emptyPresentationState, mergeTranscriptPage } from "../../presentation/projection.ts";
import type { RuntimeClientAgent } from "../../protocol/app-server.ts";
import type { SubagentTranscript } from "../../app-server/subagent-transcript.ts";
import type { PresentationPreferences } from "../preferences.ts";
import type { PopupContent } from "./popup-frame.ts";
import { renderTranscript } from "./transcript.ts";
import { banded } from "./transcript-block.ts";

/** The ordinary transcript grammar, with no Composer or interaction controls. */
export class SubagentTranscriptView implements PopupContent {
  #offset = 0;
  #height = 10;
  #lines = 0;
  readonly reader: SubagentTranscript;
  readonly child: () => RuntimeClientAgent | undefined;
  readonly preferences: PresentationPreferences;
  readonly close: () => void;
  readonly changed: () => void;
  constructor(
    reader: SubagentTranscript,
    child: () => RuntimeClientAgent | undefined,
    preferences: PresentationPreferences,
    close: () => void,
    changed: () => void,
  ) {
    this.reader = reader; this.child = child; this.preferences = preferences;
    this.close = close; this.changed = changed;
  }
  popupTitle(): string {
    const child = this.child();
    return `${child?.agent ?? this.reader.selected} · read only · ${child?.state ?? "unavailable"}`;
  }
  popupFooter(): string[] {
    return ["↑↓ scroll · PageUp older · Home newest · Esc Main"];
  }
  setBodyHeight(height: number): void { this.#height = Math.max(1, height); }
  handleInput(data: string): void {
    if (matchesKey(data, "escape")) { this.close(); return; }
    if (matchesKey(data, "pageUp")) { this.#offset = 0; void this.reader.older(); }
    else if (matchesKey(data, "home")) { this.#offset = 0; void this.reader.newest(); }
    else if (matchesKey(data, "up")) this.#offset--;
    else if (matchesKey(data, "down")) this.#offset++;
    else if (matchesKey(data, "pageDown")) this.#offset += this.#height;
    this.changed();
  }
  render(width: number): string[] {
    const page = this.reader.page;
    const lines = this.reader.error ? [this.reader.error] : page === undefined ? ["Reading child history…"] :
      renderTranscript(mergeTranscriptPage(emptyPresentationState(null), page), this.preferences)
        .flatMap(block => [...banded(block).render(width), ""]);
    if (page && lines.length === 0) lines.push("No committed transcript entries.");
    this.#lines = lines.length;
    this.#offset = Math.max(0, Math.min(this.#offset, this.#lines - this.#height));
    return lines.slice(this.#offset, this.#offset + this.#height).map(line => truncateToWidth(line, width));
  }
  invalidate(): void {}
}
