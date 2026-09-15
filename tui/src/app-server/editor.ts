import type { UserInputBlock } from "../protocol/app-server.ts";

/** Restored receipts are draft input, never caller-authored canonical metadata. */
export function editorText(content: readonly UserInputBlock[]): string {
  return content.flatMap(block => block.type === "text" ? [block.text] : []).join("");
}
export function editorSubmission(content: readonly UserInputBlock[], text: string): UserInputBlock[] {
  if (text === editorText(content)) return [...content];
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
