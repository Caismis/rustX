#!/bin/sh
# Linux distinct-mount-view proofs for the execution boundary (T02/T42):
#
#   test-support/prove_execution_mount_view.sh
#
# It exports RUSTX_MOUNT_VIEW_PROOF=1, runs the opt-in App Server
# and Client proofs, and repeats the Agent tool-plane test with isolation:
#
#  - `filesystem_operations_use_the_private_mount_view` spawns the real
#    rustx-execution-client under
#
#      bwrap --ro-bind / / --dev /dev --bind <physical_dir> <workspace> --tmpfs <scratch> --
#
#    so the same absolute workspace path resolves to a different physical
#    directory inside the client's mount namespace than the server sees;
#    the server asserts the marker stays NotFound on its side before and
#    after the remote range RPC.
#
#  - `explicit_composition_reads_from_client_mount_namespace` runs the real
#    two-process App Server composition, where a test-only sibling shell
#    launcher inside the fixture's private bin directory wraps the staged
#    Client in the same bwrap layout:
#
#      bwrap --ro-bind / / --dev /dev --bind <physical> <workspace> --tmpfs <bin>/tmp
#
#    with TMPDIR=<bin>/tmp so the Client's own scratch is a private tmpfs
#    outside the workspace: host /tmp and the bootstrap socket stay as the
#    Server sees them. `--dev /dev` supplies usable device nodes: a plain
#    `--ro-bind / /` leaves /dev nodev, and the supervisor's `/dev/null`
#    stdin open fails with EACCES. A tmpfs is required because the staging tree may sit
#    on a filesystem without POSIX permissions, and the supervisor's
#    owner-only environment-snapshot check must hold. The staged `bash-supervisor`
#    sits beside the Client as in production. Product fs/readFile,
#    fs/getMetadata, fs/readDirectory and fs/writeFile calls serve the marker
#    from the Client's private mount view, and a foreground `command/exec`
#    for the bound Session runs `/bin/bash -c 'cat marker; printf client >
#    process-marker'` there: its output is the rewritten marker, and the
#    result file exists only in the physical directory. The Server's own view
#    of the same absolute workspace path stays NotFound for the marker and the
#    result, the Server-only decoy stays unreachable, and an unbound Session's
#    command is refused as ExecutionUnavailable. Cold attach uses a deadline
#    from the Client's rustx.toml while the Server's copy is invalid. An
#    explicit reconcile captures the Client's next authored revision.
#    The launcher is a fixture
#    artifact only; production resolves the sibling executable directly and
#    has no override. This proves the App Server command/exec argv path.
#
#  - The background Bash continuation test runs native Write, Read, Bash and
#    Grep in the Client view. A FIFO holds the operation while Read/Grep
#    inspect the committed prefix. The final output uses the same locator.
#  - Workspace Python resolves and runs the interpreter from a Client-only
#    overlay, using an actual uv and the existing supervisor.
#  - Stdio MCP reads the Client-only source declaration and launches the peer
#    in that same mount. One foreground call crosses multiple execution pages.
#
# The product proof runs only against the real prepared artifacts — the
# process harness plus its metadata-verified sibling Execution Client — so
# preparation runs first. Requires Linux and bubblewrap; no CI privileges
# assumed.
set -eu
cd "$(dirname "$0")/.."
if ! command -v bwrap >/dev/null 2>&1; then
    echo "error: the mount-view proof requires bubblewrap (bwrap)" >&2
    exit 2
fi
python3 test-support/prepare_native_tests.py -- \
    cargo test --all-features --locked --no-run --lib --test process --test subagent
python3 test-support/prepare_native_tests.py -- \
    cargo test -p rustx-execution-client --locked --no-run --test bootstrap
export RUSTX_MOUNT_VIEW_PROOF=1
cargo test -p rustx-execution-client --locked \
    --test bootstrap filesystem_operations_use_the_private_mount_view \
    -- --exact --ignored --nocapture
cargo test -p rustx-execution-client --locked \
    --test bootstrap workspace_python_prepares_and_runs_in_the_private_mount_view \
    -- --exact --ignored --nocapture
cargo test -p rustx --all-features --locked \
    --test process app_server::explicit_composition_reads_from_client_mount_namespace \
    -- --exact --ignored --nocapture
cargo test -p rustx --all-features --locked \
    --test process app_server::stdio_mcp_session_runs_and_publishes_from_the_client_mount_view \
    -- --exact --ignored --nocapture
cargo test -p rustx --all-features --locked \
    --test process app_server::managed_python_prepares_and_runs_in_the_bound_client_mount \
    -- --exact --ignored --nocapture
cargo test -p rustx --all-features --locked \
    --test process app_server::managed_python_revokes_only_its_guarded_unpublished_root \
    -- --exact --ignored --nocapture
cargo test -p rustx --all-features --locked --lib \
    tools::background::tests::background_live_output_is_readable_while_running_and_reused_at_settlement \
    -- --exact --nocapture
cargo test -p rustx --all-features --locked --test subagent \
    a_real_child_executes_native_tools_in_the_frozen_nested_workspace -- --nocapture
cargo test -p rustx --all-features --locked --lib \
    bound_configuration_load_and_reconcile_without_any_server_workspace_directory \
    -- --ignored --nocapture
cargo test -p rustx --all-features --locked --lib \
    bound_reconcile_keeps_the_admitted_agent_when_its_workspace_source_is_lost \
    -- --ignored --nocapture
cargo test -p rustx --all-features --locked --lib \
    client_source_watch_reloads_guidance_and_skill_metadata_after_registration \
    -- --ignored --nocapture
cargo test -p rustx --all-features --locked --lib \
    bound_skill_watch_keeps_the_failed_target_observable \
    -- --ignored --nocapture
cargo test -p rustx --all-features --locked --lib \
    session_input_commit_publishes_its_bound_skill_dependencies \
    -- --ignored --nocapture
cargo test -p rustx --all-features --locked --lib \
    newer_bound_skill_observation_fences_an_older_session_input \
    -- --ignored --nocapture
cargo test -p rustx --all-features --locked --lib \
    a_bound_source_rechecks_after_its_final_document_read_fails \
    -- --ignored --nocapture
cargo test -p rustx --all-features --locked --lib \
    bound_workspace_mcp_catalog_uses_client_definitions_and_server_network \
    -- --ignored --nocapture
retirement_client=$(cargo build -p rustx-execution-client --features test-support --locked \
    --bin rustx-execution-client --message-format=json | python3 -c '
import json, sys
for line in sys.stdin:
    row = json.loads(line)
    if row.get("reason") == "compiler-artifact" and row.get("executable"):
        print(row["executable"])
')
test -x "$retirement_client"
RUSTX_WATCH_RETIREMENT_PROOF_CLIENT="$retirement_client" \
    cargo test -p rustx --all-features --locked --lib \
    replacing_a_skill_refreshes_the_catalog_while_native_watch_retirement_is_held \
    -- --ignored --nocapture
