# Web connection ownership and security

The dev launcher owns the App Server, exact endpoint, transport credential, browser
launch credential, Host config, carrier and private scratch lifetime. Carrier
authentication hands admission material to the browser; it adds no App Server RPC,
proxy, native configuration or Workspace authority. The browser connects directly
with the existing AppServerClient and App Server protocol **v8**.

`GET /?token=<browser-launch-token>` accepts exactly one bounded 43-character
base64url credential on the root route. Timing-safe comparison follows format checks.
Success returns 303 to `/`, a host-only HttpOnly SameSite=Strict Path=/ session
cookie, no persistent expiry, `Cache-Control: no-store`, and `Referrer-Policy: no-referrer`.
No application resources load before redirect. The native token is never in this URL.
The printed launch URL can authorize another browser during the same composition.

Every carrier HTTP request must have the exact `127.0.0.1:<bound-port>` Host;
an Origin, if present, must match it. Sessions use a random process-ephemeral HMAC
secret and exact authority as signed input and cookie-name input. Cookies cannot
authenticate another port even if copied/renamed. Restart invalidates old sessions.
Malformed/wrong tokens fail even when a valid session accompanies them. No query
or Authorization bearer is accepted on APIs. Auth runs before Product Host middleware.

Authenticated `GET /__rustx/bootstrap` returns only:

```json
{
  "connectionMode": "local",
  "appServerEndpoint": "ws://127.0.0.1:<native-port>/",
  "appServerTransportToken": "<native-transport-token>"
}
```

The response is JSON with `Cache-Control: no-store`. Workspace roots, native settings
and provider/MCP credentials are excluded. Root authorization remains exact-root
Host policy, independent of browser authentication and native socket admission.

| Material | Owner/storage | Lifetime |
| --- | --- | --- |
| Native transport token | Launcher 0600 scratch file; carrier/page memory | Composition/page |
| Browser launch token | Launcher 0600 bootstrap config, initial URL, carrier memory | Composition |
| Browser session secret | Carrier memory only | Carrier process |
| Browser session credential | Authority-bound HttpOnly session cookie | Valid only in carrier process |
| Remote transport token | Settings/controller memory only | Page; discarded when returning Local |
| Endpoint-scoped navigation/presentation hints | Browser localStorage | Preference lifetime; never connection material |
| Provider/MCP credentials | Existing native owners | Unchanged |

The bootstrap config references the existing private transport-token file instead
of duplicating it. Child settlement precedes scratch deletion. No credential or
bootstrap JSON enters localStorage, sessionStorage or IndexedDB. Only the initial
browser launch URL is printed, once; transport details belong in advanced Settings/Inspector.

ConnectionController owns source selection: **LocalManaged** fetches authenticated
same-origin bootstrap, while **RemoteExplicit** requires a Settings gesture and
explicit endpoint/token. Neither failure changes mode. Reconnect stays mode-local.
Reload starts Local and re-bootstraps; Remote tokens and mode are not persisted.
Standalone component servers without bootstrap fail closed into product recovery.

Selecting Remote disconnects Local before accepting remote material. Selecting
Local disconnects Remote and waits for socket closure before fetching bootstrap.
AppServerClient retains endpoint/token validation and socket replacement, waiting
for the old close event before creating a replacement. Generations fence obsolete
continuations. No mutations are replayed and Session authority remains native.

Browser Session/control state belongs to one concrete App Server authority: the
exact normalized WebSocket endpoint (scheme, host, port and root path). This is a
transport authority, not a durable server identity. A server replaced behind the
same endpoint is indistinguishable without new protocol identity; v8 runtime and
attachment fencing still apply. Local compositions use ephemeral endpoints. No
server registry or identity protocol is added.

ConnectionController explicitly chooses same-authority reconnect or authority
replacement. Same-authority reconnect retains wanted views/node intent and repairs
them from native facts. Replacement fences the old generation synchronously,
settles its socket once, then retires its catalog, views, focus, projections,
attachment/node intent, interaction admission and Settings drafts before admitting
the new connection. A colliding Session ID never carries intent across endpoints.
Saved navigation hints are restored only for the first matching authenticated or
explicitly selected endpoint, never used to select an endpoint, and are cleared on
authority replacement. No multi-authority view history is maintained.

Transmitted operations that lose responses remain uncertain exactly once. On
replacement, uncertainty and Session diagnostic evidence move into read-only
**Settings → Connection → Detached authority diagnostics**, tagged with their old
endpoint. They have no control, replay, reattachment or new-server admission effect,
even when Session/interaction IDs collide. Evidence is page-memory only: at most
eight detached batches, each with the client's bounded operations and up to 64
Session diagnostics. Capacity refuses replacement rather than evicting evidence;
explicit acknowledgement removes a batch. Wire observations are replaceable logs
and clear on authority replacement. Unresolved deletion verification or committed
deletion recovery refuses replacement with a visible error: reconnect the original
authority and resolve it first. It is neither dropped nor sent to another server.

Ordinary Settings opens **Overview**. Recovery **Show details** and connection
recovery actions explicitly open **Connection**; transport is never the default.

Browser handoff observes OS acceptance, never browser lifetime. On Linux/macOS/WSL,
`open()` success lets the helper exit without referencing or waiting on the returned
process. On Windows only, the helper waits for the short-lived PowerShell launcher's
exit and rejects nonzero status. The parent's deadline/cancellation can kill only
the rustX helper, never the user's browser; the environment allowlist is unchanged.

Harness `ddefc45fbc7f8e46dd73185e68295696d1297887` was inspected as a product/security
reference. Authentication and launcher code are independently implemented. Unlike
Harness's persistent signing credential and 30-day cookie, rustX has no durable
browser secret or expiry database. Existing adapted Settings provenance is retained.
