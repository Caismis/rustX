import {
  Markdown,
  matchesKey,
  parseKey,
  type Component,
} from "@earendil-works/pi-tui";

import type {
  QuestionnaireAnswer,
  QuestionnaireAnswerEntry,
  QuestionnaireResponse,
  QuestionnaireSpecification,
} from "../../protocol/types.ts";
import { markdownTheme, role, style, plainWidth } from "../theme.ts";

const MAX_CUSTOM_ANSWER_CHARS = 4096;

export interface QuestionnaireOverlayOptions {
  interactionId: string;
  questionnaire: QuestionnaireSpecification;
  onSubmit: (response: QuestionnaireResponse) => void;
  onDecline: () => void;
  onInterrupt: () => void;
  onChange?: () => void;
}

/**
 * One ephemeral questionnaire surface.
 *
 * The overlay owns only focus, selections, and unsubmitted custom-answer
 * drafts. The runtime remains authoritative: the surface sends a response
 * once, and disappears when the pending interaction leaves the projection.
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
  #tab = 0;
  #row = 0;
  #submitting = false;

  constructor(options: QuestionnaireOverlayOptions) {
    this.interactionId = options.interactionId;
    this.questionnaire = options.questionnaire;
    this.#onSubmit = options.onSubmit;
    this.#onDecline = options.onDecline;
    this.#onInterrupt = options.onInterrupt;
    this.#onChange = options.onChange;
    this.#selected = options.questionnaire.questions.map(() => new Set<string>());
    this.#custom = options.questionnaire.questions.map(() => undefined);
  }

  invalidate(): void {
    // The overlay state is intentionally retained across redraws. Attachment
    // replacement creates a new instance and therefore discards only drafts.
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
    if (matchesKey(data, "ctrl+c")) {
      this.#onInterrupt();
      return;
    }
    if (matchesKey(data, "escape")) {
      this.beginSubmitting();
      this.#onDecline();
      return;
    }
    if (matchesKey(data, "shift+tab")) {
      this.#moveTab(-1);
      return;
    }
    if (matchesKey(data, "tab")) {
      this.#moveTab(1);
      return;
    }
    if (matchesKey(data, "up")) {
      this.#moveRow(-1);
      return;
    }
    if (matchesKey(data, "down")) {
      this.#moveRow(1);
      return;
    }
    if (this.#tab < this.questionnaire.questions.length) {
      const question = this.questionnaire.questions[this.#tab]!;
      const customRow = question.options.length;
      if (this.#row === customRow) {
        if (matchesKey(data, "backspace") || matchesKey(data, "delete")) {
          const draft = this.#custom[this.#tab] ?? "";
          this.#custom[this.#tab] = [...draft].slice(0, -1).join("") || undefined;
          this.#changed();
          return;
        }
        // `parseKey` names a literal space as `space`, but a space is still a
        // valid character while the client-owned custom answer is focused.
        const printable = data === " " ? " " : parseKey(data);
        if (
          printable !== undefined &&
          printable !== "tab" &&
          printable !== "enter" &&
          printable !== "backspace" &&
          printable !== "delete" &&
          [...printable].length === 1
        ) {
          const draft = `${this.#custom[this.#tab] ?? ""}${printable}`;
          if ([...draft].length <= MAX_CUSTOM_ANSWER_CHARS) {
            this.#custom[this.#tab] = draft;
            this.#selected[this.#tab]!.clear();
            this.#changed();
          }
          return;
        }
      }
      if (matchesKey(data, "space") && question.multi_select && this.#row < customRow) {
        this.#toggleOption(question.options[this.#row]!.label);
        return;
      }
      if (matchesKey(data, "enter")) {
        if (this.#row < customRow) {
          if (question.multi_select) {
            this.#toggleOption(question.options[this.#row]!.label);
          } else {
            this.#selected[this.#tab]!.clear();
            this.#selected[this.#tab]!.add(question.options[this.#row]!.label);
            this.#custom[this.#tab] = undefined;
            this.#changed();
          }
        } else if ((this.#custom[this.#tab] ?? "").length > 0) {
          this.#selected[this.#tab]!.clear();
          this.#changed();
        }
      }
      return;
    }
    if (matchesKey(data, "enter") && this.#row === 0) {
      this.#submitting = true;
      this.#onSubmit(this.#submission());
      this.#changed();
    }
  }

  render(width: number): string[] {
    const safeWidth = Math.max(36, width);
    const lines: string[] = [
      role.strong("Ask user · questionnaire"),
      role.meta(`interaction ${this.interactionId}`),
      this.#renderTabs(safeWidth),
    ];
    if (this.#submitting) {
      lines.push("", role.pending("Submitting response…"));
      return lines;
    }
    if (this.#tab === this.questionnaire.questions.length) {
      lines.push(...this.#renderReview(safeWidth));
    } else {
      lines.push(...this.#renderQuestion(safeWidth));
    }
    lines.push(
      "",
      role.meta("Tab/Shift+Tab tabs · arrows rows · Enter choose/submit · Space toggle · Esc decline · Ctrl+C cancel attempt"),
    );
    return lines;
  }

  #renderTabs(width: number): string {
    const tabs = this.questionnaire.questions.map((question, index) =>
      index === this.#tab ? role.accent(`[${question.header}]`) : role.meta(question.header),
    );
    tabs.push(
      this.#tab === this.questionnaire.questions.length
        ? role.accent("[Review / submit]")
        : role.meta("Review / submit"),
    );
    return clip(`${tabs.join("  ")}  `, width);
  }

  #renderQuestion(width: number): string[] {
    const question = this.questionnaire.questions[this.#tab]!;
    const lines: string[] = ["", role.strong(question.question)];
    for (const [index, option] of question.options.entries()) {
      const focused = index === this.#row;
      const selected = this.#selected[this.#tab]!.has(option.label);
      const marker = question.multi_select ? (selected ? "[x]" : "[ ]") : selected ? "●" : "○";
      lines.push(
        `${focused ? role.accent("›") : " "} ${marker} ${role.strong(option.label)}`,
        ...wrap(`   ${option.description}`, Math.max(20, width - 8)).map((line) => role.meta(line)),
      );
    }
    const customFocused = this.#row === question.options.length;
    const draft = this.#custom[this.#tab];
    lines.push(
      `${customFocused ? role.accent("›") : " "} ${role.accent("Type something.")}`,
      ...wrap(`   ${draft ?? "Enter a custom answer"}`, Math.max(20, width - 8)).map((line) =>
        draft === undefined ? role.meta(line) : line,
      ),
    );

    const preview = this.#focusedPreview(question);
    if (preview !== undefined) {
      const previewLines = new Markdown(preview, 0, 0, markdownTheme).render(
        Math.max(20, Math.floor(width * 0.46)),
      );
      if (width >= 100) {
        const optionLines = lines.slice(0);
        const leftWidth = Math.max(24, Math.floor(width * 0.5));
        const count = Math.max(optionLines.length, previewLines.length);
        return [
          ...Array.from({ length: count }, (_, index) =>
            `${pad(optionLines[index] ?? "", leftWidth)} ${previewLines[index] ?? ""}`.trimEnd(),
          ),
        ];
      }
      lines.push("", role.strong("Preview"), ...previewLines);
    }
    return lines;
  }

  #renderReview(width: number): string[] {
    const lines: string[] = ["", role.strong("Review your answers")];
    for (const [index, question] of this.questionnaire.questions.entries()) {
      const answer = this.#answerFor(index);
      lines.push(
        answer === undefined
          ? role.warning(`${question.header}: unanswered`)
          : role.success(`${question.header}: ${answer}`),
      );
    }
    lines.push(
      "",
      `${this.#row === 0 ? role.accent("›") : " "} ${role.strong(this.#hasAnswers() ? "Submit answers" : "Submit (decline)")}`,
      role.meta(`  ${width < 60 ? "Esc declines" : "Esc explicitly declines this questionnaire"}`),
    );
    return lines;
  }

  #focusedPreview(question: QuestionnaireSpecification["questions"][number]): string | undefined {
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

  #toggleOption(label: string): void {
    const selected = this.#selected[this.#tab]!;
    if (selected.has(label)) selected.delete(label);
    else selected.add(label);
    this.#custom[this.#tab] = undefined;
    this.#changed();
  }

  #moveTab(delta: number): void {
    const count = this.questionnaire.questions.length + 1;
    this.#tab = (this.#tab + delta + count) % count;
    this.#row = 0;
    this.#changed();
  }

  #moveRow(delta: number): void {
    const max = this.#tab === this.questionnaire.questions.length
      ? 0
      : this.questionnaire.questions[this.#tab]!.options.length;
    this.#row = Math.max(0, Math.min(max, this.#row + delta));
    this.#changed();
  }

  #changed(): void {
    this.#onChange?.();
  }
}

function wrap(text: string, width: number): string[] {
  const words = text.split(/\s+/).filter((word) => word.length > 0);
  if (words.length === 0) return [""];
  const lines: string[] = [];
  let line = "";
  for (const word of words) {
    if (line.length > 0 && [...line, " ", ...word].length > width) {
      lines.push(line);
      line = word;
    } else {
      line = line.length === 0 ? word : `${line} ${word}`;
    }
  }
  if (line.length > 0) lines.push(line);
  return lines;
}

function pad(value: string, width: number): string {
  return `${value}${" ".repeat(Math.max(0, width - plainWidth(value)))}`;
}

function clip(value: string, width: number): string {
  if (plainWidth(value) <= width) return value;
  return [...value].slice(0, Math.max(0, width - 1)).join("") + style.dim("…");
}
