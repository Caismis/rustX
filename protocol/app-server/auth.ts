/** Encode the bearer bytes into one valid WebSocket subprotocol token. */
export function credentialSubprotocol(credential: string): string {
  return `rustx-token.${btoa(credential).replace(/\+/g, '-').replace(/\//g, '_').replace(/=+$/, '')}`;
}
