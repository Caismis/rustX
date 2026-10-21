/** The message of any thrown value. */
export function errorMessage(error: unknown): string {
  return error instanceof Error ? error.message : String(error);
}

/** One status-line-safe diagnostic: newlines become separators. */
export function compactDiagnostic(error: unknown): string {
  return errorMessage(error).replace(/\s*\r?\n\s*/g, " · ").trim();
}
