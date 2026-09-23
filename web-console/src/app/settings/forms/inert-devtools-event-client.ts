/** The replacement for `@tanstack/devtools-event-client` in this application.
 *
 * TanStack Form core ships a devtools event client and always constructs it:
 * every mounted form publishes its complete state — field values included — as
 * `CustomEvent`s on `window`, and queues those payloads while it waits for a
 * devtools bus to answer its handshake. Settings forms hold literal Provider
 * credentials and MCP literal environment values while they are being typed,
 * and nothing in this product may broadcast or retain such a value outside the
 * one actor-owned transaction that owns it.
 *
 * Vite resolves the package to this module for both the production bundle and
 * the test runtime (see `vite.config.ts`), so the channel does not exist at
 * all: nothing is queued, dispatched or listened for. It implements exactly the
 * public surface form core calls, and deliberately no devtools behavior. */
type Cleanup = () => void;
const none: Cleanup = () => {};

export class EventClient<TEventMap extends Record<string, unknown>> {
  readonly #pluginId: string;
  constructor({ pluginId }: { pluginId: string; debug?: boolean; reconnectEveryMs?: number; enabled?: boolean }) {
    this.#pluginId = pluginId;
  }
  getPluginId(): string { return this.#pluginId; }
  createEventPayload<TEvent extends keyof TEventMap & string>(eventSuffix: TEvent, payload: TEventMap[TEvent]) {
    return { type: `${this.#pluginId}:${eventSuffix}`, payload, pluginId: this.#pluginId };
  }
  emit(): void {}
  on(): Cleanup { return none; }
  onAll(): Cleanup { return none; }
  onAllPluginEvents(): Cleanup { return none; }
}
