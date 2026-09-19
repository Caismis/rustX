/**
 * Separates paste content from app shortcuts. Pi still decodes and edits text.
 * ProcessTerminal normally delivers complete packets; direct component input
 * may split the closing marker, so retain only its bounded suffix, not a paste.
 */
export class PasteGuard {
  #active = false;
  #suffix = "";
  content(data: string): boolean {
    const content = this.#active || data.includes("\x1b[200~");
    if (!content) return false;
    const framed = this.#suffix + data;
    this.#active = !framed.includes("\x1b[201~");
    this.#suffix = this.#active ? framed.slice(-5) : "";
    return true;
  }
}
