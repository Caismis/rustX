import {
  Input,
  Key,
  Markdown,
  matchesKey,
  truncateToWidth,
  visibleWidth,
  wrapTextWithAnsi,
  type Component,
} from "@earendil-works/pi-tui";

import type {
  QuestionSpecification,
  QuestionnaireAnswer,
  QuestionnaireAnswerEntry,
  QuestionnaireResponse,
  QuestionnaireSpecification,
} from "../../protocol/types.ts";
import { markdownTheme, role } from "../theme.ts";

const MAX_CUSTOM_ANSWER_CHARS = 4096;
const PREVIEW_CONTENT_LINES = 12;

export interface QuestionnaireOverlayOptions {
  interactionId: string;
  questionnaire: QuestionnaireSpecification;
  onSubmit: (response: QuestionnaireResponse) => void;
  onDecline: () => void;
  onInterrupt: () => void;
  onChange?: () => void;
}

type RenderedBody = {
  lines: string[];
  focusLine: number;
};

/**
 * One ephemeral questionnaire surface.
 *
 * The overlay owns only focus, selections, and unsubmitted custom-answer
 * drafts. Pi's single-line Input owns editing semantics, including bracketed
 * paste, Kitty printable input, grapheme-aware cursor movement, and deletion.
 * The runtime remains authoritative: the surface sends a response once, and
 * disappears when the pending interaction leaves the projection.
 */
export class QuestionnaireOverlay implements Component {
  readonly interactionId: string;
  readonly questionnaire: QuestionnaireSpecification;
  readonly #onSubmit: (response: QuestionnaireResponse) => void;
  readonly #onDecline: () => void;
  readonly #onInterrupt: () => void;
  readonly #onChange: (() => void) | undefined;
  readonly #selected: Array<Set<string>>;
  readonly #custom: Array<string | undefined>;
  readonly #customInputs: Input[];
  #tab = 0;
  #row = 0;
  #submitting = false;
  #viewportHeight = 24;
  #previewOffset = 0;
  #previewLineCount = 0;
  #previewContentViewport = PREVIEW_CONTENT_LINES;

  constructor(options: QuestionnaireOverlayOptions) {
    this.interactionId = options.interactionId;
    this.questionnaire = options.questionnaire;
    this.#onSubmit = options.onSubmit;
    this.#onDecline = options.onDecline;
    this.#onInterrupt = options.onInterrupt;
    this.#onChange = options.onChange;
    this.#selected = options.questionnaire.questions.map(() => new Set<string>());
    this.#custom = options.questionnaire.questions.map(() => undefined);
    this.#customInputs = options.questionnaire.questions.map(() => new Input());
  }

  invalidate(): void {
    // The overlay state is intentionally retained across redraws. Attachment
    // replacement creates a new instance and therefore discards only drafts.
  }

  /** Sets the self-managed viewport supplied by the owning app/overlay host. */
  setViewportHeight(height: number): void {
    const next = Math.max(1, Math.floor(height));
    if (next !== this.#viewportHeight) this.#viewportHeight = next;
  }

  /** Marks an explicit submission/decline as in flight. */
  beginSubmitting(): void {
    this.#submitting = true;
    this.#changed();
  }

  /** Re-enables the surface when the Runtime Client rejects the response. */
  submissionFailed(): void {
    this.#submitting = false;
    this.#changed();
  }

  handleInput(data: string): void {
    if (this.#submitting) return;
    if (matchesKey(data, Key.ctrl("c"))) {
      this.#onInterrupt();
      return;
    }
    if (matchesKey(data, Key.escape)) {
      this.beginSubmitting();
      this.#onDecline();
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
      const question = this.questionnaire.questions[this.#tab]!;
      const customRow = question.options.length;
      if (this.#row === customRow) {
        if (matchesKey(data, Key.enter)) {
          if ((this.#custom[this.#tab] ?? "").length > 0) {
            this.#selected[this.#tab]!.clear();
            this.#changed();
          }
          return;
        }
        // Delegate every editing path to Pi's established primitive. This
        // includes raw bracketed-paste markers, multi-character batches,
        // Kitty printable sequences, Unicode graphemes, cursor movement, and
        // backspace/delete. Only the questionnaire's focus/cancellation keys
        // are intercepted above.
        this.#handleCustomInput(data);
        return;
      }
      if (matchesKey(data, Key.space) && question.multi_select) {
        this.#toggleOption(question.options[this.#row]!.label);
        return;
      }
      if (matchesKey(data, Key.enter)) {
        if (question.multi_select) {
          this.#toggleOption(question.options[this.#row]!.label);
        } else {
          this.#selected[this.#tab]!.clear();
          this.#selected[this.#tab]!.add(question.options[this.#row]!.label);
          this.#clearCustom(this.#tab);
          this.#changed();
        }
      }
      return;
    }

    if (matchesKey(data, Key.enter) && this.#row === 0) {
      this.#submitting = true;
      this.#onSubmit(this.#submission());
      this.#changed();
    }
  }

  render(width: number): string[] {
    const safeWidth = Math.max(1, Math.floor(width));
    const header = [
      fitLine(role.strong("Ask user · questionnaire"), safeWidth),
      fitLine(role.meta(`interaction ${this.interactionId}`), safeWidth),
      ...this.#renderTabs(safeWidth),
    ];
    const footer = [
      fitLine(
        role.meta(
          "Tab/Shift+Tab tabs · arrows rows · Enter choose/submit · Space toggle · PageUp/PageDown preview · Esc decline · Ctrl+C cancel attempt",
        ),
        safeWidth,
      ),
    ];
    const available = Math.max(1, this.#viewportHeight - header.length - footer.length);
    const body = this.#submitting
      ? { lines: [role.pending("Submitting response…")], focusLine: 0 }
      : this.#tab === this.questionnaire.questions.length
        ? this.#renderReview(safeWidth, available)
        : this.#renderQuestion(safeWidth, available);
    const lines = [...header, ...body.lines, ...footer];

    // Tiny test terminals cannot display the full frame. Keep the contract
    // explicit even there: no line escapes the component's requested height
    // or width. Normal overlays have enough room for header, body, and help.
    return lines.slice(0, this.#viewportHeight).map((line) => fitLine(line, safeWidth));
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
    const intro = ["", ...questionLines.map((line) => fitLine(line, width))];
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

  #renderQuestionRows(question: QuestionSpecification, width: number): RenderedBody {
    const lines: string[] = [];
    let focusLine = 0;
    for (const [index, option] of question.options.entries()) {
      if (index === this.#row) focusLine = lines.length;
      const selected = this.#selected[this.#tab]!.has(option.label);
      const marker = question.multi_select
        ? selected ? "[x]" : "[ ]"
        : selected ? "●" : "○";
      lines.push(
        fitLine(
          `${index === this.#row ? role.accent("›") : " "} ${marker} ${role.strong(option.label)}`,
          width,
        ),
      );
      const descriptionWidth = Math.max(1, width - 2);
      for (const line of wrapStyled(role.meta(option.description), descriptionWidth)) {
        lines.push(fitLine(`  ${line}`, width));
      }
    }

    const customRow = question.options.length;
    if (this.#row === customRow) focusLine = lines.length;
    lines.push(
      fitLine(
        `${this.#row === customRow ? role.accent("›") : " "} ${role.accent("Type something.")}`,
        width,
      ),
    );
    const draft = this.#custom[this.#tab];
    const input = this.#customInputs[this.#tab]!;
    input.focused = this.#row === customRow;
    const inputLine = input.render(Math.max(1, width - 2))[0] ?? "";
    input.focused = false;
    if (draft !== undefined || this.#row === customRow) {
      lines.push(fitLine(`  ${inputLine}`, width));
    } else {
      lines.push(fitLine(`  ${role.meta("Enter a custom answer")}`, width));
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
      const answer = this.#answerFor(index);
      lines.push(
        fitLine(
          answer === undefined
            ? role.warning(`${question.header}: unanswered`)
            : role.success(`${question.header}: ${answer}`),
          width,
        ),
      );
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
    if (this.#row >= question.options.length) return undefined;
    return question.options[this.#row]?.preview;
  }

  #answerFor(index: number): string | undefined {
    const custom = this.#custom[index];
    if (custom !== undefined && custom.length > 0) return `custom: ${custom}`;
    const question = this.questionnaire.questions[index]!;
    const labels = question.options
      .filter((option) => this.#selected[index]!.has(option.label))
      .map((option) => option.label);
    return labels.length === 0 ? undefined : labels.join(", ");
  }

  #hasAnswers(): boolean {
    return this.questionnaire.questions.some((_, index) => this.#answerFor(index) !== undefined);
  }

  #submission(): QuestionnaireResponse {
    const answers: QuestionnaireAnswerEntry[] = [];
    for (const [questionIndex, question] of this.questionnaire.questions.entries()) {
      const custom = this.#custom[questionIndex];
      let answer: QuestionnaireAnswer | undefined;
      if (custom !== undefined && custom.length > 0) {
        answer = { type: "custom", value: { answer: custom } };
      } else {
        const selected = question.options
          .filter((option) => this.#selected[questionIndex]!.has(option.label))
          .map((option) => option.label);
        if (selected.length > 0) {
          answer = question.multi_select
            ? { type: "multiple_option", value: { selected } }
            : { type: "single_option", value: { label: selected[0]! } };
        }
      }
      if (answer !== undefined) answers.push({ question_index: questionIndex, answer });
    }
    return { type: "submitted", value: { answers } };
  }

  #handleCustomInput(data: string): void {
    const index = this.#tab;
    const input = this.#customInputs[index]!;
    const before = input.getValue();
    input.handleInput(data);
    const value = input.getValue();
    const bounded = scalarPrefix(value, MAX_CUSTOM_ANSWER_CHARS);
    if (bounded !== value) input.setValue(bounded);
    const next = bounded.length > 0 ? bounded : undefined;
    if (next !== this.#custom[index]) {
      this.#custom[index] = next;
      if (next !== undefined) this.#selected[index]!.clear();
    }
    // Cursor-only edits do not change the draft but still need a redraw.
    if (before !== input.getValue() || data.length > 0) this.#changed();
  }

  #clearCustom(index: number): void {
    this.#custom[index] = undefined;
    this.#customInputs[index]!.setValue("");
  }

  #toggleOption(label: string): void {
    const selected = this.#selected[this.#tab]!;
    if (selected.has(label)) selected.delete(label);
    else selected.add(label);
    this.#clearCustom(this.#tab);
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
      : this.questionnaire.questions[this.#tab]!.options.length;
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
