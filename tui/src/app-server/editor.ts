import type { UserInputBlock } from "../protocol/app-server.ts";

/** Restored receipts are draft input, never caller-authored canonical metadata. */
export function editorText(content: readonly UserInputBlock[]): string {
  return content.flatMap(block => block.type === "text" ? [block.text] : []).join("");
}
export class RestoredEditorOrderingError extends Error {
  constructor() {
    super("Cannot edit restored text separated by uploads in the TUI. Restore the original text to submit with upload ordering intact. Draft and receipts retained.");
    this.name = "RestoredEditorOrderingError";
  }
}

/** Whether a flat editor change crosses a receipt-delimited text boundary. */
function separatedText(content: readonly UserInputBlock[]): boolean {
  let sawText = false;
  let uploadAfterText = false;
  for (const block of content) {
    if (block.type === "text") {
      if (uploadAfterText) return true;
      sawText = true;
    } else if (sawText) uploadAfterText = true;
  }
  return false;
}

function replaceText(content: readonly UserInputBlock[], text: string): UserInputBlock[] {
  let replaced = false;
  const result = content.flatMap<UserInputBlock>(block => {
    if (block.type !== "text") return [block];
    if (replaced) return [];
    replaced = true;
    return [{ type: "text" as const, text }];
  });
  if (!replaced) result.push({ type: "text", text });
  return result;
}

/**
 * Retain a raw editor baseline while joining drafts. An unambiguous text region
 * can follow its edits; an ambiguous edit must keep its original receipt/text
 * baseline so joining another draft cannot turn it into valid input.
 */
export function retainedEditorBlocks(content: readonly UserInputBlock[], text: string): UserInputBlock[] {
  return text === editorText(content) || separatedText(content) ? [...content] : replaceText(content, text);
}

export function editorSubmission(content: readonly UserInputBlock[], text: string): UserInputBlock[] {
  if (text === editorText(content)) return [...content];
  // Adjacent text blocks form one editable region. An upload between text
  // regions makes a flat edit ambiguous; reject before producing any payload.
  if (separatedText(content)) throw new RestoredEditorOrderingError();
  return replaceText(content, text);
}
