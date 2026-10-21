/**
 * Pure answer-shape rules and wording of the questionnaire surface: which
 * rows a question offers, how a draft is validated against its declared
 * shape (the exact numeric reading is the shared protocol seam), how bounds
 * and requesters are described. No terminal state lives here.
 */
import type { AnswerSpecification, FiniteNumberWire, InteractionRequester, OptionSpecification, QuestionSpecification } from "../../protocol/app-server.ts";
import { finiteNumberDecimal, finiteNumberFromWire, readScalarAnswer, type ScalarAnswerRejection } from "../../../../protocol/app-server/number.ts";

const NATIVE_POPUP_TITLE = "Ask user · questionnaire";

/** The typed rows one question's answer surface presents. */
export type RowKind =
  | { kind: "option"; optionIndex: number }
  | { kind: "boolean"; value: boolean }
  | { kind: "scalar" }
  | { kind: "custom" };

/** The declared options of a choice question, or an empty list. */
export function choiceOptions(question: QuestionSpecification): OptionSpecification[] {
  const answer = question.answer;
  return answer.type === "single_choice" || answer.type === "multi_choice"
    ? answer.options
    : [];
}

/** Whether a question accepts a free-text answer outside its options. */
export function allowsCustom(answer: AnswerSpecification): boolean {
  return (answer.type === "single_choice" || answer.type === "multi_choice") &&
    answer.allow_custom;
}

/** The ordered rows one question's answer surface presents. */
export function rowsOf(question: QuestionSpecification): RowKind[] {
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
      if (answer.min_length != null && length < answer.min_length) {
        return `Enter at least ${answer.min_length} characters.`;
      }
      if (answer.max_length != null && length > answer.max_length) {
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
    case "number":
    case "integer": {
      // The shared reading decides syntax, exactness, the canonical domain
      // and the declared bounds; only the words are this terminal's.
      const reading = readScalarAnswer(answer, value);
      return reading.kind === "answer" ? undefined : scalarRejectionText(answer.type, reading);
    }
    default:
      return undefined;
  }
}

/** This terminal's words for one shared machine-readable rejection. */
function scalarRejectionText(type: "number" | "integer", rejection: ScalarAnswerRejection): string {
  const noun = type === "number" ? "a number" : "a whole number";
  switch (rejection.reason) {
    case "malformed": return `Enter ${noun}.`;
    case "not-finite": return "Enter a finite number.";
    case "inexact": return "Enter a number this runtime can represent exactly.";
    case "out-of-range": return "Enter a whole number inside the 64-bit range.";
    case "below-minimum": return `Enter ${noun} at least ${rejection.bound}.`;
    case "above-maximum": return `Enter ${noun} at most ${rejection.bound}.`;
  }
}

/**
 * One declared `Number` bound as a JavaScript `number`.
 *
 * Every `Number` bound in this client is decoded here, so no second parser can
 * disagree with the one the answer is encoded through.
 */
export function numberBound(bound: FiniteNumberWire | undefined): number | undefined {
  return bound === undefined ? undefined : finiteNumberFromWire(bound);
}

/**
 * One declared `Number` bound as the decimal a human reads — and can type
 * back, which `String(value)` would not guarantee for a large whole number.
 */
export function numberBoundLabel(bound: FiniteNumberWire | undefined): string | undefined {
  const value = numberBound(bound);
  return value === undefined ? undefined : finiteNumberDecimal(value);
}

export function isCalendarDate(value: string): boolean {
  const match = /^(\d{4})-(\d{2})-(\d{2})$/.exec(value);
  if (match === null) return false;
  const [, year, month, day] = match;
  const date = new Date(`${year}-${month}-${day}T00:00:00Z`);
  return !Number.isNaN(date.getTime()) &&
    date.getUTCFullYear() === Number(year) &&
    date.getUTCMonth() + 1 === Number(month) &&
    date.getUTCDate() === Number(day);
}

export function isAbsoluteUri(value: string): boolean {
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
  if ("managed_python" in requester.origin) {
    return [`Requested by Python source: ${requester.origin.managed_python.package}`, `Tool: ${requester.tool_name}`];
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
    : "mcp" in requester.origin ? `mcp:${requester.origin.mcp.server_id}/${requester.tool_name}` : `python:${requester.origin.managed_python.package}/${requester.tool_name}`;
}

/** The popup frame title for one requester. */
export function requesterTitle(requester: InteractionRequester): string {
  return requester.origin === "builtin"
    ? NATIVE_POPUP_TITLE
    : "mcp" in requester.origin ? `MCP elicitation · ${requester.origin.mcp.server_id}` : `Python source · ${requester.origin.managed_python.package}`;
}

export function scalarPrompt(answer: AnswerSpecification): string {
  switch (answer.type) {
    case "number":
      return "Enter a number.";
    case "integer":
      return "Enter a whole number.";
    default:
      return "Type your answer.";
  }
}

/**
 * The human-facing statement of a scalar question's declared bounds.
 *
 * Both scalar domains hand it decimal *text*: `Integer` bounds are already the
 * canonical decimal the runtime published, and `Number` bounds have been
 * decoded from their canonical binary64 spelling into the decimal a human
 * reads. Neither domain's wire form is ever shown.
 */
export function boundsSentence(
  noun: string,
  minimum: string | undefined,
  maximum: string | undefined,
): string | undefined {
  if (minimum !== undefined && maximum !== undefined) {
    return `${noun} between ${minimum} and ${maximum}`;
  }
  if (minimum !== undefined) return `${noun} at least ${minimum}`;
  if (maximum !== undefined) return `${noun} at most ${maximum}`;
  return undefined;
}

export function textGuidance(
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
