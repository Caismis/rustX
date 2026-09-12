# External source activation and credentials

Offline [configuration diagnostics](configuration-diagnostics.md) stop before
this activation pipeline. Host path capture is separate from credential capture.
Explicit `doctor --probe` uses the same authority and lifecycle owners; it never
enables sources, grants trust, installs a recovery task or invokes business Tools.
Managed Python preparation additionally requires `--prepare`.

The source pipeline is:

```text
bounded inert discovery
  -> host trust and source eligibility
  -> admitted Agent/Workflow source demand
  -> credential resolution and instance freeze
  -> existing Python preparation / MCP process or HTTP connection
  -> capability availability
  -> main / Subagent / Workflow admission
  -> model exposure
```

Each arrow is a separate decision. Discovery is not enablement; enablement does
not prove availability; availability does not grant domain admission; admission
does not necessarily expose a Tool to the main model. Managed Python has typed source provenance `python:<folder>` and shares
[ordinary source selection](tool-source-selection.md) with MCP. The existing
materialization owners remain distinct within one Tool Plane.

## TOML surface (runtime schema 8)

MCP source definitions remain explicit settings and require `enabled = true`
to be eligible for preparation. An absent or false value is inert.

```toml
[mcp_servers.public-service]
enabled = true
type = "http"
url = "https://service.example/mcp"
```

Managed Python source existence comes solely from `.agents/tools/<package>`.
Its identity is `python:<package>`. The generation-scoped `ManagedPythonCatalog`
records bounded directory identities, without reading server code or dependency
files. Missing roots, empty roots and packages without prepared environments do
not block native-only startup. Non-directory incidental files have no identity;
invalid package identities and redirected package roots fail static discovery.

There is no workspace `python_sources` settings map. Discovery does not enable
every package, import Python, run uv, install dependencies, create environments,
spawn MCP, or capture credentials. Admitted Agent/Workflow selection supplies finite preparation demand to the
existing source lifecycle; unreferenced discovered packages remain inert.

## Trust, layers, and replacement

The CFG-01 Rust resolver remains the only merger: defaults < user settings <
trusted project settings < applicable explicit CLI fields. There is no source
activation CLI overlay. `--config` replaces the project slot and never changes
its authority. Host trust is required before composition, including for an
otherwise empty project; an untrusted launch is rejected before preparation.
Trust does not enable any external source. Trusted + disabled stays inert;
untrusted + enabled is rejected; trusted + enabled is eligible for preparation.

`mcp_servers` overlays names and replaces a same-name entry **as a whole**. A
replacement includes its own enable decision, command/URL, ordinary environment,
headers and credential references. Absent members never inherit from the
replaced entry. An empty map clears the MCP map. Tool allowlists,
Subagent selectors and Workflow references cannot supply an activation grant.

Project-authored sources may declare ordinary transport configuration and
enablement under host trust. They cannot declare `sensitive_env` or
`sensitive_headers`, even empty objects. Providers, destinations for provider
credentials and provider credential declarations remain host-owned. Define a
credential-bearing MCP source entirely in user `settings.toml`. A project may
replace its name, but the replacement receives none of the host credentials or
enablement. A project cannot partially redirect a host command/URL while keeping
its authentication. Project resource paths retain CFG-01 containment checks,
including on reload, reconnect and frozen child materialization.

## Explicit credential references

Use `$ENV_VAR` only in declared secret fields. Names follow
`[A-Za-z_][A-Za-z0-9_]*`. There is no `${...}`, substitution inside strings,
shell evaluation, command execution, or document-wide interpolation.

Host `models.toml` retains `api_key: "$RUSTX_OPENAI_API_KEY"`; provider literal
keys are also supported, but references are preferable. Host `settings.toml`:

```toml
[mcp_servers.private-http]
enabled = true
url = "https://service.example/mcp"

[mcp_servers.private-http.sensitive_headers]
Authorization = "$RUSTX_SERVICE_AUTHORIZATION"

[mcp_servers.private-process]
enabled = true
command = "service-mcp"

[mcp_servers.private-process.env]
LOG_LEVEL = "warn"

[mcp_servers.private-process.sensitive_env]
SERVICE_TOKEN = "$RUSTX_SERVICE_TOKEN"
```

The Authorization variable contains the complete value, including `Bearer ` if
required. `env` and `headers` contain ordinary literal values; they do not expand
`$` expressions. Put all sensitive values in the explicitly sensitive maps.
Sensitive MCP fields support references only, not literals. Ordinary and
sensitive maps cannot define the same process variable or HTTP header. Header
comparison is case-insensitive.

At admitted runtime/probe use, the host captures its credential environment once.
Static path discovery and config check/show never capture it. Each admitted source resolves and
caches its own references before the existing spawn/connect owner starts work.
Reconnect uses the same frozen cache. Providers validate required credentials
and construct their adapter on binding, so an unused provider's missing key
does not block a different model. A missing or empty required variable fails
the admitted source with its identity and `env:NAME`, never an empty credential
or an unauthenticated fallback. Disabled sources do not resolve references.

Resolved credentials have redacted Debug/Display and no normal serialization.
Source caches and captured environments are skipped on serialization. Provider
literal projections contain a redaction marker and cannot be replayed as valid
configuration; references retain only names. The native child process owner
transfers admitted credential values through its private child environment,
while the control-channel specification contains references. No credential file
is written. Source setup/catalog errors and normalized provider failures redact
credential values before reaching runtime projections. Sensitive HTTP header
values are marked sensitive in the transport; redirects remain disabled.

## Lifetimes and frontiers

The coordinator checks source admission before opening Python storage or
preparing an MCP source. The shared MCP connection boundary checks the frozen
decision again before credentials, spawn or HTTP setup; selected child
materialization reaches that same boundary. Healthy admitted external Tools
retain existing invocation, cancellation, deadlines, effect certainty and
terminal settlement semantics.

Initial composition stages a complete capability/resource generation before
Session publication. Reload rereads only launch-pinned slots. It stages the
new decisions, credentials and resources off-side. The existing runtime
admission lock serializes capability commit and resource snapshot assignment;
that is the generation publication point. Failed/cancelled staging owns and
settles only its staging resources. Source-local failures remain in capability
availability and do not erase unrelated working sources. A last-known-good
source is carried forward only for an identical binding, never for disablement
or changed credentials/destination.

Ordinary refresh captures source inputs and their base revision together under
the coordinator's publication lock order. This is the staging admission cut:
work admitted before disablement may finish staging, but cannot publish its
old enablement over the newer revision. A stale candidate releases only its
own resources.

Replacing a generation retires its old connection authority. Retirement closes
future reconnect admission atomically without cancelling an already admitted
call. Existing attempt/background leases retain physical ownership until they
settle. If reconnect admission precedes retirement it may finish under that
frozen owner; if retirement wins it cannot establish another connection. An
old healthy connection may finish admitted work, but a stale recovery task may
not reopen a retired source. New admission uses the published activation state.
Trust and provider catalog remain launch-scoped; no generic hot activation or
new recovery supervisor is introduced.

Every initial and replacement MCP generation reaches the same native service
construction seam, where SDK response caching is disabled before semantic
operations. Positive `ttlMs` cannot answer a refresh from cache, nor can an error
be converted into stale cached success. The committed rustX capability snapshot
remains the only semantic capability cache (PR #244).

Runtime Client protocol 23 projects inactive decisions (`disabled`,
`unconfigured`, `untrusted`), enabled/unprepared, preparation failure
(`unavailable`, with a bounded reason), and available (`ready`) through the
existing source availability owner. These states are not Tool identities.

`--no-tools` controls ordinary main-model Tool exposure. Independently admitted
Agent/Workflow source demand may still require preparation. Unreferenced sources
are not prepared; source selection cannot override MCP disablement. Minimal native-only startup needs no Python, `uv`, MCP
executables/endpoints or external-source secrets, even when an unrelated demo
folder exists. It still requires the selected native model's own credential.

OAuth, keyrings, shell secrets, generic hot reload, OS sandboxing, CFG-03 exact
Tool selection and CFG-04 configuration inspection are outside this contract.
