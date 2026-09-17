/* Copyright (c) 2026 DeepSeek. MIT. See PROVENANCE.md. */
/** Browser clipboard only; no deprecated host compatibility path. */
export async function writeClipboard(text: string): Promise<boolean> {
  if (!navigator.clipboard?.writeText) return false;
  try { await navigator.clipboard.writeText(text); return true; }
  catch { return false; }
}
