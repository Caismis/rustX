/** One URL identity for browser binding and Product Host routing. */
export function endpointIdentity(endpoint: string): string {
  return new URL(endpoint).href;
}
export function sameEndpoint(left: string | undefined, right: string | undefined): boolean {
  if (!left || !right) return false;
  try { return endpointIdentity(left) === endpointIdentity(right); } catch { return false; }
}
