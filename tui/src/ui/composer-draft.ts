import { editorSubmission, editorText, retainedEditorBlocks } from "../app-server/editor.ts";
import type { UserInputBlock } from "../protocol/app-server.ts";

/** Command-token origin belongs to the same logical draft as its text. */
export interface ComposerProvenance {
  readonly literal: boolean;
  readonly pasteAtTokenEnd?: number;
}

/** Unsubmitted presentation state. Receipt validity and lifetime remain Session-owned. */
export class ComposerDraft {
  text = "";
  provenance: ComposerProvenance = { literal: false };
  blocks: UserInputBlock[] | undefined;
  tail = false;
  restore(blocks: UserInputBlock[]): void {
    this.blocks = structuredClone(blocks);
    this.text = editorText(blocks);
    this.provenance = { literal: Boolean(this.text) };
    this.tail = false;
  }
  submission(text = this.text): UserInputBlock[] {
    if (this.blocks === undefined) return text ? [{ type: "text", text }] : [];
    if (this.tail) return [...this.blocks, ...(text ? [{ type: "text" as const, text }] : [])];
    return editorSubmission(this.blocks, text);
  }
  /** Join retained state, without validating or constructing a submission. */
  combine(newer: ComposerDraft): ComposerDraft {
    if (newer === this || newer.empty) return this;
    if (this.empty) return newer;
    const older = this.#rawState();
    const next = newer.#rawState();
    const combined = new ComposerDraft();
    combined.blocks = [...older.blocks, ...next.blocks];
    combined.text = older.text + next.text;
    combined.provenance = { literal: Boolean(combined.text) };
    return combined;
  }
  /** A known-unsent capture precedes any newer edits, including invalid ones. */
  prepend(content: UserInputBlock[]): void {
    const newer = this.#rawState();
    this.blocks = [...structuredClone(content), ...newer.blocks];
    this.text = editorText(content) + newer.text;
    this.provenance = { literal: Boolean(this.text) };
    this.tail = false;
  }
  #rawState(): { blocks: UserInputBlock[]; text: string } {
    if (this.blocks === undefined) return { blocks: this.text ? [{ type: "text", text: this.text }] : [], text: this.text };
    if (this.tail) return {
      blocks: [...this.blocks, ...(this.text ? [{ type: "text" as const, text: this.text }] : [])],
      text: editorText(this.blocks) + this.text,
    };
    return { blocks: retainedEditorBlocks(this.blocks, this.text), text: this.text };
  }
  get empty(): boolean { return !this.text && !this.blocks?.length; }
  clear(): void { this.text = ""; this.provenance = { literal: false }; this.blocks = undefined; this.tail = false; }
  get uploads(): number { return this.blocks?.filter(block => block.type === "upload").length ?? 0; }
}
