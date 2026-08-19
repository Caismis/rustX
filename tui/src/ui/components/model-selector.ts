/**
 * The searchable model selector.
 *
 * ```text
 * Select model
 * Search: sonnet▌
 *
 * ❯ alpha/claude-sonnet-x                              current
 *     Messages · 200k ctx · 8k out · tools · text,image
 *     reasoning: low medium high (default medium)
 *   beta/gpt-x
 *     Responses · 272k ctx · 16k out · tools · text
 *     reasoning: unsupported
 *
 * ↑↓ navigate · Enter select · Esc close
 * ```
 *
 * Every value shown is a `CatalogModelView` field the runtime published
 * through `model_catalog_get`. This component does not read `models.json`,
 * does not contact a provider, does not know what "sonnet" means, and does
 * not invent a reasoning scale: a reasoning-capable model that declares no
 * profiles is shown as exactly that, because there is no universal
 * off/low/medium/high and asserting one would make the client a second
 * model-configuration authority.
 *
 * Selection produces a `CatalogModelView` and nothing else. Applying it is
 * one `model_set` request, owned by the dispatcher; this component never
 * mutates session state.
 */

import {
  fuzzyFilter,
  matchesKey,
  type Component,
  type Focusable,
} from "@earendil-works/pi-tui";

import type {
  CatalogModelView,
  SessionModelView,
} from "../../protocol/types.ts";
import type { AttemptPresentation } from "../../presentation/state.ts";
import { role, style, plainText, plainWidth } from "../theme.ts";

/** How many rows the list shows before it scrolls. */
const VISIBLE_ROWS = 6;

export interface ModelSelectorOptions {
  models: CatalogModelView[];
  /** The session's own configured/effective model. */
  sessionModel: SessionModelView;
  /** The active attempt, when one exists, for the frozen-model notice. */
  attempt?: AttemptPresentation;
}

export class ModelSelector implements Component, Focusable {
  focused = false;
  onSelect?: (model: CatalogModelView) => void;
  onCancel?: () => void;
  onChange?: () => void;

  readonly #models: CatalogModelView[];
  readonly #sessionModel: SessionModelView;
  readonly #attempt: AttemptPresentation | undefined;
  #query = "";
  #selected = 0;

  constructor(options: ModelSelectorOptions) {
    this.#models = options.models;
    this.#sessionModel = options.sessionModel;
    this.#attempt = options.attempt;
  }

  /** The current search text. */
  get query(): string {
    return this.#query;
  }

  /**
   * The filtered models, in the order they are shown.
   *
   * With an empty query this is the catalog's own order — the runtime decides
   * how its catalog is ordered, not the client. With a query it is the
   * deterministic fuzzy ranking of that same list.
   */
  visibleModels(): CatalogModelView[] {
    if (this.#query.length === 0) {
      return this.#models;
    }
    return fuzzyFilter(this.#models, this.#query, (model) => model.model);
  }

  /** The highlighted model, or `undefined` when nothing matches. */
  selectedModel(): CatalogModelView | undefined {
    return this.visibleModels()[this.#selected];
  }

  setQuery(query: string): void {
    this.#query = query;
    this.#selected = 0;
    this.onChange?.();
  }

  invalidate(): void {
    // Nothing is cached: the component renders from its inputs every time.
  }

  handleInput(data: string): void {
    const visible = this.visibleModels();
    if (matchesKey(data, "escape")) {
      this.onCancel?.();
      return;
    }
    if (matchesKey(data, "enter")) {
      const model = visible[this.#selected];
      if (model !== undefined) {
        this.onSelect?.(model);
      }
      return;
    }
    if (matchesKey(data, "up")) {
      this.#move(-1, visible.length);
      return;
    }
    if (matchesKey(data, "down")) {
      this.#move(1, visible.length);
      return;
    }
    if (matchesKey(data, "backspace")) {
      this.setQuery(this.#query.slice(0, -1));
      return;
    }
    // Everything else that is plain printable text goes to the search box.
    // Control and escape sequences are ignored rather than typed, so an
    // unhandled key never corrupts the query.
    if (isPrintable(data)) {
      this.setQuery(this.#query + data);
    }
  }

  render(width: number): string[] {
    const visible = this.visibleModels();
    const lines: string[] = [
      role.strong("Select model"),
      `${role.meta("Search:")} ${this.#query}${this.focused ? role.accent("▌") : ""}`,
      "",
    ];

    if (visible.length === 0) {
      lines.push(role.meta(`no model matches ${JSON.stringify(this.#query)}`));
    } else {
      const start = Math.max(
        0,
        Math.min(
          this.#selected - Math.floor(VISIBLE_ROWS / 2),
          visible.length - VISIBLE_ROWS,
        ),
      );
      const window = visible.slice(start, start + VISIBLE_ROWS);
      window.forEach((model, index) => {
        lines.push(...this.#renderRow(model, start + index === this.#selected, width));
      });
      if (visible.length > VISIBLE_ROWS) {
        lines.push(
          role.meta(`${this.#selected + 1}/${visible.length}`),
        );
      }
    }

    lines.push("");
    lines.push(...this.#renderContext());
    lines.push(role.meta("↑↓ navigate · Enter select · Esc close"));
    return lines;
  }

  #move(delta: number, length: number): void {
    if (length === 0) {
      return;
    }
    this.#selected = (this.#selected + delta + length) % length;
    this.onChange?.();
  }

  #renderRow(
    model: CatalogModelView,
    selected: boolean,
    width: number,
  ): string[] {
    const marker = selected ? role.accent("❯") : " ";
    const name = selected ? style.bold(model.model) : model.model;
    const current =
      model.model === this.#sessionModel.configured.model
        ? ` ${role.success("current")}`
        : "";
    const head = `${marker} ${name}${current}`;
    const rows = [truncate(head, width)];
    // Details only for the highlighted row, so the list stays scannable.
    if (selected) {
      rows.push(truncate(`    ${role.meta(capabilityLine(model))}`, width));
      rows.push(truncate(`    ${role.meta(reasoningLine(model))}`, width));
    }
    return rows;
  }

  /**
   * The configured/effective/attempt-frozen model context.
   *
   * The three identities are distinct rustX facts and the selector says so
   * explicitly. A user changing the model while an attempt runs must not be
   * left believing the running attempt switched with it.
   */
  #renderContext(): string[] {
    const lines: string[] = [
      role.meta(
        `configured ${this.#sessionModel.configured.model} · effective ${this.#sessionModel.effective.model}`,
      ),
    ];
    const attempt = this.#attempt;
    if (attempt !== undefined && attempt.phase.type !== "settled") {
      lines.push(
        role.pending(
          `the running attempt stays on ${attempt.model.primary.model}; a change applies to the next attempt`,
        ),
      );
    }
    return lines;
  }
}

/** The published effective capability of one catalog entry, compactly. */
export function capabilityLine(model: CatalogModelView): string {
  const capabilities = model.effectiveCapabilities;
  const parts = [
    protocolLabel(model.protocol),
    `${tokens(model.contextWindow)} ctx`,
    `${tokens(model.maxOutputTokens)} out`,
    capabilities.toolCalls ? "tools" : "no tools",
    `in ${capabilities.inputModalities.join("/") || "none"}`,
  ];
  return parts.join(" · ");
}

/**
 * The reasoning profiles the catalog published, exactly as published.
 *
 * Three genuinely different cases, kept different: unsupported, supported
 * with selectable profiles, and supported with none.
 */
export function reasoningLine(model: CatalogModelView): string {
  if (!model.effectiveCapabilities.reasoning) {
    return "reasoning: unsupported";
  }
  const profiles = model.reasoningProfiles ?? [];
  if (profiles.length === 0) {
    return "reasoning: supported, no selectable profile";
  }
  const names = profiles
    .map((profile) => (profile.enabled ? profile.id : `${profile.id} (off)`))
    .join(" ");
  const fallback =
    model.defaultReasoningProfile === undefined
      ? ""
      : ` (default ${model.defaultReasoningProfile})`;
  return `reasoning: ${names}${fallback}`;
}

/** The protocol name, shortened for a list row. Cosmetic only. */
function protocolLabel(protocol: CatalogModelView["protocol"]): string {
  switch (protocol) {
    case "openai_chat_completions":
      return "Chat Completions";
    case "openai_responses":
      return "Responses";
    case "anthropic_messages":
      return "Messages";
    default:
      return protocol;
  }
}

function tokens(value: number): string {
  if (value >= 1_000_000) {
    return `${Math.round(value / 1_000_000)}M`;
  }
  if (value >= 1_000) {
    return `${Math.round(value / 1_000)}k`;
  }
  return String(value);
}

/** Whether one input chunk is text a search box should accept. */
function isPrintable(data: string): boolean {
  if (data.length === 0) {
    return false;
  }
  for (const character of data) {
    const code = character.codePointAt(0) ?? 0;
    if (code < 0x20 || code === 0x7f) {
      return false;
    }
  }
  return true;
}

/**
 * Bounds one row to the overlay width.
 *
 * Styling is dropped rather than sliced when a row has to be cut: slicing
 * through an SGR sequence would leak escape bytes into the terminal.
 */
function truncate(text: string, width: number): string {
  if (plainWidth(text) <= width) {
    return text;
  }
  const plain = plainText(text);
  return `${[...plain].slice(0, Math.max(0, width - 1)).join("")}…`;
}
