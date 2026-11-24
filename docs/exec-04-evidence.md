# EXEC-04 installed-product evidence

The installed-product branch starts at `6ca38dc3d3ffe87c9c476a15d10642abab8053ba` on `feat/576-execution-boundary`, after PR #600 merged. [Installation](installation.md) lists the entry points and commands. The [Execution Protocol](execution-protocol.md#test-ownership-t01t73-and-optional-o-sum) and the EXEC-01 through EXEC-03 evidence ledgers describe the contracts below the packaging boundary.

## Installed verification

`release/package.py all` builds the Server and Client in separate Cargo target directories. The archives contain the audited dependency graphs and a manifest with the commit, release version, wire revision, and hashes or link targets for every installed file. `release/conformance.py` verifies these facts before starting products. Its real-process replay shares test functions with the Boundary lane. See [the test index](../tests/README.md) for dependencies and lane selection.

| Gate | Reproducible installed proof or outstanding work |
| --- | --- |
| T02 | `explicit_composition_reads_from_client_mount_namespace` and `stdio_mcp_session_runs_and_publishes_from_the_client_mount_view` start real processes with distinct Linux Workspace views. |
| T12 | The installed TUI disconnects from the hosted Session catalog. The installed process harness covers observer disconnect during active work, stdio EOF with one canonical commit, WebSocket drain, and external Host loss with zero replay. `installed_ui_detach_cancels_the_client_with_both_lanes_occupied` parks a physical Client reply on each lane, submits four real watch stops on separate UI sockets and then drops the command owner. It proves that an unrelated settings request succeeds, that the matching cancellation physically stops the command before the data lane resumes, and that the Client releases its journal exactly once. The in-crate detach case separately establishes the exact full control queue by observing a fifth admission refusal. |
| T23 | `standalone_server_commits_a_model_turn_without_a_client_or_helpers` submits one Fake Provider request, stops the standalone Server, moves the Workspace out of view, and reads the committed response on restart. `app_server_archive_stdio_download_works_without_web_or_runtime_attachment` reads a sealed canonical attachment in 32-byte ranges using `session/historyContentRead` and checks the complete archive bytes. The cold Server has no Workspace or Client, loads no Session runtime, and refuses the same read cut under a sibling Session. The separate live `artifact/read` path requires an attachment. |
| T24 | The archive manifests and restricted-PATH smoke checks verify binary ownership, and the build audits production dependencies. The installed WSS test feeds a real Client an older bootstrap revision, requires an explicit refusal, then admits the matching Client on the original reservation. Server handshake refusal also has dedicated protocol tests in EXEC-01 and EXEC-02. |
| T25 | Linux manifest, CLI, Web loopback/browser bootstrap, TUI PTY, owner termination, and missing-Sibling checks. Native macOS runs remain open. |
| T41 | `websocket_and_unix_transports_serve_identical_seeded_operations` replays seeded operations through independent extraction paths. |
| T42 | The independent Client range and grep probe measures the bounded wire responses below. The installed native file-tools scenario also measures both live processes while an Agent reads one line near the end of a 64 MiB source, searches it, and edits a separate file. The installed Bash scenario saves 4 MiB on the Client and reads a line missing from the Server preview through both Read and Grep. See the wire measurements below. |
| T44 | `execution_enrollment_precedes_client_and_workspace_configuration_readiness` exercises minimal enrollment, denies browsing before authentication, lists a descendant despite invalid Workspace configuration, and refuses outside paths and directory symlinks. Cancelling through normal Session deletion removes the catalog entry, prevents further browsing, and reaps the Client while the selected directory and its original bytes survive. The installed Web carrier also exercises the private Product Host picker by browsing, confirming a revision and registering a descendant. `installed_workspace_picker_uses_a_private_client_and_reaps_before_registration` verifies the private lane rejects ordinary transport credentials, refuses outside paths and symlink traversal, succeeds despite malformed Workspace configuration, rejects a replaced directory, and clears its recovery fence before returning. The temporary binding creates no durable Session. |
| T45 | `client_death_after_canonical_host_result_cannot_rewrite_saved_history` keeps a sealed canonical attachment and a published Client Spill in one Conversation. After a real Linux SIGKILL, it verifies removal of the old storage, `NotFound` for the exact locator on the replacement, no Save replay, and the original canonical result. A cold standalone Server then exports the exact attachment bytes with the Workspace absent, with no Spill entry. The sealed image is seeded via the durable fixture API. |
| T46 | Installed Hosted tests include bounded frames, full digest validation, one canonical commit, and an empty Server scratch directory. The Linux baseline below measures Host-to-Server traffic and compares settlement time, Server CPU, peak memory, and physical writes with the real HTTP MCP transport. Concurrent candidates and queue high-water measurements remain open. |
| T49 | `deleted_session_recovers_only_for_cleanup_in_the_original_client` holds a real same-instance reattach until deletion reports cleanup pending. The original Client sends one Revoke receipt. The data lane accepts no requests, the Session stays absent from the catalog, and the original Workspace bytes remain unchanged. The remote SIGKILL fixture covers pending deletion when no authorized receipt is available. |
| T52 | `standalone_server_validates_large_host_stream_without_a_client` uses a real external WebSocket Host against the standalone Server. It verifies individual frame bounds, wrong-digest refusal, one canonical result, and bounded continuation with truthful unavailable output. |
| T65 | Installed Linux tests cover strong Server/Client kills and restart fences. macOS death and settlement evidence remains open. |
| T66 | `exact_store_identity_ignores_origin_and_permissions_but_keeps_runtime_and_source` covers source identity, interpreter and uv path/version, and the FastMCP pin in the Client Store. |
| T67 | `same_store_joins_one_owner_and_waiter_drop_cannot_release_its_stage` holds the physical build, verifies that a second caller joins it, and observes one build plus physical cleanup when the owner cancels. |
| T68 | `independent_stores_reuse_the_exclusive_winner_and_refuse_corruption` verifies unchanged corrupt manifest, source, and interpreter entries. `client_package_watch_refreshes_permissions_and_nested_source_from_the_same_binding` verifies that a permission edit leaves the source digest unchanged and a new Attempt observes the adopted snapshot. |
| T69 | `managed_python_prepares_and_runs_in_the_bound_client_mount` uses real Client-side uv and stdio MCP in a separate mount and verifies that the reusable store survives Session cleanup. |
| T70 | `independent_stores_reuse_the_exclusive_winner_and_refuse_corruption` verifies an exclusive publication winner and identical inode for both owners. In the installed `doctor_probes_selected_managed_python_in_an_owned_client` test, a new process reopens the same published inode, performs a new stdio handshake and catalog discovery, invokes no tools, and runs neither `uv lock` nor `uv sync` again. |

The four evidence ledgers cover all 73 test IDs by number. Open product cases in this table still require their named proof. Local and earlier-commit runs cannot serve as the integrated commit's FULL or Merge verification receipt.

## Linux Client I/O baseline

`test-support/measure_execution_reads.py` ran 18 samples against the separately built release Client. Each fixture size and operation had three samples. The probe uses the current Execution Protocol and measures complete frames for each operation. Startup and shutdown are timed separately in its JSONL output.

| Source size | Range-read median | Grep median |
| --- | ---: | ---: |
| 1 MiB | 1.59 ms | 0.66 ms |
| 8 MiB | 7.03 ms | 3.20 ms |
| 64 MiB | 63.28 ms | 22.56 ms |

Each operation took one RPC. The range read transmitted 97–99 bytes and received 262–265 bytes. Grep transmitted 149 bytes and received 130–132 bytes. The observed peak RSS was 9,736–11,380 KiB for the Client and 18,916–19,056 KiB for the measuring peer. The peer sends the actual wire requests but does not include the Agent runtime. Use its memory figures as a probe baseline. Full Server/Client and native macOS measurements remain separate gates.

## Installed native file tools baseline

Twenty Linux x86-64 runs used the Server and Client extracted from
`5320f6dc7b193b4405cd83c9f7ef8fcd3b38cf15`. Each run makes two Fake Provider
requests and commits one Read, one Grep and one Edit in the same Attempt.
Read and Grep return only the selected marker from a 67,108,884-byte source.
Edit publishes its separate small file. The test checks exact resulting bytes
and bounds the full tool-turn traffic to 16 data exchanges and 16 KiB.
Every sample used nine exchanges, with 1,812 bytes sent and 1,008–1,178 bytes
received, including length prefixes. The largest Client reply was 275 bytes.
Each exchange carries one request and one response. Continuous control polls
and the model HTTP connection are outside this counter.

| Metric | P50 | P95 | Maximum |
| --- | ---: | ---: | ---: |
| Turn settlement | 160.6 ms | 163.9 ms | 165.3 ms |
| Server peak RSS | 80,458 KiB | 81,328 KiB | 81,348 KiB |
| Client peak RSS | 11,842 KiB | 12,000 KiB | 12,056 KiB |

The largest sum of the two lifetime high-water marks was 93,188 KiB. The
processes can peak at different instants. Median CPU usage was 80 ms in
each process at 10 ms counter resolution. The median write deltas were
499,712 bytes for the Server and 229,376 for the Client. These are physical
write counters including traces, canonical storage and filesystem work.
The Client's supervisors are outside these two process counters.

```sh
export RUSTX_INSTALLED_PRODUCTS=/absolute/extracted-products
RUSTX_FILE_TOOL_MEASUREMENT=1 /absolute/process-harness --exact \
  app_server_host::generation::installed_file_tools::installed_native_file_tools_keep_large_results_on_the_client --nocapture
```

Repeat for twenty samples and collect the `FILE_TOOL_MEASUREMENT` records.

## Linux standalone Hosted and HTTP MCP baseline

The two real transports were measured against the standalone Linux x86-64 Server extracted from `94a8ca4b6a6e893b045ec3af3bf4145997e4576d`. The Client was absent, and each Attempt used the Fake Provider and produced one canonical tool result. Twenty fresh Server processes ran per transport and size. Samples alternate transports and sizes. Settlement latency runs from submitting the turn to observing its settlement. CPU and physical write deltas use the same window, and peak RSS covers process lifetime. CPU resolution is 10 ms on the measured system. P95 uses the nearest rank.

| Result size | Transport | P50 settlement | P95 settlement | Median CPU | Median peak RSS | Maximum peak RSS | Median Server writes |
| --- | --- | ---: | ---: | ---: | ---: | ---: | ---: |
| 2 MiB | Hosted | 233.1 ms | 319.4 ms | 80 ms | 84,206 KiB | 85,164 KiB | 495,616 B |
| 2 MiB | HTTP MCP | 173.9 ms | 240.1 ms | 100 ms | 91,366 KiB | 92,136 KiB | 733,184 B |
| 4 MiB | Hosted | 392.6 ms | 451.7 ms | 90 ms | 83,892 KiB | 85,720 KiB | 495,616 B |
| 4 MiB | HTTP MCP | 214.7 ms | 251.0 ms | 110 ms | 95,234 KiB | 95,980 KiB | 733,184 B |
| 16 MiB | Hosted | 1,367.5 ms | 1,443.1 ms | 160 ms | 84,454 KiB | 85,488 KiB | 495,616 B |
| 16 MiB | HTTP MCP | 465.2 ms | 535.2 ms | 180 ms | 120,464 KiB | 121,860 KiB | 733,184 B |

For Hosted, the fixtures sent 35, 67, and 259 requests carrying 2,819,196 B, 5,636,540 B, and 22,541,036 B of serialized JSON. These counts include the deliberately rejected End and exclude WebSocket headers and responses. The HTTP fixture made one business RPC and returned 2,093,056 B, 4,190,208 B, or 16,773,120 B of tool text before JSON and HTTP framing. It uses the HTTP path's bounded parsing behavior. Hosted results used bounded frames and retained the same peak-memory band as payload size increased. Both suites check the scratch directory before shutdown, enforce a bounded preview, and verify that no Client started. Server write counts include trace and canonical writes. The cases use different transports and fixtures, with no like-for-like comparison of wire bytes or upload throughput. A second run starts two real Host candidates, leaves the loser active after four chunks, and completes the winner. The loser is refused after the winner claims the result. With no Client, there are no Save requests or private Save files. Across twenty samples each at 2 and 16 MiB, the loser adds six requests and 353,241 or 353,242 JSON bytes. Both sizes produce 17 durable events, one canonical tool result, and a 416,256-byte Conversation database after shutdown.

| Winning body | P50 settlement | P95 settlement | Median peak Server RSS | Maximum peak Server RSS |
| --- | ---: | ---: | ---: | ---: |
| 2 MiB | 265.7 ms | 301.8 ms | 81,030 KiB | 82,132 KiB |
| 16 MiB | 1,435.4 ms | 1,497.9 ms | 81,912 KiB | 83,308 KiB |

Run `app_server_host::standalone_server_arbitrates_two_streaming_host_candidates_without_a_client` with the same `RUSTX_HOST_MEASUREMENT_MIB` option. Capture `HOST_MEASUREMENT` and `HOST_JOURNAL_MEASUREMENT`. Bound Client candidate cleanup is covered by the real Save receipt and Client file assertions in the EXEC-03 evidence.

To reproduce, prepare the `process` harness and set `RUSTX_INSTALLED_PRODUCTS` to a directory containing the three extracted products under `rustx-server`, `rustx-client`, and `rustx`. Repeat the two commands 20 times each for `mib=2`, `4`, and `16` and use the `HOST_MEASUREMENT` and `HTTP_MCP_MEASUREMENT` records. `getconf CLK_TCK` gives the CPU counter frequency.

```sh
export RUSTX_REQUIRE_PROVIDER_EMULATOR=1
export RUSTX_INSTALLED_PRODUCTS=/absolute/extracted-products
mib=16
RUSTX_HOST_MEASUREMENT_MIB="$mib" /absolute/process-harness --exact \
  app_server_host::standalone_server_validates_large_host_stream_without_a_client --nocapture
RUSTX_HTTP_MCP_MEASUREMENT_BYTES="$((mib * 1048576 - 4096))" /absolute/process-harness --exact \
  app_server::near_limit_http_mcp_body_commits_only_a_bounded_preview_without_a_client --nocapture
```

## Installed pure-model and cold history baseline

Twenty Linux x86-64 runs used the standalone Server from `5320f6dc7b193b4405cd83c9f7ef8fcd3b38cf15`. Each run commits one warm-up turn using the Fake Provider, waits for native Idle, and measures a second turn until the matching Attempt settles. It then restarts the Server, with the Workspace moved out of view, and measures one history page read after initialization. The continuation run makes one model request and the history read makes zero. No Execution Client or execution helpers are present.

| Operation | P50 | P95 | Maximum |
| --- | ---: | ---: | ---: |
| Warm model turn through settlement | 40.9 ms | 54.7 ms | 55.4 ms |
| Cold history page | 15.6 ms | 19.7 ms | 19.9 ms |

The warm turn used a median of 20 ms CPU at 10 ms counter resolution and wrote 147,456 physical bytes in every sample. Server lifetime peak RSS was 82,240 KiB median and 82,872 KiB maximum. History pages contained 6,651–6,653 bytes of serialized window data, excluding the outer response and framing. History timing excludes process startup and initialization. P95 uses the nearest rank.

```sh
export RUSTX_INSTALLED_PRODUCTS=/absolute/extracted-products
RUSTX_MODEL_MEASUREMENT=1 /absolute/process-harness --exact \
  app_server::standalone_server_commits_a_model_turn_without_a_client_or_helpers --nocapture
```

Repeat for twenty samples and collect the `MODEL_MEASUREMENT` records.

## Installed Bash capture and continuation baseline

Twenty Linux x86-64 samples drive native Agent Bash, Read and Grep through
the installed pair. Bash captures 4,194,332 bytes on the Client. The Fake
Provider derives the locator from the actual bounded Bash result. Read and
Grep each fetch the marker at the middle of that capture with one targeted
request. The marker is absent from the initial preview. The three provider
requests include the first invocation, Bash continuation, and both selected
results. There is one data connection with 14 serialized exchanges. These
carry 4,475–4,489 bytes to the Client and 35,558 bytes to the Server, including
the Bash preview and framing. The largest reply is 33,298 bytes.

| Metric | P50 | P95 | Maximum |
| --- | ---: | ---: | ---: |
| Turn settlement | 140.0 ms | 149.5 ms | 152.3 ms |
| Server peak RSS | 81,900 KiB | 82,856 KiB | 82,928 KiB |
| Client peak RSS | 11,770 KiB | 11,880 KiB | 11,920 KiB |

Set `RUSTX_FILE_TOOL_MEASUREMENT=1` and run
`app_server_host::generation::installed_file_tools::installed_bash_continuation_reads_only_the_selected_client_output`
with the same extracted products as the ordinary file-tool baseline. This
measures Agent-native Bash. The streaming test below measures the public
command endpoint instead.

## Installed Bash streaming baseline

Twenty Linux x86-64 runs used the Server and Client from the `5320f6dc7b193b4405cd83c9f7ef8fcd3b38cf15` bundle. The command uses the public `command/exec` endpoint once in each already-bound Session. Bash produces the prefix, waits for the test to receive it and send a line using `command/exec/write`, then emits a 128 KiB body plus a suffix. Every run received exactly 131,081 bytes in 18 ordered output notifications, with no truncation and a successful terminal response. First byte means the first nonempty output notification at the App Server consumer. Settlement includes the gated input exchange. Measurements exclude initialization and do not include Agent-native Bash dispatch.

| Operation | P50 | P95 | Maximum |
| --- | ---: | ---: | ---: |
| Bash first output byte | 22.4 ms | 23.2 ms | 23.2 ms |
| Bash settlement | 93.6 ms | 97.9 ms | 113.3 ms |

```sh
export RUSTX_INSTALLED_PRODUCTS=/absolute/extracted-products
RUSTX_BASH_MEASUREMENT=1 /absolute/process-harness --exact \
  app_server::explicit_composition_launches_and_reaps_the_sibling_client --nocapture
```

Repeat for twenty samples and collect the `BASH_MEASUREMENT` records. The output notification count is on the App Server connection. It does not measure the private Execution lane's frame count or queue high-water marks.

## Installed Managed Python restart reuse

The Linux diagnostic probe opens the Workspace through an owned Client. A successful cold run writes one `uv lock` and one `uv sync` record after the wheel cache has been populated. The reopened run uses the exact Store entry and writes no additional build record. Both real stdio launches verify the same tool catalog, clean up their diagnostic roots, and leave no Session or business tool call. The installed sample took 6.8 ms for the plan, 2,391.2 ms for cold preparation through discovery and shutdown, and 2,040.0 ms for a fresh process reopening the prepared Store through discovery and shutdown. These are single-fixture elapsed measurements after wheel-cache population. The owner-level race tests above measure build counts and independently fence publication.
