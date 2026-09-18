# Web connection ownership and security

The dev launcher owns the App Server, exact endpoint, transport credential, browser
launch credential, Host config, carrier and private scratch lifetime. Carrier
authentication hands admission material to the browser; it adds no App Server RPC,
proxy, native configuration or Workspace authority. The browser connects directly
with the existing AppServerClient and App Server protocol **v8**.

`GET /?token=<browser-launch-token>` accepts exactly one bounded 43-character
base64url credential on the root route. Timing-safe comparison follows format checks.
Success mints a fresh independent 32-byte random browser-session proof and returns
a tiny HTML exchange page, not the app. It has `Cache-Control: no-store`,
`Referrer-Policy: no-referrer`, no external resources, and CSP `default-src 'none'`
with only the exact inline script hash permitted, no base/forms/frames. The script
stores only the proof under `rustx-browser-session` in sessionStorage, then calls
`location.replace('/')`. No application resources load from the credential URL.
The launch token is never stored; the native token never appears in the exchange.
The printed launch URL can authorize another browser during the same composition.

Every carrier HTTP request must have the exact `127.0.0.1:<bound-port>` Host;
an Origin, if present, must match the exact `http://127.0.0.1:<port>` origin.
Cookies have no authentication authority: browsers deliver host-scoped Cookies to
other ports regardless of Cookie name, HMAC or SameSite. Instead sessionStorage's
scheme/host/port boundary isolates delivery. A shared browser HTTP helper attaches
`X-Rustx-Browser-Session` only to this exact origin's `/__rustx/bootstrap` and
`/product-host/*`, never redirects or external destinations. Cross-origin navigation
does not send it. The carrier validates bounded proof format and its exact origin
registration before Product Host middleware. Origin/Host checks are defenses, not
substitutes for the proof. No query or Authorization bearer is accepted on APIs.

Proofs are independently random and distinct from launch/native tokens. The carrier
keeps at most 128 proof/origin registrations in process memory, refuses further
exchanges with 429 rather than evicting live tabs, and loses all registrations on
restart. A stale tab reaches LocalManaged recovery; reopen the newly printed launch
URL to authenticate again. Reload in the same tab preserves a valid proof. Opening
the startup URL again mints a new proof. Static app resources are public on loopback
and confer no API authority. Cross-Origin-Opener-Policy is `same-origin`.

This protects against another loopback origin observing browser-delivered credentials,
not against same-origin script compromise, malicious browser extensions or local
processes able to read launcher scratch/process memory. The proof is JavaScript-
readable on its own origin; it is not an HttpOnly credential.

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
| Native transport token | Launcher 0600 scratch file; carrier/page memory, never browser storage | Composition/page |
| Browser launch token | Launcher 0600 bootstrap config, initial URL, carrier memory; never browser storage | Composition |
| Browser session proof | Exact-origin sessionStorage only; carrier memory registration | Tab storage; accepted only by minting carrier activation |
| Remote transport token | Settings/controller memory only | Page; discarded when returning Local |
| Endpoint-scoped navigation/presentation hints | Browser localStorage | Preference lifetime; never connection material |
| Provider/MCP credentials | Existing native owners | Unchanged |

The bootstrap config references the existing private transport-token file instead
of duplicating it. Child settlement precedes scratch deletion. Only the derived
browser proof may enter sessionStorage. Launch/native/provider/MCP credentials and
bootstrap JSON never enter any browser storage; no proof enters localStorage or
IndexedDB. Only the initial
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
