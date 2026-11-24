# EXEC-04 installed-product evidence

The installed-product branch starts at `6ca38dc3d3ffe87c9c476a15d10642abab8053ba` on `feat/576-execution-boundary`, after PR #600 merged. [Installation](installation.md) lists the entry points and commands. The [Execution Protocol](execution-protocol.md#test-ownership-t01t73-and-optional-o-sum) and the EXEC-01 through EXEC-03 evidence ledgers describe the contracts below the packaging boundary.

## Installed verification

`release/package.py all` builds the Server and Client in separate Cargo target directories. The archives contain the audited dependency graphs and a manifest with the commit, release version, wire revision, and hashes or link targets for every installed file. `release/conformance.py` verifies these facts before starting products. Its real-process replay shares test functions with the Boundary lane. See [the test index](../tests/README.md) for dependencies and lane selection.

| Gate | Reproducible installed proof or outstanding work |
| --- | --- |
| T02 | `explicit_composition_reads_from_client_mount_namespace` and `stdio_mcp_session_runs_and_publishes_from_the_client_mount_view` start real processes with distinct Linux Workspace views. |
| T12 | The installed TUI exercises an observer disconnect on the same Server. The installed combination of UI detach and active command settlement still needs proof. |
| T23 | Cold Session listing with both recorded directories missing and pure model work run from the standalone Server archive. Cold reads of completed delivered artifacts remain to be exercised against an installed Server. |
| T24 | The archive manifests and restricted-PATH smoke checks verify binary ownership, and the build audits production dependencies. Wire handshake refusal has separate protocol tests in EXEC-01 and EXEC-02. |
| T25 | Linux manifest, CLI, Web loopback/browser bootstrap, TUI PTY, owner termination, and missing-Sibling checks. Native macOS runs remain open. |
| T41 | `websocket_and_unix_transports_serve_identical_seeded_operations` replays seeded operations through independent extraction paths. |
| T42 | Linux read and grep measurements below cover the installed Client with a measuring peer. Installed Spill and full-Agent Server memory measurements remain open. |
| T44 | `execution_enrollment_precedes_client_and_workspace_configuration_readiness` exercises minimal enrollment. The installed UI lists operator-configured locations. Interactive first-folder selection with a temporary bound Session and cancellation cleanup remains open. |
| T45 | Installed Client destruction and honest unavailability have process coverage. The combined durable delivered-content and temporary-Spill scenario still needs an installed fixture. |
| T46 | Hosted and HTTP MCP use separate real-protocol fixtures. An installed Host memory, write amplification, and queue profile remains open. |
| T49 | SIGKILL tests include offline deletion with cleanup pending. Installed later cleanup without Session or Workspace resurrection remains open. |
| T52 | `standalone_server_validates_large_host_stream_without_a_client` uses a real external WebSocket Host against the standalone Server. It verifies individual frame bounds, wrong-digest refusal, one canonical result, and bounded continuation with truthful unavailable output. |
| T65 | Installed Linux tests cover strong Server/Client kills and restart fences. macOS death and settlement evidence remains open. |
| T69 | `managed_python_prepares_and_runs_in_the_bound_client_mount` uses real Client-side uv and stdio MCP in a separate mount and verifies that the reusable store survives Session cleanup. |

The remaining T-ID mapping must be reconciled with discovered tests at the final integrated commit. Local and earlier-commit runs cannot serve as that commit's FULL or Merge verification receipt.

## Linux Client I/O baseline

`test-support/measure_execution_reads.py` ran 18 samples against the separately built release Client. Each fixture size and operation had three samples. The probe uses the current Execution Protocol and measures complete frames for each operation. Startup and shutdown are timed separately in its JSONL output.

| Source size | Range-read median | Grep median |
| --- | ---: | ---: |
| 1 MiB | 1.59 ms | 0.66 ms |
| 8 MiB | 7.03 ms | 3.20 ms |
| 64 MiB | 63.28 ms | 22.56 ms |

Each operation took one RPC. The range read transmitted 97–99 bytes and received 262–265 bytes. Grep transmitted 149 bytes and received 130–132 bytes. The observed peak RSS was 9,736–11,380 KiB for the Client and 18,916–19,056 KiB for the measuring peer. The peer sends the actual wire requests but does not include the Agent runtime. Use its memory figures as a probe baseline. Full Server/Client measurements, Hosted versus HTTP measurements, and native macOS measurements are separate gates.
