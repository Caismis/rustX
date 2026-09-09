import {
  Input,
  Key,
  Markdown,
  matchesKey,
  truncateToWidth,
  visibleWidth,
  wrapTextWithAnsi,
} from "@earendil-works/pi-tui";

import type {
  AnswerSpecification,
  InteractionRequester,
  OptionSpecification,
  QuestionSpecification,
  QuestionnaireAnswer,
  QuestionnaireAnswerEntry,
  QuestionnaireResponse,
  QuestionnaireSpecification,
} from "../../protocol/types.ts";
import { markdownTheme, role } from "../theme.ts";
import type { PopupContent } from "./popup-frame.ts";

const MAX_CUSTOM_ANSWER_CHARS = 4096;
const MAX_TEXT_ANSWER_CHARS = 4096;
const PREVIEW_CONTENT_LINES = 12;

const NATIVE_POPUP_TITLE = "Ask user · questionnaire";
const POPUP_FOOTER =
  "Tab/Shift+Tab tabs · arrows rows · Enter choose/submit · Space toggle · PageUp/PageDown preview · Esc decline · Ctrl+C cancel attempt";

/** The typed rows one question's answer surface presents. */
type RowKind =
  | { kind: "option"; optionIndex: number }
  | { kind: "boolean"; value: boolean }
  | { kind: "scalar" }
  | { kind: "custom" };

export interface QuestionnaireOverlayOptions {
  interactionId: string;
  questionnaire: QuestionnaireSpecification;
  /**
   * The canonical identity of the tool that asked. It is projected by the
   * runtime, never inferred here, so an MCP-originated prompt can name its
   * server and a native `ask_user` prompt is never labelled as MCP.
   */
  requester: InteractionRequester;
  /**
   * The routed source label ("Question from reviewer"), when the interaction
   * did not originate in the conversation this client is attached to.
   *
   * It is deliberately independent of {@link requester}: this says *where* the
   * interaction came from, the requester says *who* asked.
   */
  sourceLabel?: string;
  onSubmit: (response: QuestionnaireResponse) => void;
  onDecline: () => void;
  onInterrupt: () => void;
  onChange?: () => void;
}

type RenderedBody = {
  lines: string[];
  focusLine: number;
};

/** The declared options of a choice question, or an empty list. */
export function choiceOptions(question: QuestionSpecification): OptionSpecification[] {
  const answer = question.answer;
  return answer.type === "single_choice" || answer.type === "multi_choice"
    ? answer.options
    : [];
}

/** Whether a question accepts a free-text answer outside its options. */
function allowsCustom(answer: AnswerSpecification): boolean {
  return (answer.type === "single_choice" || answer.type === "multi_choice") &&
    answer.allow_custom;
}

/** The ordered rows one question's answer surface presents. */
function rowsOf(question: QuestionSpecification): RowKind[] {
  const answer = question.answer;
  switch (answer.type) {
    case "text":
    case "number":
    case "integer":
      return [{ kind: "scalar" }];
    case "boolean":
      return [{ kind: "boolean", value: true }, { kind: "boolean", value: false }];
    default: {
      const rows: RowKind[] = answer.options.map((_, optionIndex) => ({
        kind: "option" as const,
        optionIndex,
      }));
      if (answer.allow_custom) rows.push({ kind: "custom" });
      return rows;
    }
  }
}

/**
 * The human-readable statement of a multi-choice question's own bounds.
 *
 * The bounds come from the request; the client only explains and pre-checks
 * them. The runtime re-validates every submission against the same facts.
 */
export function selectionBoundsLabel(min: number, max: number): string {
  if (min === max) return `Select exactly ${min}`;
  if (min === 0) return `Select up to ${max}`;
  return `Select ${min}–${max}`;
}

/** The inclusive bounds of the runtime's canonical `Integer` domain. */
const I64_MIN = -(2n ** 63n);
const I64_MAX = 2n ** 63n - 1n;

/**
 * Validates one scalar draft against its declared answer shape.
 *
 * Returns `undefined` when the draft is acceptable. This is **UX only**: the
 * runtime validates the very same facts again and stays authoritative, and an
 * invalid draft keeps the user inside the interaction rather than failing the
 * tool invocation.
 */
export function scalarValidationError(
  answer: AnswerSpecification,
  draft: string,
): string | undefined {
  const value = draft.trim();
  switch (answer.type) {
    case "text": {
      const length = [...draft].length;
      if (answer.min_length !== undefined && length < answer.min_length) {
        return `Enter at least ${answer.min_length} characters.`;
      }
      if (answer.max_length !== undefined && length > answer.max_length) {
        return `Enter at most ${answer.max_length} characters.`;
      }
      if (answer.format === "date" && !isCalendarDate(draft)) {
        return "Enter a date as YYYY-MM-DD.";
      }
      if (answer.format === "date_time" && Number.isNaN(Date.parse(draft))) {
        return "Enter an RFC 3339 date-time.";
      }
      if (answer.format === "uri" && !isAbsoluteUri(draft)) {
        return "Enter an absolute URI.";
      }
      return undefined;
    }
    case "number": {
      if (!/^-?(\d+(\.\d*)?|\.\d+)([eE][+-]?\d+)?$/.test(value)) {
        return "Enter a number.";
      }
      const parsed = Number(value);
      if (!Number.isFinite(parsed)) return "Enter a finite number.";
      // The runtime's Number domain is the finite binary64, which is exactly
      // what this `Number` holds — so parsing here changes no value the
      // runtime would have accepted. The one case where it *would* is a whole
      // number binary64 cannot hold exactly: the runtime refuses those rather
      // than rounding them into range, so the client refuses them too instead
      // of submitting a value the user did not type.
      if (/^-?\d+$/.test(value) && BigInt(value) !== BigInt(parsed)) {
        return "Enter a number this runtime can represent exactly.";
      }
      if (answer.minimum !== undefined && parsed < answer.minimum) {
        return `Enter a number at least ${answer.minimum}.`;
      }
      if (answer.maximum !== undefined && parsed > answer.maximum) {
        return `Enter a number at most ${answer.maximum}.`;
      }
      return undefined;
    }
    case "integer": {
      // Never `Number(...)`: the runtime's Integer domain is the exact i64,
      // and a JavaScript number would silently round every value above 2^53 —
      // the whole reason this field crosses the wire as decimal text. The
      // draft is compared as a `BigInt` and submitted as the user's own
      // digits; the runtime performs the one authoritative parse.
      if (!/^-?\d+$/.test(value)) return "Enter a whole number.";
      const parsed = BigInt(value);
      if (parsed < I64_MIN || parsed > I64_MAX) {
        return "Enter a whole number inside the 64-bit range.";
      }
      if (answer.minimum !== undefined && parsed < BigInt(answer.minimum)) {
        return `Enter a whole number at least ${answer.minimum}.`;
      }
      if (answer.maximum !== undefined && parsed > BigInt(answer.maximum)) {
        return `Enter a whole number at most ${answer.maximum}.`;
      }
      return undefined;
    }
    default:
      return undefined;
  }
}

function isCalendarDate(value: string): boolean {
  const match = /^(\d{4})-(\d{2})-(\d{2})$/.exec(value);
  if (match === null) return false;
  const [, year, month, day] = match;
  const date = new Date(`${year}-${month}-${day}T00:00:00Z`);
  return !Number.isNaN(date.getTime()) &&
    date.getUTCFullYear() === Number(year) &&
    date.getUTCMonth() + 1 === Number(month) &&
    date.getUTCDate() === Number(day);
}

function isAbsoluteUri(value: string): boolean {
  try {
    // eslint-disable-next-line no-new
    new URL(value);
    return true;
  } catch {
    return false;
  }
}

/**
 * The presentation label of an MCP or native requester.
 *
 * The words come from canonical facts, so a native `ask_user` prompt is never
 * described as MCP and an MCP prompt always names its server.
 */
export function requesterLines(requester: InteractionRequester): string[] {
  if (requester.origin === "builtin") {
    return [`Requested by ${requester.tool_name}`];
  }
  return [
    `Requested by MCP server: ${requester.origin.mcp.server_id}`,
    `Tool: ${requester.tool_name}`,
  ];
}

/**
 * The compact requester name used by queue rows and activity lines.
 *
 * It is derived from canonical origin facts, so a native tool is never
 * presented as MCP and an MCP tool always carries its server identity.
 */
export function requesterName(requester: InteractionRequester): string {
  return requester.origin === "builtin"
    ? requester.tool_name
    : `mcp:${requester.origin.mcp.server_id}/${requester.tool_name}`;
}

/** The popup frame title for one requester. */
export function requesterTitle(requester: InteractionRequester): string {
  return requester.origin === "builtin"
    ? NATIVE_POPUP_TITLE
    : `MCP elicitation · ${requester.origin.mcp.server_id}`;
}

/**
 * One ephemeral questionnaire surface.
 *
 * The overlay owns only focus, selections, and unsubmitted drafts. Pi's
 * single-line Input owns editing semantics, including bracketed paste, Kitty
 * printable input, grapheme-aware cursor movement, and deletion. The runtime
 * remains authoritative: the surface sends a response once, and disappears
 * when the pending interaction leaves the projection.
 *
 * Every question is rendered **according to its declared answer shape**: a
 * text/number/integer question shows an input field and no manufactured
 * option rows, a boolean question shows an explicit true/false choice, and a
 * choice question shows a custom-answer row only when the request allows one.
 */
export class QuestionnaireOverlay implements PopupContent {
  readonly interactionId: string;
  readonly questionnaire: QuestionnaireSpecification;
  readonly requester: InteractionRequester;
  readonly #sourceLabel: string | undefined;
  readonly #onSubmit: (response: QuestionnaireResponse) => void;
  readonly #onDecline: () => void;
  readonly #onInterrupt: () => void;
  readonly #onChange: (() => void) | undefined;
  /** Selected option indices, per question. */
  readonly #selected: Array<Set<number>>;
  /** The chosen boolean, per question. */
  readonly #boolean: Array<boolean | undefined>;
  /** The scalar or custom-answer draft, per question. */
  readonly #draft: Array<string | undefined>;
  readonly #inputs: Input[];
  /** Whether the user has interacted with a question at all. */
  readonly #touched: boolean[];
  /**
   * Whether the user has explicitly committed this question's scalar field.
   *
   * This is the **answer-presence bit**, and it is deliberately independent of
   * the draft's length. For a text question an omitted answer and an explicit
   * empty string are different facts: the first says nothing was answered, the
   * second is a real `Text("")` the runtime accepts whenever `min_length` is
   * absent or `0`, and which reaches an MCP server as `""` rather than a
   * decline. Inferring presence from `draft.length > 0` would collapse the two
   * and make an intentional empty answer unsubmittable.
   *
   * It is set by editing the field — typing a character and erasing it again
   * is an explicit empty answer — and by pressing Enter on the field, which
   * commits the current draft as it stands. An untouched field is never
   * committed, so a blank questionnaire still submits nothing.
   */
  readonly #committed: boolean[];
  #tab = 0;
  #row = 0;
  #submitting = false;
  #bodyHeight = 24;
  #previewOffset = 0;
  #previewLineCount = 0;
  #previewContentViewport = PREVIEW_CONTENT_LINES;
  #notice: string | undefined;

  constructor(options: QuestionnaireOverlayOptions) {
    this.interactionId = options.interactionId;
    this.questionnaire = options.questionnaire;
    this.requester = options.requester;
    this.#sourceLabel = options.sourceLabel;
    this.#onSubmit = options.onSubmit;
    this.#onDecline = options.onDecline;
    this.#onInterrupt = options.onInterrupt;
    this.#onChange = options.onChange;
    this.#selected = options.questionnaire.questions.map(() => new Set<number>());
    this.#boolean = options.questionnaire.questions.map(() => undefined);
    this.#draft = options.questionnaire.questions.map(() => undefined);
    this.#inputs = options.questionnaire.questions.map(() => new Input());
    this.#touched = options.questionnaire.questions.map(() => false);
    this.#committed = options.questionnaire.questions.map(() => false);
  }

  invalidate(): void {
    // The overlay state is intentionally retained across redraws. Attachment
    // replacement creates a new instance and therefore discards only drafts.
  }

  /** The popup's frame title, which names an MCP requester when there is one. */
  popupTitle(): string {
    return requesterTitle(this.requester);
  }

  /** The popup's help line, contained by the frame below the body. */
  popupFooter(): string[] {
    return [POPUP_FOOTER];
  }

  /** Sets the finite body-row budget the PopupFrame allocated for this pass. */
  setBodyHeight(height: number): void {
    const next = Math.max(1, Math.floor(height));
    if (next !== this.#bodyHeight) this.#bodyHeight = next;
  }

  /** Marks an explicit submission/decline as in flight. */
  beginSubmitting(): void {
    this.#submitting = true;
    this.#changed();
  }

  /** Re-enables the surface when the Runtime Client rejects the response. */
  submissionFailed(): void {
    this.#submitting = false;
    this.#notice = "The runtime refused that response. Correct it and submit again.";
    this.#changed();
  }

  /**
   * The explicit Esc decline: a typed `declined` response, exactly once.
   *
   * Declining settles this questionnaire; it never cancels the attempt and
   * never touches any other pending interaction.
   */
  decline(): void {
    if (this.#submitting) return;
    this.beginSubmitting();
    this.#onDecline();
  }

  handleInput(data: string): void {
    if (this.#submitting) return;
    if (matchesKey(data, Key.ctrl("c"))) {
      this.#onInterrupt();
      return;
    }
    if (matchesKey(data, Key.escape)) {
      this.decline();
      return;
    }
    if (matchesKey(data, Key.shift("tab"))) {
      this.#moveTab(-1);
      return;
    }
    if (matchesKey(data, Key.tab)) {
      this.#moveTab(1);
      return;
    }
    if (matchesKey(data, Key.pageUp)) {
      this.#scrollPreview(-1);
      return;
    }
    if (matchesKey(data, Key.pageDown)) {
      this.#scrollPreview(1);
      return;
    }
    if (matchesKey(data, Key.up)) {
      this.#moveRow(-1);
      return;
    }
    if (matchesKey(data, Key.down)) {
      this.#moveRow(1);
      return;
    }

    if (this.#tab < this.questionnaire.questions.length) {
      this.#handleQuestionInput(data);
      return;
    }

    if (matchesKey(data, Key.enter) && this.#row === 0) {
      this.#submit();
    }
  }

  #handleQuestionInput(data: string): void {
    const question = this.questionnaire.questions[this.#tab]!;
    const rows = rowsOf(question);
    const row = rows[this.#row];
    if (row === undefined) return;
    if (row.kind === "scalar" || row.kind === "custom") {
      if (row.kind === "custom" && matchesKey(data, Key.enter)) {
        if ((this.#draft[this.#tab] ?? "").length > 0) {
          this.#selected[this.#tab]!.clear();
          this.#changed();
        }
        return;
      }
      if (row.kind === "scalar" && matchesKey(data, Key.enter)) {
        // Enter commits the field exactly as it stands. That is the one
        // deterministic way to answer a text question with the empty string
        // without typing and erasing a character first.
        this.#committed[this.#tab] = true;
        this.#touched[this.#tab] = true;
        this.#notice = undefined;
        this.#changed();
        return;
      }
      // Delegate every editing path to Pi's established primitive. This
      // includes raw bracketed-paste markers, multi-character batches, Kitty
      // printable sequences, Unicode graphemes, cursor movement, and
      // backspace/delete. Only the questionnaire's focus/cancellation keys
      // are intercepted above.
      this.#handleDraftInput(data, row.kind === "custom");
      return;
    }
    if (row.kind === "boolean") {
      if (matchesKey(data, Key.enter) || matchesKey(data, Key.space)) {
        this.#boolean[this.#tab] = row.value;
        this.#touched[this.#tab] = true;
        this.#changed();
      }
      return;
    }
    const answer = question.answer;
    const multi = answer.type === "multi_choice";
    if (matchesKey(data, Key.space) && multi) {
      this.#toggleOption(row.optionIndex);
      return;
    }
    if (matchesKey(data, Key.enter)) {
      if (multi) {
        this.#toggleOption(row.optionIndex);
      } else {
        this.#selected[this.#tab]!.clear();
        this.#selected[this.#tab]!.add(row.optionIndex);
        this.#clearDraft(this.#tab);
        this.#touched[this.#tab] = true;
        this.#changed();
      }
    }
  }

  render(width: number): string[] {
    const safeWidth = Math.max(1, Math.floor(width));
    const header = [
      fitLine(role.meta(`interaction ${this.interactionId}`), safeWidth),
      ...requesterLines(this.requester).map((line) =>
        fitLine(role.meta(line), safeWidth)
      ),
      ...(this.#sourceLabel === undefined
        ? []
        : [fitLine(role.meta(this.#sourceLabel), safeWidth)]),
      ...this.#renderTabs(safeWidth),
    ];
    const available = Math.max(1, this.#bodyHeight - header.length);
    const body = this.#submitting
      ? { lines: [role.pending("Submitting response…")], focusLine: 0 }
      : this.#tab === this.questionnaire.questions.length
        ? this.#renderReview(safeWidth, available)
        : this.#renderQuestion(safeWidth, available);
    const lines = [...header, ...body.lines];

    // Tiny test terminals cannot display the full body. Keep the contract
    // explicit even there: no line escapes the finite rectangle the frame
    // allocated. Normal popups have enough room for header and body.
    return lines.slice(0, this.#bodyHeight).map((line) => fitLine(line, safeWidth));
  }

  #renderTabs(width: number): string[] {
    const labels = this.questionnaire.questions.map((question, index) =>
      index === this.#tab ? role.accent(`[${question.header}]`) : role.meta(question.header),
    );
    labels.push(
      this.#tab === this.questionnaire.questions.length
        ? role.accent("[Review / submit]")
        : role.meta("Review / submit"),
    );

    const lines: string[] = [];
    let current = "";
    for (const label of labels) {
      const next = current.length === 0 ? label : `${current}  ${label}`;
      if (current.length > 0 && visibleWidth(next) > width) {
        lines.push(fitLine(current, width));
        current = label;
      } else {
        current = next;
      }
    }
    if (current.length > 0) lines.push(fitLine(current, width));
    return lines.length > 0 ? lines : [""];
  }

  #renderQuestion(width: number, viewportHeight: number): RenderedBody {
    const question = this.questionnaire.questions[this.#tab]!;
    const questionLines = wrapStyled(role.strong(question.question), width);
    const guidance = this.#guidance(question);
    const intro = [
      "",
      ...questionLines.map((line) => fitLine(line, width)),
      ...(guidance === undefined ? [] : [fitLine(role.meta(guidance), width)]),
    ];
    const preview = this.#focusedPreview(question);

    if (preview !== undefined && width >= 100) {
      const gutter = 1;
      const leftWidth = Math.max(1, Math.floor((width - gutter) * 0.52));
      const rightWidth = Math.max(1, width - gutter - leftWidth);
      // Reserve the intro rows before allocating the split-pane viewport.
      // When the question itself is taller than the available body, keep its
      // beginning visible and give the panes the remaining bounded space.
      const minimumPaneHeight = Math.min(3, viewportHeight);
      const introHeight = Math.min(
        intro.length,
        Math.max(0, viewportHeight - minimumPaneHeight),
      );
      const visibleIntro = intro.slice(0, introHeight);
      const paneHeight = Math.max(1, viewportHeight - visibleIntro.length);
      const rows = this.#renderQuestionRows(question, leftWidth);
      const left = clipToFocus(
        rows.lines,
        rows.focusLine,
        paneHeight,
      );
      const right = this.#renderPreview(
        preview,
        rightWidth,
        Math.max(1, paneHeight - 2),
      );
      const visibleRight = right.slice(0, paneHeight);
      const count = Math.max(left.lines.length, visibleRight.length);
      const combined = Array.from({ length: count }, (_, index) => {
        const leftLine = padLine(left.lines[index] ?? "", leftWidth);
        const rightLine = fitLine(visibleRight[index] ?? "", rightWidth);
        return `${leftLine} ${rightLine}`;
      });
      return clipToFocus(
        [...visibleIntro, ...combined],
        visibleIntro.length + left.focusLine,
        viewportHeight,
      );
    }

    const rows = this.#renderQuestionRows(question, width);
    const list = clipToFocus(
      [...intro, ...rows.lines],
      intro.length + rows.focusLine,
      preview === undefined
        ? viewportHeight
        : narrowOptionViewport(viewportHeight),
    );
    if (preview === undefined) {
      this.#previewLineCount = 0;
      this.#previewOffset = 0;
      this.#previewContentViewport = PREVIEW_CONTENT_LINES;
      return list;
    }

    const previewViewport = narrowPreviewViewport(viewportHeight);
    if (previewViewport === 0) {
      this.#previewLineCount = 0;
      this.#previewOffset = 0;
      this.#previewContentViewport = PREVIEW_CONTENT_LINES;
      return list;
    }
    const previewLines = this.#renderPreview(
      preview,
      width,
      Math.max(1, previewViewport - 2),
    );
    return {
      lines: [...list.lines, ...previewLines].slice(0, viewportHeight),
      focusLine: list.focusLine,
    };
  }

  /** The concise explanation of what a legal answer to this question is. */
  #guidance(question: QuestionSpecification): string | undefined {
    const answer = question.answer;
    switch (answer.type) {
      case "multi_choice":
        return selectionBoundsLabel(answer.min_selected, answer.max_selected);
      case "integer":
        return boundsSentence("Whole number", answer.minimum, answer.maximum);
      case "number":
        return boundsSentence("Number", answer.minimum, answer.maximum);
      case "text":
        return textGuidance(answer);
      default:
        return undefined;
    }
  }

  #renderQuestionRows(question: QuestionSpecification, width: number): RenderedBody {
    const lines: string[] = [];
    let focusLine = 0;
    const rows = rowsOf(question);
    const options = choiceOptions(question);
    const multi = question.answer.type === "multi_choice";
    for (const [index, row] of rows.entries()) {
      if (index === this.#row) focusLine = lines.length;
      const marker = index === this.#row ? role.accent("›") : " ";
      if (row.kind === "option") {
        const option = options[row.optionIndex]!;
        const selected = this.#selected[this.#tab]!.has(row.optionIndex);
        const box = multi
          ? selected ? "[x]" : "[ ]"
          : selected ? "●" : "○";
        lines.push(fitLine(`${marker} ${box} ${role.strong(option.label)}`, width));
        const descriptionWidth = Math.max(1, width - 2);
        for (const line of wrapStyled(role.meta(option.description), descriptionWidth)) {
          lines.push(fitLine(`  ${line}`, width));
        }
        continue;
      }
      if (row.kind === "boolean") {
        const chosen = this.#boolean[this.#tab] === row.value;
        lines.push(
          fitLine(
            `${marker} ${chosen ? "●" : "○"} ${role.strong(row.value ? "Yes" : "No")}`,
            width,
          ),
        );
        lines.push(
          fitLine(`  ${role.meta(row.value ? "true" : "false")}`, width),
        );
        continue;
      }
      // A scalar input row, or the custom-answer row of a choice question
      // that explicitly allows one. No option rows are manufactured for a
      // free-form question, and no custom row exists for a bounded one.
      lines.push(
        fitLine(
          `${marker} ${role.accent(row.kind === "custom" ? "Type something." : scalarPrompt(question.answer))}`,
          width,
        ),
      );
      const input = this.#inputs[this.#tab]!;
      input.focused = index === this.#row;
      const inputLine = input.render(Math.max(1, width - 2))[0] ?? "";
      input.focused = false;
      const draft = this.#draft[this.#tab];
      if (draft !== undefined || index === this.#row) {
        lines.push(fitLine(`  ${inputLine}`, width));
      } else {
        lines.push(
          fitLine(
            `  ${role.meta(row.kind === "custom" ? "Enter a custom answer" : "Enter a value")}`,
            width,
          ),
        );
      }
    }

    const error = this.#answerError(this.#tab);
    if (error !== undefined) {
      lines.push(fitLine(role.warning(error), width));
    }
    return { lines, focusLine };
  }

  #renderPreview(
    preview: string,
    width: number,
    contentViewport = PREVIEW_CONTENT_LINES,
  ): string[] {
    const markdown = new Markdown(preview, 0, 0, markdownTheme);
    const allLines = markdown.render(Math.max(1, width));
    const lines = allLines.length > 0 ? allLines : ["(empty preview)"];
    this.#previewLineCount = lines.length;
    this.#previewContentViewport = Math.max(1, Math.floor(contentViewport));
    const maxOffset = Math.max(0, lines.length - this.#previewContentViewport);
    this.#previewOffset = Math.max(
      0,
      Math.min(this.#previewOffset, maxOffset),
    );
    const visible = lines.slice(
      this.#previewOffset,
      this.#previewOffset + this.#previewContentViewport,
    );
    const first = this.#previewOffset + 1;
    const last = Math.min(
      this.#previewOffset + visible.length,
      this.#previewLineCount,
    );
    return [
      fitLine(role.strong("Preview"), width),
      fitLine(
        role.meta(
          `lines ${first}-${last} of ${this.#previewLineCount} · PageUp/PageDown scroll`,
        ),
        width,
      ),
      ...visible.map((line) => fitLine(line, width)),
    ];
  }

  #renderReview(width: number, viewportHeight: number): RenderedBody {
    const lines: string[] = ["", fitLine(role.strong("Review your answers"), width)];
    for (const [index, question] of this.questionnaire.questions.entries()) {
      const error = this.#answerError(index);
      const answer = this.#answerFor(index);
      lines.push(
        fitLine(
          error !== undefined
            ? role.warning(`${question.header}: ${error}`)
            : answer === undefined
              ? role.warning(`${question.header}: unanswered`)
              : role.success(`${question.header}: ${answer}`),
          width,
        ),
      );
    }
    if (this.#notice !== undefined) {
      lines.push(fitLine(role.warning(this.#notice), width));
    }
    lines.push(
      "",
      fitLine(
        `${this.#row === 0 ? role.accent("›") : " "} ${role.strong(this.#hasAnswers() ? "Submit answers" : "Submit (decline)")}`,
        width,
      ),
      fitLine(
        role.meta(
          width < 60 ? "Esc declines" : "Esc explicitly declines this questionnaire",
        ),
        width,
      ),
    );
    return clipToFocus(lines, lines.length - 2, viewportHeight);
  }

  #focusedPreview(question: QuestionSpecification): string | undefined {
    const row = rowsOf(question)[this.#row];
    if (row === undefined || row.kind !== "option") return undefined;
    return choiceOptions(question)[row.optionIndex]?.preview;
  }

  /**
   * The concise reason this question's current draft is not submittable, or
   * `undefined` when it is. An unanswered question is not an error: a partial
   * questionnaire is a legal submission.
   */
  #answerError(index: number): string | undefined {
    const question = this.questionnaire.questions[index]!;
    const answer = question.answer;
    const draft = this.#draft[index];
    if (answer.type === "text") {
      // An uncommitted field is simply unanswered, which is legal. A committed
      // one is validated as it stands — including the empty string, which a
      // positive `min_length` must still refuse.
      if (!this.#committed[index]) return undefined;
      return scalarValidationError(answer, draft ?? "");
    }
    if (answer.type === "number" || answer.type === "integer") {
      // There is no empty number: an empty draft is an unanswered question.
      if (draft === undefined || draft.length === 0) return undefined;
      return scalarValidationError(answer, draft);
    }
    if (draft !== undefined && draft.length > 0) {
      return [...draft].length > MAX_CUSTOM_ANSWER_CHARS
        ? `Enter at most ${MAX_CUSTOM_ANSWER_CHARS} characters.`
        : undefined;
    }
    if (answer.type === "multi_choice") {
      const count = this.#selected[index]!.size;
      if (count === 0 && !this.#touched[index]) return undefined;
      if (count < answer.min_selected) {
        return selectionBoundsLabel(answer.min_selected, answer.max_selected);
      }
      if (count > answer.max_selected) {
        return selectionBoundsLabel(answer.min_selected, answer.max_selected);
      }
    }
    return undefined;
  }

  #answerFor(index: number): string | undefined {
    const question = this.questionnaire.questions[index]!;
    const answer = question.answer;
    const draft = this.#draft[index];
    if (answer.type === "text") {
      if (!this.#committed[index]) return undefined;
      return draft === undefined || draft.length === 0 ? "(empty)" : draft;
    }
    if (answer.type === "number" || answer.type === "integer") {
      return draft !== undefined && draft.length > 0 ? draft : undefined;
    }
    if (answer.type === "boolean") {
      const value = this.#boolean[index];
      return value === undefined ? undefined : value ? "Yes (true)" : "No (false)";
    }
    if (draft !== undefined && draft.length > 0) return `custom: ${draft}`;
    const selected = this.#selected[index]!;
    if (selected.size === 0) {
      return answer.type === "multi_choice" && answer.min_selected === 0 &&
          this.#touched[index]
        ? "(none)"
        : undefined;
    }
    return answer.options
      .filter((_, optionIndex) => selected.has(optionIndex))
      .map((option) => option.label)
      .join(", ");
  }

  #hasAnswers(): boolean {
    return this.questionnaire.questions.some((_, index) => this.#answerFor(index) !== undefined);
  }

  /** Submits, unless a draft is invalid — an invalid draft keeps the user here. */
  #submit(): void {
    const invalid = this.questionnaire.questions.findIndex(
      (_, index) => this.#answerError(index) !== undefined,
    );
    if (invalid >= 0) {
      this.#notice = `Correct ${this.questionnaire.questions[invalid]!.header} before submitting.`;
      this.#tab = invalid;
      this.#row = 0;
      this.#changed();
      return;
    }
    this.#notice = undefined;
    this.#submitting = true;
    this.#onSubmit(this.#submission());
    this.#changed();
  }

  #submission(): QuestionnaireResponse {
    const answers: QuestionnaireAnswerEntry[] = [];
    for (const [questionIndex, question] of this.questionnaire.questions.entries()) {
      const answer = this.#typedAnswer(questionIndex, question);
      if (answer !== undefined) answers.push({ question_index: questionIndex, answer });
    }
    return { type: "submitted", value: { answers } };
  }

  #typedAnswer(
    index: number,
    question: QuestionSpecification,
  ): QuestionnaireAnswer | undefined {
    const answer = question.answer;
    const draft = this.#draft[index];
    const filled = draft !== undefined && draft.length > 0;
    switch (answer.type) {
      case "text":
        // Presence, not length: a committed field answers, even with "".
        return this.#committed[index]
          ? { type: "text", value: { value: draft ?? "" } }
          : undefined;
      case "number":
        // The runtime's Number domain is the finite binary64 this `Number`
        // holds, and the draft was already proven exactly representable, so
        // the parsed value is the one the runtime validates and emits.
        return filled ? { type: "number", value: { value: Number(draft.trim()) } } : undefined;
      case "integer":
        // The user's own digits cross the wire. Converting to a JavaScript
        // number here would round every answer above 2^53 before the runtime
        // ever performed its authoritative parse.
        return filled ? { type: "integer", value: { value: draft.trim() } } : undefined;
      case "boolean": {
        const value = this.#boolean[index];
        return value === undefined ? undefined : { type: "boolean", value: { value } };
      }
      default: {
        // A custom answer exists only where the request allows one, so this
        // branch cannot fabricate a response shape the runtime would refuse.
        if (filled && allowsCustom(answer)) {
          return { type: "custom", value: { answer: draft } };
        }
        const selected = [...this.#selected[index]!].sort((a, b) => a - b);
        if (answer.type === "single_choice") {
          return selected.length === 0
            ? undefined
            : { type: "option", value: { option_index: selected[0]! } };
        }
        if (selected.length === 0 && !(answer.min_selected === 0 && this.#touched[index])) {
          return undefined;
        }
        return { type: "options", value: { option_indices: selected } };
      }
    }
  }

  #handleDraftInput(data: string, custom: boolean): void {
    const index = this.#tab;
    const input = this.#inputs[index]!;
    const before = input.getValue();
    input.handleInput(data);
    const value = input.getValue();
    const bounded = scalarPrefix(
      value,
      custom ? MAX_CUSTOM_ANSWER_CHARS : MAX_TEXT_ANSWER_CHARS,
    );
    if (bounded !== value) input.setValue(bounded);
    const next = bounded.length > 0 ? bounded : undefined;
    if (next !== this.#draft[index]) {
      this.#draft[index] = next;
      this.#touched[index] = true;
      // Editing the field is an explicit answer, including editing it back to
      // empty: the user who erased their draft answered with "".
      this.#committed[index] = true;
      if (next !== undefined && custom) this.#selected[index]!.clear();
    }
    // Cursor-only edits do not change the draft but still need a redraw.
    if (before !== input.getValue() || data.length > 0) this.#changed();
  }

  #clearDraft(index: number): void {
    this.#draft[index] = undefined;
    this.#inputs[index]!.setValue("");
  }

  /**
   * Toggles one multi-choice option, refusing to select above the request's
   * own `max_selected`. Preventing the obviously invalid selection is UX; the
   * runtime enforces the same bound authoritatively.
   */
  #toggleOption(optionIndex: number): void {
    const question = this.questionnaire.questions[this.#tab]!;
    const selected = this.#selected[this.#tab]!;
    this.#touched[this.#tab] = true;
    if (selected.has(optionIndex)) {
      selected.delete(optionIndex);
    } else {
      const answer = question.answer;
      if (answer.type === "multi_choice" && selected.size >= answer.max_selected) {
        this.#notice = selectionBoundsLabel(answer.min_selected, answer.max_selected);
        this.#changed();
        return;
      }
      selected.add(optionIndex);
    }
    this.#clearDraft(this.#tab);
    this.#changed();
  }

  #moveTab(delta: number): void {
    const count = this.questionnaire.questions.length + 1;
    this.#tab = (this.#tab + delta + count) % count;
    this.#row = 0;
    this.#previewOffset = 0;
    this.#previewLineCount = 0;
    this.#previewContentViewport = PREVIEW_CONTENT_LINES;
    this.#changed();
  }

  #moveRow(delta: number): void {
    const max = this.#tab === this.questionnaire.questions.length
      ? 0
      : Math.max(0, rowsOf(this.questionnaire.questions[this.#tab]!).length - 1);
    const next = Math.max(0, Math.min(max, this.#row + delta));
    if (next === this.#row) return;
    this.#row = next;
    this.#previewOffset = 0;
    this.#previewLineCount = 0;
    this.#previewContentViewport = PREVIEW_CONTENT_LINES;
    this.#changed();
  }

  #scrollPreview(direction: number): void {
    if (this.#tab >= this.questionnaire.questions.length || this.#previewLineCount <= 0) {
      return;
    }
    const page = Math.max(1, this.#previewContentViewport);
    const maxOffset = Math.max(0, this.#previewLineCount - this.#previewContentViewport);
    const next = Math.max(
      0,
      Math.min(maxOffset, this.#previewOffset + direction * page),
    );
    if (next === this.#previewOffset) return;
    this.#previewOffset = next;
    this.#changed();
  }

  #changed(): void {
    this.#onChange?.();
  }
}

function scalarPrompt(answer: AnswerSpecification): string {
  switch (answer.type) {
    case "number":
      return "Enter a number.";
    case "integer":
      return "Enter a whole number.";
    default:
      return "Type your answer.";
  }
}

function boundsSentence(
  noun: string,
  minimum: number | string | undefined,
  maximum: number | string | undefined,
): string | undefined {
  if (minimum !== undefined && maximum !== undefined) {
    return `${noun} between ${minimum} and ${maximum}`;
  }
  if (minimum !== undefined) return `${noun} at least ${minimum}`;
  if (maximum !== undefined) return `${noun} at most ${maximum}`;
  return undefined;
}

function textGuidance(
  answer: Extract<AnswerSpecification, { type: "text" }>,
): string | undefined {
  const parts: string[] = [];
  if (answer.min_length !== undefined && answer.max_length !== undefined) {
    parts.push(`${answer.min_length}–${answer.max_length} characters`);
  } else if (answer.min_length !== undefined) {
    parts.push(`at least ${answer.min_length} characters`);
  } else if (answer.max_length !== undefined) {
    parts.push(`at most ${answer.max_length} characters`);
  }
  if (answer.format === "date") parts.push("as YYYY-MM-DD");
  if (answer.format === "date_time") parts.push("as an RFC 3339 date-time");
  if (answer.format === "uri") parts.push("as an absolute URI");
  return parts.length === 0 ? undefined : `Text ${parts.join(", ")}`;
}

function scalarPrefix(value: string, maximum: number): string {
  if ([...value].length <= maximum) return value;
  let result = "";
  let count = 0;
  for (const scalar of value) {
    if (count >= maximum) break;
    result += scalar;
    count += 1;
  }
  return result;
}

function wrapStyled(value: string, width: number): string[] {
  return wrapTextWithAnsi(value, Math.max(1, width));
}

function clipToFocus(lines: string[], focusLine: number, viewportHeight: number): RenderedBody {
  const height = Math.max(1, Math.floor(viewportHeight));
  if (lines.length === 0) return { lines: [""], focusLine: 0 };
  const safeFocus = Math.max(0, Math.min(focusLine, lines.length - 1));
  const maxOffset = Math.max(0, lines.length - height);
  const offset = Math.max(
    0,
    Math.min(maxOffset, safeFocus - Math.floor(height / 2)),
  );
  return {
    lines: lines.slice(offset, offset + height),
    focusLine: safeFocus - offset,
  };
}

function narrowPreviewViewport(viewportHeight: number): number {
  if (viewportHeight < 4) return 0;
  return Math.min(14, Math.max(3, Math.floor(viewportHeight / 2)));
}

function narrowOptionViewport(viewportHeight: number): number {
  const previewHeight = narrowPreviewViewport(viewportHeight);
  return Math.max(1, viewportHeight - previewHeight);
}

function fitLine(value: string, width: number): string {
  return truncateToWidth(value, Math.max(1, width), "…");
}

function padLine(value: string, width: number): string {
  const fitted = fitLine(value, width);
  return `${fitted}${" ".repeat(Math.max(0, width - visibleWidth(fitted)))}`;
}
