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
  #traceIndex = -1;
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
    return `${child?.agent ?? this.reader.selected} · read only · ${this.reader.phase} · ${child?.state ?? "unavailable"}`;
  }
  popupFooter(): string[] {
    return ["↑↓ scroll · PageUp older · Home follow · t Trace · m message · b collection · p page · c content · n next bytes · Esc Main"];
  }
  setBodyHeight(height: number): void { this.#height = Math.max(1, height); }
  handleInput(data: string): void {
    if (matchesKey(data, "escape")) { this.close(); return; }
    if(matchesKey(data,"t")){const records=this.reader.traceRecords.filter(record=>record.kind!=='request');if(records.length){this.#traceIndex=(this.#traceIndex+1)%records.length;void this.reader.inspectTrace(records[this.#traceIndex]!.id);}}
    else if(matchesKey(data,"m")){this.reader.selectTraceMessage();}
    else if(matchesKey(data,"b")){void this.reader.selectTraceCollection();}
    else if(matchesKey(data,"p")){void this.reader.nextTraceMessages();}
    else if (matchesKey(data, "c")) { void this.reader.loadContent(this.reader.content ? (this.reader.content.index+1)%Math.max(1,this.reader.deferredCount) : 0); }
    else if (matchesKey(data, "n") && this.reader.content?.next !== undefined) { void this.reader.loadContent(this.reader.content.index, this.reader.content.next); }
    else if (matchesKey(data, "pageUp")) { this.#offset = 0; void this.reader.older(); }
    else if (matchesKey(data, "home")) { this.#offset = 0; void this.reader.newest(); }
    else if (matchesKey(data, "up")) this.#offset--;
    else if (matchesKey(data, "down")) this.#offset++;
    else if (matchesKey(data, "pageDown")) this.#offset += this.#height;
    this.changed();
  }
  render(width: number): string[] {
    const page = this.reader.page;
    const lines = this.reader.error ? [this.reader.error] : page === undefined ? ["Reading child history…"] :
      renderTranscript(this.reader.state ?? mergeTranscriptPage(emptyPresentationState(null), page), this.preferences)
        .flatMap(block => [...banded(block).render(width), ""]);
    const inspected=this.reader.inspection?.head;
    if(inspected){
      lines.push(`Trace ${inspected.id} · ${inspected.kind} · ${inspected.record.state}`);
      if(inspected.content.type==='tool'){lines.push(`Tool definition: ${inspected.content.definition.type}`);if(this.reader.inspectionCollections.length)lines.push(`${this.reader.inspectionCollections.length} original Tool collections · b selects`);}
      if(inspected.content.type==='messages')lines.push(`Original messages · ${inspected.content.page.total} total · p next bounded page`);
    }
    const message=this.reader.inspectionMessage;
    if(message)lines.push(`Original message: ${message.identity??JSON.stringify(message.locator)} · ${message.collections.length} collections`);
    const collection=this.reader.inspection?.collection;
    if(collection)lines.push(`${collection.domain} · ${collection.total} items · p next page`);
    if (this.reader.deferredCount) lines.push(`${this.reader.deferredCount} original content selections · c reads at most 65536 bytes.`);
    if (this.reader.content) {
      const nativeField=this.reader.deferredJournalFields[this.reader.content.index-this.reader.deferred.length];
      if(nativeField)lines.push(nativeField.label);
      if(this.reader.deferredAttemptIdentities[this.reader.content.index-this.reader.deferred.length-this.reader.deferredJournal.length])lines.push('Original Turn owner identity');
      const trace=this.reader.traceFields[this.reader.content.index-this.reader.deferred.length-this.reader.deferredJournal.length-this.reader.deferredAttemptIdentities.length];
      if(trace)lines.push(`Original Trace field: ${trace.field}`);
      lines.push(this.reader.content.text);
    }
    if (this.reader.phase === 'durableOnly') lines.unshift('DurableOnly · retained history; execution outcome is independent.');
    if (page && lines.length === 0) lines.push("No committed transcript entries.");
    this.#lines = lines.length;
    this.#offset = Math.max(0, Math.min(this.#offset, this.#lines - this.#height));
    return lines.slice(this.#offset, this.#offset + this.#height).map(line => truncateToWidth(line, width));
  }
  invalidate(): void {}
}
