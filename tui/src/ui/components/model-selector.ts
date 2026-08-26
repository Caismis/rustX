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
 * through `model_catalog_get`. This component does not read `models.jsonc`,
 * does not contact a provider, does not know what "sonnet" means, and does
 * not invent a reasoning scale: a reasoning-capable model that declares no
 * profiles is shown as exactly that, because there is no universal
 * off/low/medium/high and asserting one would make the client a second
 * model-configuration authority.
 *
 * Search runs over the same published catalog facts the rows display — the
 * model reference, the protocol, the modalities, the capability flags, the
 * reasoning profile ids, the limits — and over nothing else. See
 * {@link searchTerms}: there is no `claude`-means-Messages alias and no
 * family taxonomy, because the client is not an authority on what a model is.
 *
 * Selection produces a `CatalogModelView` and nothing else. Applying it is
 * one `model_set` request, owned by the dispatcher; this component never
 * mutates session state.
 */

import {
  fuzzyMatch,
  Input,
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
  onSelect?: (model: CatalogModelView) => void;
  onCancel?: () => void;
  onChange?: () => void;

  readonly #models: CatalogModelView[];
  readonly #sessionModel: SessionModelView;
  readonly #attempt: AttemptPresentation | undefined;
  readonly #searchInput: Input;
  #query = "";
  #selected = 0;

  constructor(options: ModelSelectorOptions) {
    this.#models = options.models;
    this.#sessionModel = options.sessionModel;
    this.#attempt = options.attempt;
    this.#searchInput = new Input();
    this.#searchInput.onEscape = () => this.onCancel?.();
    this.#searchInput.onSubmit = () => this.#selectCurrent();
  }

  get focused(): boolean {
    return this.#searchInput.focused;
  }

  set focused(value: boolean) {
    this.#searchInput.focused = value;
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
   * deterministic ranking of that same list over the published facts of
   * {@link searchTerms}.
   */
  visibleModels(): CatalogModelView[] {
    return filterModels(this.#models, this.#query);
  }

  /** The highlighted model, or `undefined` when nothing matches. */
  selectedModel(): CatalogModelView | undefined {
    return this.visibleModels()[this.#selected];
  }

  setQuery(query: string): void {
    this.#query = query;
    this.#searchInput.setValue(query);
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
    // Let pi-tui's native input own cursor movement, insertion, deletion,
    // word editing, undo, kill/yank, and bracketed paste. The selector owns
    // only the model filter and list navigation; it never reimplements text
    // editing policy.
    this.#searchInput.handleInput(data);
    const query = this.#searchInput.getValue();
    if (query !== this.#query) {
      this.#query = query;
      this.#selected = 0;
      this.onChange?.();
    }
  }

  render(width: number): string[] {
    const visible = this.visibleModels();
    const searchWidth = Math.max(1, width - plainWidth("Search: "));
    const search = this.#searchInput.render(searchWidth)[0] ?? "";
    const lines: string[] = [
      role.strong("Select model"),
      truncate(`${role.meta("Search:")} ${search}`, width),
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

  #selectCurrent(): void {
    const model = this.visibleModels()[this.#selected];
    if (model !== undefined) {
      this.onSelect?.(model);
    }
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

// ---------------------------------------------------------------------------
// Search
// ---------------------------------------------------------------------------

/**
 * The searchable terms of one catalog row, the model reference first.
 *
 * Every term is a field the runtime published through `model_catalog_get`, or
 * the row label this component already renders for one. Nothing here is a
 * provider alias, a family name, or a guess: there is no `claude` term, no
 * `gpt` term, and no mapping from either to a protocol, because inventing one
 * would make this client a second authority on what a model *is*.
 *
 * Session state is deliberately absent too. `configured` and `effective` are
 * facts about the session, not about the catalog entry, and searching them as
 * though they were row metadata would let a row match on something it does
 * not describe.
 *
 * The head and the tail are matched differently — see {@link filterModels}.
 */
export function searchTerms(model: CatalogModelView): string[] {
  const capabilities = model.effectiveCapabilities;
  const terms = [
    // The head: the model reference.
    model.model,
    // The tail: published metadata. Both spellings the runtime and this
    // overlay use for one protocol fact, so `responses` and
    // `openai_responses` both find the same rows.
    model.protocol,
    protocolLabel(model.protocol),
    ...capabilities.inputModalities,
    ...capabilities.outputModalities,
    // Capability terms are added only when the capability is present: a
    // negative term would let a query match a model precisely because it
    // cannot do the thing the query named.
    ...(capabilities.toolCalls ? ["tools"] : []),
    ...(capabilities.reasoning ? ["reasoning"] : []),
    ...(model.reasoningProfiles ?? []).map((profile) => profile.id),
    ...(model.defaultReasoningProfile === undefined
      ? []
      : [model.defaultReasoningProfile]),
    tokens(model.contextWindow),
    tokens(model.maxOutputTokens),
  ];
  return [...new Set(terms.filter((term) => term.length > 0))];
}

/** How much worse a metadata match ranks than a good reference match. */
const METADATA_SCORE = 0;
/** How much worse an infix metadata match ranks than a whole-term one. */
const INFIX_PENALTY = 10;

/**
 * The deterministic ranking of a catalog against one query.
 *
 * Two kinds of term, matched two ways, because they are two kinds of string.
 *
 * ```text
 * model reference   free-form, arbitrarily long   fuzzy subsequence
 * published metadata  a short closed vocabulary   case-insensitive containment
 * ```
 *
 * Fuzzy matching a short label fabricates: `image` is a subsequence of
 * `anthropic_messages`, so a subsequence rule would return a text-only row
 * for a query about image input. Containment is precise, still forgiving
 * enough for prefixes and infixes, and — unlike a corpus built by gluing
 * every term into one string — a match remains a statement about a single
 * fact the catalog actually published.
 *
 * Every query token must match, so tokens narrow rather than widen. A
 * whitespace- or slash-separated query is tokenized the way Pi's own filter
 * tokenizes one.
 */
export function filterModels(
  models: CatalogModelView[],
  query: string,
): CatalogModelView[] {
  const queryTokens = query.split(/[\s/]+/).filter((token) => token.length > 0);
  if (queryTokens.length === 0) {
    // The catalog's own order, untouched.
    return models;
  }
  const scored: Array<{ model: CatalogModelView; score: number; index: number }> =
    [];
  models.forEach((model, index) => {
    let score = 0;
    for (const token of queryTokens) {
      const best = tokenScore(token, model);
      if (best === undefined) {
        return;
      }
      score += best;
    }
    scored.push({ model, score, index });
  });
  // Ties break on catalog position, so the ordering is total and stable.
  scored.sort((a, b) => a.score - b.score || a.index - b.index);
  return scored.map((entry) => entry.model);
}

/** The best score one query token achieves against one row, if any. */
function tokenScore(
  token: string,
  model: CatalogModelView,
): number | undefined {
  const [reference = "", ...metadata] = searchTerms(model);
  const byReference = fuzzyMatch(token, reference);
  let best = byReference.matches ? byReference.score : undefined;
  const needle = token.toLowerCase();
  for (const term of metadata) {
    const haystack = term.toLowerCase();
    if (!haystack.includes(needle)) {
      continue;
    }
    const score =
      METADATA_SCORE + (haystack.length === needle.length ? 0 : INFIX_PENALTY);
    if (best === undefined || score < best) {
      best = score;
    }
  }
  return best;
}
