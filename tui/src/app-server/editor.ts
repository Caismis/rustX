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

export function editorSubmission(content: readonly UserInputBlock[], text: string): UserInputBlock[] {
  if (text === editorText(content)) return [...content];
  // Adjacent text blocks form one editable region. An upload between text
  // regions makes a flat edit ambiguous; reject before producing any payload.
  let sawText = false;
  let uploadAfterText = false;
  for (const block of content) {
    if (block.type === "text") {
      if (uploadAfterText) throw new RestoredEditorOrderingError();
      sawText = true;
    } else if (sawText) {
      uploadAfterText = true;
    }
  }
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
