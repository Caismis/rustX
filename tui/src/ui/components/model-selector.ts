/**
 * The searchable model selector.
 *
 * ```text
 * Select model
 * Search: sonnet▌
 *
 * ❯ alpha/claude-sonnet-x                              configured
 *     Messages · 200k ctx · 8k out · tools · text,image
 *     catalog reasoning: low medium high (catalog default medium)
 *   beta/gpt-x                                         effective
 *     Responses · 272k ctx · 16k out · tools · text
 *     catalog reasoning: unsupported
 *
 * configured  alpha/claude-sonnet-x
 * effective   beta/gpt-x
 * attempt     gamma/older-x · frozen at admission
 * configured reasoning  profile high
 * effective reasoning   on (profile medium)
 *
 * ↑↓ navigate · Enter select · Esc close
 * ```
 *
 * Two kinds of fact share this overlay and are never merged. A row describes
 * the **catalog**: what a model offers, and which reasoning profile the
 * catalog would fall back to. The context block below describes the
 * **session and the attempt**: what was configured, what is effective, and
 * what a running attempt froze. A catalog default is never labelled as the
 * current configuration, and a row is never labelled `current` when
 * `current` could mean either configured or effective.
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
  ModelRef,
  SessionModelView,
} from "../../protocol/types.ts";
import type { AttemptPresentation } from "../../presentation/state.ts";
import {
  describeConfiguredReasoning,
  describeReasoning,
} from "../../presentation/selectors.ts";
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
    const roles = this.#rolesOf(model.model);
    const head = `${marker} ${name}${roles.length === 0 ? "" : ` ${role.success(roles.join(" · "))}`}`;
    const rows = [truncate(head, width)];
    // Details only for the highlighted row, so the list stays scannable.
    if (selected) {
      rows.push(truncate(`    ${role.meta(capabilityLine(model))}`, width));
      rows.push(truncate(`    ${role.meta(reasoningLine(model))}`, width));
    }
    return rows;
  }

  /**
   * Which session/attempt roles one catalog row currently holds.
   *
   * `current` is used only when there is exactly one thing it can mean —
   * configured, effective, and any attempt all point at this row. The moment
   * they diverge every role is named, so a row is never ambiguous about
   * whether it is what was asked for or what is actually in use.
   */
  #rolesOf(model: ModelRef): string[] {
    const configured = this.#sessionModel.configured.model;
    const effective = this.#sessionModel.effective.model;
    const attempt = this.#attempt?.model.primary.model;
    const unified =
      configured === effective && (attempt === undefined || attempt === effective);
    if (unified) {
      return model === configured ? ["current"] : [];
    }
    const roles: string[] = [];
    if (model === configured) {
      roles.push("configured");
    }
    if (model === effective) {
      roles.push("effective");
    }
    if (attempt !== undefined && model === attempt) {
      roles.push("attempt");
    }
    return roles;
  }

  /**
   * The configured/effective/attempt-frozen model context.
   *
   * The three identities are distinct rustX facts and the selector says so
   * explicitly. A user changing the model while an attempt runs must not be
   * left believing the running attempt switched with it.
   */
  #renderContext(): string[] {
    const session = this.#sessionModel;
    const configured = session.configured.model;
    const effective = session.effective.model;
    const lines: string[] = [];
    if (configured === effective) {
      lines.push(role.meta(`configured · effective  ${configured}`));
    } else {
      // Two distinct runtime facts, so two distinct lines. Collapsing them
      // into one "current" would hide that the session cannot use what it
      // asked for.
      lines.push(role.meta(`configured  ${configured}`));
      lines.push(role.meta(`effective   ${effective}`));
    }

    const attempt = this.#attempt;
    if (attempt !== undefined) {
      const frozen = attempt.model.primary.model;
      lines.push(
        attempt.phase.type === "settled"
          ? role.meta(`attempt     ${frozen} · frozen at admission (settled)`)
          : role.pending(
              `attempt     ${frozen} · frozen at admission; a change applies to the next attempt`,
            ),
      );
    }

    // The session's own reasoning configuration, which is neither a catalog
    // default nor something this overlay may change: only `model_set` does.
    lines.push(
      role.meta(
        `configured reasoning  ${describeConfiguredReasoning(session.configured)}`,
      ),
    );
    lines.push(
      role.meta(`effective reasoning   ${describeReasoning(session.effective)}`),
    );
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
 * The reasoning profiles the *catalog* published, exactly as published.
 *
 * Three genuinely different cases, kept different: unsupported, supported
 * with selectable profiles, and supported with none.
 */
export function reasoningLine(model: CatalogModelView): string {
  if (!model.effectiveCapabilities.reasoning) {
    return "catalog reasoning: unsupported";
  }
  const profiles = model.reasoningProfiles ?? [];
  if (profiles.length === 0) {
    return "catalog reasoning: supported, no selectable profile";
  }
  const names = profiles
    .map((profile) => (profile.enabled ? profile.id : `${profile.id} (off)`))
    .join(" ");
  // Explicitly the *catalog's* fallback. It is not evidence that the session
  // configured this profile, and the context block below says what it did.
  const fallback =
    model.defaultReasoningProfile === undefined
      ? ""
      : ` (catalog default ${model.defaultReasoningProfile})`;
  return `catalog reasoning: ${names}${fallback}`;
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
