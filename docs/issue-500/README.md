# MCP Resource tools (#500)

Base: `ef3f917038a34b0c039fc62668ba11eca1a774b7` (`origin/main`, including #498/#499).

## Authority and ownership

`SourceToolSelection::All` grants ordinary Tools and the source's negotiated MCP
Resources. `Exact` grants only named Tools. Connection demand, configured bindings,
Workflow dependencies and cached Tool hints are not Resource permission.

The connection handshake captures `McpNegotiatedCapabilities` on the concrete
transport generation. `McpDiscoveryOutcome` pairs those facts with the canonical
Tool catalog, which may be empty. The existing connection catalog lock and
invalidation epoch validate the complete observation. The capability coordinator
publishes it with the existing candidate commit, retirement and last-known-good
rules. No Resource enumeration occurs during discovery.

The available catalog records negotiated Resource sources; the resolved Agent
profile intersects them with its own ready, source-wide selection. The immutable
snapshot and its three helper executors capture that exact authority. A change to
the authorized set participates in candidate equality even when helper names and
schemas remain unchanged. The manifest includes resource-only selected sources.

Child and Workflow Agent specifications freeze Resource source identities
separately from exact Tool identities, plus only their required physical bindings.
Selected materialization prepares the union and rejects missing Resources before
Ready, settling prepared siblings. Private child IPC is v36 and semantic profile
framing is v6; no legacy decoding path is retained.

Each helper acquires the existing runtime generation lease. Transport resolution
and resource capability validation use the connection's existing discovery owner;
replacement facts become available to the normal adoption boundary. The request
then crosses `dispatch_owned_request`, its call gate, outbound ownership seam,
progress lease and cancellation arbitration. A replacement without Resources
fails before `resources/*` dispatch. An in-flight Attempt retains its declaration;
only a subsequent safe snapshot adoption changes the three-tool group. No request
is replayed after an uncertain outcome.

## Observable capability matrix

| Negotiated source state | Selection | Ordinary Tools | Resource helpers |
| --- | --- | --- | --- |
| No admitted source | None | None | 0 |
| Tools only | `all` or exact names | Selected Tools | 0 |
| Resources only | `all` | None; no `tools/list` request | 3 |
| Tools and Resources, empty Tool catalog | `all` | None | 3 |
| Resources with empty descriptor list or only templates | `all` | As advertised | 3 |
| Several Resource sources | `all` | Selected Tools | 3 total |
| Resources | Exact Tool names or `[]` | Exact Tools only | 0 |
| Configured, Workflow-demanded, or another Agent's source | Unselected | None for this Agent | 0 |
| Failed, pending handshake, or cached Tool hint | `all` | Existing readiness rules | 0 from unvalidated facts |
| Resource-only child source | Frozen source-wide authority | None | 3 after verification |

Both modern MCP 2026-07-28 and legacy initialize handshakes support resource-only
servers. The protocol adapter exposes typed `resources/list`,
`resources/templates/list` and `resources/read` over stdio and Streamable HTTP.

## Bounds and output

The native names are `list_mcp_resources`, `list_mcp_resource_templates` and
`read_mcp_resource`; remote Tools cannot claim these names. They are composed
automatically and cannot be selected through `tools.builtin`.

Named lists return one page. Aggregate lists return one page per authorized
source in identity order, at most 32 sources and four concurrent requests, with
independent statuses/cursors. Larger source sets require a named call. No helper
automatically follows a cursor; an echoed next cursor, invalid cursor, oversized
page or excess descriptors fails explicitly. A source failure remains visible
and cannot become an empty success. Aggregate cancellation and uncertainty retain
their honest terminal status after every admitted sibling settles.

Limits are 4 KiB per cursor, 8 KiB per URI, 256 descriptors per page, 16 KiB per
content block's MIME/metadata, 64 read blocks, 16 MiB aggregate decoded read
content and 8 MiB per decoded blob. Resource pages also obey the existing model
result byte bound. Larger text and aggregate output use managed output spill and
projection; decoded blobs use the existing artifact store, never model-visible
base64. A URI, including `file://`, is passed only to its authorized remote peer;
no enumeration whitelist or local filesystem interpretation is introduced.

Resources are untrusted ordinary Tool results. Their bodies enter canonical
history/model context only when a Tool call is performed and committed. No new UI,
App Server browsing RPC, cache, subscription, background scheduler or automatic
context injection is introduced.

## Test-to-invariant map

| Tests | Invariant |
| --- | --- |
| `negotiated_matrix_skips_unsupported_tool_lists_and_never_enumerates_at_startup` | Actual request counts for capability matrix; resource-only and empty catalogs |
| `resource_only_stdio_negotiates_modern_and_legacy_without_tools_list` | Real stdio handshakes, text/blob/template requests, no fabricated Tool catalog |
| `resource_only_sources_publish_three_helpers_only_for_source_wide_authority` | Model declarations, exact selection, demand isolation, manifest, one group, entitlement narrowing |
| `resource_candidate_epoch_and_last_good_publication_are_atomic` | Stale candidate rejection and immutable last-known-good snapshot |
| `negotiated_resource_changes_wait_for_the_existing_catalog_adoption_boundary` | Pending handshake, late negotiation and capability changes use existing safe publication |
| `native_resource_helpers_route_only_frozen_sources_with_one_identity_ordered_group` | Actual helper dispatch, ordered aggregate pages, explicit routing and denied peer request counts |
| `resource_http_pages_templates_direct_uris_blobs_and_errors_are_bounded` | Pagination, opaque URIs, MIME/multiple blocks, errors, spill and artifacts |
| `resource_cancellation_before_and_after_http_dispatch_settles_ownership_without_replay` | Channel-ordered cancellation, zero pre-dispatch requests, HTTP release and no replay |
| `replacement_capability_removal_updates_existing_catalog_and_refuses_resource_dispatch` | Reconnect removal, frozen-call refusal and existing catalog reconciliation |
| `resource_only_child_uses_frozen_binding_despite_new_settings_and_sibling` | Zero-tool child, exact binding freeze, mutable settings and sibling isolation |
| `resource_capability_mismatch_settles_required_siblings_before_returning` | Required child source failure closes prepared siblings |
| `resource_authority_follows_effective_workflow_agent_selection_and_frozen_generation` | Workflow All/Exact/replacement semantics and frozen generation isolation |
| `resource_body_enters_context_only_after_one_committed_normal_tool_result` | Exactly one normal Tool result and final Attempt event; no startup content injection or discovery-triggered model request |
| `remote_tools_cannot_shadow_native_resource_helpers` | Reserved native helper identity across MCP and Managed Python |
| `resources_executor::tests` | Dispatch authorization, aggregate partial errors/uncertainty and bounded spill |

Validation results and the final head are recorded in the pull request.
