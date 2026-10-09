# Web connection ownership and security

The dev launcher owns the App Server, exact endpoint, transport credential, browser
launch credential, Host config, carrier and private scratch lifetime. Carrier
authentication hands admission material to the browser; it adds no App Server RPC,
proxy, native configuration or Workspace authority. The browser connects directly
with the existing AppServerClient and App Server protocol **v38**.

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
| Remote transport token | Settings/controller memory only | Page; retained across mode changes |
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
explicit endpoint/token. Reconnect stays in the committed mode; no failure triggers fallback.
Reload starts Local and re-bootstraps; Remote tokens and mode are not persisted.
Standalone component servers without bootstrap fail closed into product recovery.

Selecting Remote opens material entry without touching the current authority. If a
Remote endpoint/token has already been supplied, selecting Remote explicitly uses
that remembered material. Selecting Local fetches and validates bootstrap before
requesting a transition. `selectedMode` describes the material-entry selection;
`mode` describes committed ownership and never changes on admission refusal.

AppServerClient owns endpoint/token validation and replacement admission. Its
side-effect-free admission check runs immediately before synchronous fencing, with
no intervening await. Eight detached evidence batches or more than 64
current/reserved Session diagnostic rows (including deletion obligations) refuse
replacement without changing generation, socket, views, evidence, or credentials.
Same normalized endpoint reconnects perform replacement admission when initialization
observes a different native `authority_id`, before reading the new catalog.

Admission reserves one detached batch for the old authority, including any evidence
created by close. The request pump transmits at most eight pending operations; only
sent mutations become uncertain, exactly once. Unsent requests are discarded. The
request admission bound keeps existing uncertainty plus pending mutations at most
64. Session capacity includes the union of diagnostic rows and pending Session IDs,
including queued operations. After fencing, generation guards prevent new diagnostic
rows from old continuations; model/cancellation continuations only update reserved
rows. Once close settles, the client collects that final evidence before retiring
old views. No close-time capacity refusal can strand an admitted transition.

Ownership commits after old-socket settlement and authority retirement, immediately
before the target socket is created. The client notifies the controller at this
point to commit mode. A subsequent connection failure leaves that target selected
and does not reconnect the old authority. A close timeout retains old authority
state and mode but fails closed with a disconnected transport. Remote endpoint/token
remain in page memory across refusal, Local success, and Local failure, allowing an
explicit user-selected return; neither is persisted. New Remote material replaces
the remembered material only when its ownership commits.

Browser Session/control state belongs to the normalized endpoint and native
AppServerHost process UUID (`initialize.authority_id`). The UUID is stable over
connections to that process, not across process replacement. It is not durable-store
identity: a replacement may use the same store or a different one. Neither endpoint,
cwd, Session ID nor title establishes durable identity. No durable identity protocol
or browser recovery journal is introduced.

Product Host authority is a separate Workspace-domain identity owned by
`WorkspaceAuthority`, not by `WorkspaceAssociations` or by the HTTP adapter object's
identity. Fresh admission captures its epoch, reads native settings and classifies
that exact cwd under the captured Host ID. Before attach/fork dispatch, holding a
reserved RPC slot, it re-observes the Host and freshly classifies the same exact cwd
under that observation and the captured normalized endpoint; process identity alone
never re-admits. The transport checks that proof plus native generation, navigation
and injected callback identity before send. At most two such validations reserve
ordinary RPC slots at once; they cannot consume the independent two-request
lifecycle-control lane used by `turn/cancel`. Ordinary RPC saturation cannot delay
an otherwise admissible exact cancellation; a full control lane refuses it locally
before transmission. Host authority scope is `authorityId` plus normalized endpoint.
Old successful classification cannot authorize a replacement Host, even with the same
adapter and no display projection running. Display listens to authority retirement
only to clear incompatible evidence; a cached association never authorizes an
operation and is never required for its correctness.

Session-file bytes additionally require a separate native Product Host credential.
The launcher generates a distinct process-ephemeral 256-bit secret, passes an
owner-only file with `--product-host-token-file` to the native process and its value
only to Node's private Host configuration. The browser receives neither. File reads
use the private `/product-host/file-read` WebSocket lane, not an ordinary App Server
Method. The Host supplies current registered Workspace roots; configured picker
locations or display classification alone do not authorize bytes. Root changes and
Host retirement abort its owned reads. Native handshake authority, attachment and
mapping fences independently prevent obsolete publication. See
[the complete delivery contract](../docs/file-delivery.md).

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
and clear on authority replacement.

Unresolved deletion never requires connecting to the retired process before admitting
a replacement. Retirement captures Session ID, old endpoint/process UUID and either
unknown deletion outcome, committed cleanup pending, or committed durability uncertain.
A lost recovery reply invalidates recovery admission but preserves the earlier committed
fact. Old attachment/view authority, pending continuations and first submissions are
fenced before reused IDs enter the replacement catalog. No old delete, preview or
recovery request is automatically sent to the replacement. Same-process reconnect
still obtains `session/deletePreview`; explicit recovery uses that fresh observation.

**Settings → Advanced → Connection → Detached authority diagnostics** remains reachable
while connected or disconnected. It explains and displays retained outcomes, and
**I have reviewed this historical evidence** removes browser evidence only. This
acknowledgement neither settles native deletion/cleanup/durability nor sends an RPC.
There is no cross-authority reconciliation action; users can independently inspect
the selected authority and obtain its fresh operation preconditions through ordinary
Session actions. Old target revisions and attachments never participate.

Capacity exhaustion remains a distinct refusal. Review and acknowledge retained batches
before reconnecting. If more than 64 current/reserved Session diagnostics prevent
replacement, disconnect and use **Review disconnected Session evidence** in the same
Connection surface to acknowledge individual browser observations. This removes the
local view/intent only, emits no native deletion event and performs no mutation.
Uncertain operation records remain separately inspectable. No reload, storage clearing,
automatic evidence eviction, or native settlement claim is needed.

Ordinary Settings opens **General**. Recovery **Show details** opens the Advanced
Connection surface directly; transport is never the default Settings page.

Browser handoff observes OS acceptance, never browser lifetime. On Linux/macOS/WSL,
`open()` success lets the helper exit without referencing or waiting on the returned
process. On Windows only, the helper waits for the short-lived PowerShell launcher's
exit and rejects nonzero status. The parent's deadline/cancellation can kill only
the rustX helper, never the user's browser; the environment allowlist is unchanged.

Harness `ddefc45fbc7f8e46dd73185e68295696d1297887` was inspected as a product/security
reference. Authentication and launcher code are independently implemented. Unlike
Harness's persistent signing credential and 30-day cookie, rustX has no durable
browser secret or expiry database. Existing adapted Settings provenance is retained.

## Native upload data lane

App Server v38 advertises ordinary file policy at initialization. Upload prepare
is authenticated through the exact native attachment; a short-lived single-use
capability authorizes a separate binary socket at the same selected native origin.
No transport key, Product Host credential or local path appears in the descriptor.
Remote descriptors cannot select loopback ports or foreign origins. The binary
carrier never writes files or issues receipts; status reads use native operation
correlation. Control JSON limits and image/preview policy remain independent.

### Attachment controls and native Node residency

New target-bound effects require the exact committed `attachmentObservation`
(generation, Session/Node, Conversation, runtime incarnation, target and intent
revision). `target()` is a control admission API. A retained `view.target` is
settlement evidence, not execution permission. The client composes that proof
with each request's existing admission and checks it immediately before socket
send, including after backpressure and asynchronous validation. Native reads and
subscription repair remain observations; serialized detach and switch use their
own settlement proofs. Cancellation retains its dedicated transport lane and
correlated outcome semantics, but cannot start under revoked authority.

Release and switch synchronously revoke observation/control admission. Release
can queue behind a switch, but Open is refused until that switch has settled,
even when Release is the queue tail. Switch is a native unload, catalog selection
and successor load transaction; it is not a browser target replacement. Its
correlated acknowledgement remains authoritative after an old-Route closure
notification or local Release. Release does not reverse committed selection.

Switch failure cannot prove the old Runtime survived: the server retires its
Route even when successor composition fails. The client retires old authority
and retains cleanup identity until explicit Release confirms detach or an exact
`stale_attachment` response. Open then rereads native selection. A transmitted switch
with a lost response remains uncertain, disables automatic reattachment, and is
never replayed. A provably unsent switch retains its old target only for explicit
Release. Detach releases a claim without unloading the resident Runtime; opening
another Node requires the explicit native switch lifecycle, not Release/Open.
