import { editorSubmission, editorText } from "../app-server/editor.ts";
import type { UserInputBlock } from "../protocol/app-server.ts";

/** Unsubmitted presentation state. Receipt validity and lifetime remain Session-owned. */
export class ComposerDraft {
  text = "";
  blocks: UserInputBlock[] | undefined;
  tail = false;
  restore(blocks: UserInputBlock[]): void {
    this.blocks = structuredClone(blocks);
    this.text = editorText(blocks);
    this.tail = false;
  }
  submission(text = this.text): UserInputBlock[] {
    if (this.blocks === undefined) return text ? [{ type: "text", text }] : [];
    if (this.tail) return [...this.blocks, ...(text ? [{ type: "text" as const, text }] : [])];
    return editorSubmission(this.blocks, text);
  }
  clear(): void { this.text = ""; this.blocks = undefined; this.tail = false; }
  get uploads(): number { return this.blocks?.filter(block => block.type === "upload").length ?? 0; }
}
