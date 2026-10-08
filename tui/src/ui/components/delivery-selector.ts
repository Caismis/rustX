/**
 * `/files`: committed `present` deliveries of the focused Session.
 *
 * A presentation and intent surface only. Entries are the typed committed
 * delivery records the App Server pages to it; the selector never reads files,
 * resolves paths, or authenticates. It dispatches explicit intents — load an
 * older page, save to a typed destination, open — and shows the outcome the
 * owning app reports for the exact operation it started. An outcome for an
 * operation this surface no longer owns is dropped.
 */

import {
  Input,
  matchesKey,
  truncateToWidth,
  type Focusable,
} from "@earendil-works/pi-tui";

import {
  deliveryIdentity,
  deliveryType,
  type DeliveryPage,
  type DeliveryRecord,
} from "../../presentation/deliveries.ts";
import { isRenderableField, sanitizeField } from "../../sanitize.ts";
import { role, style } from "../theme.ts";
import { windowAroundSelected, type PopupContent } from "./popup-frame.ts";

const DEFAULT_BODY_HEIGHT = 24;
const ENTRY_ROWS = 2;

/** Which actions this connection supports, and why not when it does not. */
export interface DeliveryAvailability {
  /** Present exactly when Save is unavailable. */
  save?: string;
  /** Present exactly when Open is unavailable. */
  open?: string;
}

export type DeliveryAction = "open" | "save";

type Mode =
  | { kind: "list" }
  | { kind: "actions"; record: DeliveryRecord; selected: number }
  | { kind: "destination"; record: DeliveryRecord; prefilled: boolean };

/**
 * The editable client-local save destination.
 *
 * Every value it holds is drawn as typed, so it holds only renderable text:
 * pi-tui's Input refuses typed control characters but keeps them in a
 * bracketed paste and in `setValue`. A paste that would add a terminal
 * control or bidi character is refused whole; a programmatic value must
 * already be renderable.
 */
export class DestinationInput extends Input {
  override setValue(value: string): void {
    if (!isRenderableField(value)) throw new Error("destination must be renderable text");
    super.setValue(value);
  }

  override handleInput(data: string): void {
    const before = this.getValue();
    super.handleInput(data);
    if (!isRenderableField(this.getValue())) super.setValue(before);
  }
}

export class DeliverySelector implements PopupContent, Focusable {
  focused = false;
  onCancel?: () => void;
  onChange?: () => void;
  onLoadMore?: () => void;
  /** Starts one operation; the app reports back through {@link settle}. */
  onAction?: (operation: number, action: DeliveryAction, record: DeliveryRecord, destination?: string) => void;
  /** Requests cancellation of the operation in flight. */
  onAbort?: (operation: number) => void;

  readonly #availability: DeliveryAvailability;
  #records: DeliveryRecord[];
  #next: DeliveryPage["next"];
  #loading = false;
  #selected = 0;
  #mode: Mode = { kind: "list" };
  #destination = new DestinationInput();
  #operation = 0;
  #busy = false;
  #status: { level: "info" | "error"; text: string } | undefined;
  #bodyHeight = DEFAULT_BODY_HEIGHT;

  constructor(page: DeliveryPage, availability: DeliveryAvailability) {
    this.#records = page.records;
    this.#next = page.next;
    this.#availability = availability;
    this.#destination.onSubmit = (value) => this.#submitDestination(value);
  }

  /** The cursor of the next older transcript page, when one exists. */
  get nextCursor(): DeliveryPage["next"] {
    return this.#next;
  }

  /** Appends the next older bounded page. */
  appendPage(page: DeliveryPage): void {
    this.#records = [...this.#records, ...page.records];
    this.#next = page.next;
    this.#loading = false;
    this.onChange?.();
  }

  retryPage(): void {
    this.#loading = false;
    this.onChange?.();
  }

  /** Reports the outcome of the exact operation this surface started. */
  settle(operation: number, level: "info" | "error", text: string): void {
    if (operation !== this.#operation || !this.#busy) return;
    this.#busy = false;
    this.#status = { level, text: sanitizeField(text) };
    this.onChange?.();
  }

  invalidate(): void {
    this.#destination.invalidate();
  }

  popupTitle(): string {
    return "Delivered files";
  }

  popupFooter(): string[] {
    switch (this.#mode.kind) {
      case "list":
        return ["↑↓ navigate · Enter actions · s save · o open · Esc close"];
      case "actions":
        return ["↑↓ choose · Enter run · Esc back"];
      case "destination":
        return [this.#busy ? "Esc cancel save" : "Enter save to this client-local path · Esc back"];
    }
  }

  setBodyHeight(height: number): void {
    this.#bodyHeight = Math.max(1, Math.floor(height));
  }

  handleInput(data: string): void {
    if (this.#busy) {
      if (matchesKey(data, "escape")) this.onAbort?.(this.#operation);
      return;
    }
    const mode = this.#mode;
    if (mode.kind === "destination") {
      if (matchesKey(data, "escape")) this.#setMode({ kind: "actions", record: mode.record, selected: 1 });
      else this.#destination.handleInput(data);
      this.onChange?.();
      return;
    }
    if (mode.kind === "actions") {
      if (matchesKey(data, "escape")) this.#setMode({ kind: "list" });
      else if (matchesKey(data, "up") || matchesKey(data, "down")) {
        this.#setMode({ ...mode, selected: mode.selected === 0 ? 1 : 0 });
      } else if (matchesKey(data, "enter")) this.#request(mode.selected === 0 ? "open" : "save", mode.record);
      return;
    }
    const rows = this.#rowCount();
    if (matchesKey(data, "escape")) this.onCancel?.();
    else if (matchesKey(data, "up")) this.#move(-1, rows);
    else if (matchesKey(data, "down")) this.#move(1, rows);
    else if (matchesKey(data, "enter")) {
      const record = this.#records[this.#selected];
      if (record !== undefined) this.#setMode({ kind: "actions", record, selected: 0 });
      else this.#loadMore();
    } else if (data === "s" || data === "o") {
      const record = this.#records[this.#selected];
      if (record !== undefined) this.#request(data === "s" ? "save" : "open", record);
    }
  }

  render(width: number): string[] {
    const lines: string[] = [];
    if (this.#status !== undefined) {
      lines.push(this.#status.level === "error" ? role.error(this.#status.text) : role.success(this.#status.text));
    } else if (this.#busy) {
      lines.push(role.meta("working…"));
    } else {
      lines.push(role.meta("Committed present deliveries of this Session, newest first."));
    }
    lines.push("");
    const mode = this.#mode;
    if (mode.kind === "actions") {
      lines.push(...this.#entry(mode.record, true));
      lines.push("");
      lines.push(this.#actionLine(0, mode.selected, "Open in the system application", this.#availability.open));
      lines.push(this.#actionLine(1, mode.selected, "Save original bytes to a local path…", this.#availability.save));
      return lines.map((line) => truncateToWidth(line, width));
    }
    if (mode.kind === "destination") {
      lines.push(...this.#entry(mode.record, true));
      lines.push("");
      lines.push(role.meta(mode.prefilled
        ? "Save to (client-local path; existing files are never overwritten):"
        : "The delivered name cannot be shown as typed; enter a client-local path (existing files are never overwritten):"));
      this.#destination.focused = this.focused && !this.#busy;
      lines.push(...this.#destination.render(width));
      return lines.map((line) => truncateToWidth(line, width));
    }
    const rows = this.#rowCount();
    if (this.#records.length === 0) {
      lines.push(role.meta(this.#next === undefined
        ? "No committed deliveries in this Session."
        : "No committed deliveries in the loaded history."));
    }
    const budget = Math.max(ENTRY_ROWS, this.#bodyHeight - lines.length);
    const { start, end } = windowAroundSelected(rows, this.#selected, budget, () => ENTRY_ROWS);
    for (let index = start; index < end; index += 1) {
      const record = this.#records[index];
      if (record === undefined) {
        const label = this.#loading ? "loading older history…" : "Load older history";
        lines.push(`${index === this.#selected ? role.accent("›") : " "} ${role.meta(label)}`, "");
        continue;
      }
      lines.push(...this.#entry(record, index === this.#selected));
    }
    return lines.map((line) => truncateToWidth(line, width));
  }

  #entry(record: DeliveryRecord, selected: boolean): string[] {
    const file = record.file;
    const name = sanitizeField(file.name);
    const description = file.description ? `  ${sanitizeField(file.description)}` : "";
    return [
      `${selected ? role.accent("›") : " "} ${style.bold(name)}  ${role.meta(sanitizeField(deliveryType(file)))}${description}`,
      `   ${role.meta(`${sanitizeField(file.path)} · ${sanitizeField(file.scope.conversation_id)} · ${sanitizeField(deliveryIdentity(record))} · ${this.#summary()}`)}`,
    ];
  }

  #summary(): string {
    const open = this.#availability.open === undefined ? "open" : "open unavailable";
    const save = this.#availability.save === undefined ? "save" : "save unavailable";
    return `${open}, ${save}`;
  }

  #actionLine(index: number, selected: number, label: string, unavailable: string | undefined): string {
    const marker = index === selected ? role.accent("›") : " ";
    return unavailable === undefined
      ? `${marker} ${label}`
      : `${marker} ${role.meta(`${label} — unavailable: ${unavailable}`)}`;
  }

  #request(action: DeliveryAction, record: DeliveryRecord): void {
    const unavailable = this.#availability[action];
    if (unavailable !== undefined) {
      this.#status = { level: "error", text: `${action === "open" ? "Open" : "Save"} unavailable: ${unavailable}` };
      this.onChange?.();
      return;
    }
    if (action === "save") {
      // The original name becomes an editable path only when it renders as
      // itself; a sanitized display string never silently names a file.
      const prefilled = isRenderableField(record.file.name);
      this.#destination.setValue(prefilled ? record.file.name : "");
      this.#setMode({ kind: "destination", record, prefilled });
      return;
    }
    this.#start("open", record);
  }

  #submitDestination(value: string): void {
    if (this.#mode.kind !== "destination" || this.#busy) return;
    if (value.trim().length === 0) return;
    this.#start("save", this.#mode.record, value);
  }

  #start(action: DeliveryAction, record: DeliveryRecord, destination?: string): void {
    this.#operation += 1;
    this.#busy = true;
    this.#status = undefined;
    this.onChange?.();
    this.onAction?.(this.#operation, action, record, destination);
  }

  #setMode(mode: Mode): void {
    this.#mode = mode;
    this.onChange?.();
  }

  #rowCount(): number {
    return this.#records.length + (this.#next === undefined ? 0 : 1);
  }

  #move(delta: number, rows: number): void {
    if (rows === 0) return;
    this.#selected = Math.min(rows - 1, Math.max(0, this.#selected + delta));
    this.onChange?.();
  }

  #loadMore(): void {
    if (this.#next === undefined || this.#loading) return;
    this.#loading = true;
    this.onChange?.();
    this.onLoadMore?.();
  }
}
