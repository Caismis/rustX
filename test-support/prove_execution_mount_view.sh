#!/bin/sh
# Manual opt-in evidence for the Linux distinct-mount-view read (a read-only
# T02 fragment of the execution-boundary plan), at both implemented layers:
#
#   test-support/prove_execution_mount_view.sh
#
# It exports RUSTX_MOUNT_VIEW_PROOF=1 and runs exactly the #[ignore]d proofs:
#
#  - `range_read_from_private_mount_view` spawns the real
#    rustx-execution-client under
#
#      bwrap --ro-bind / / --ro-bind <physical_dir> <workspace> --
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
#    command is refused as ExecutionUnavailable. The launcher is a fixture
#    artifact only; production resolves the sibling executable directly and
#    has no override. This proves the App Server's command/exec argv path only,
#    not native Agent Bash.
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
    cargo test --all-features --locked --no-run --lib --test process
export RUSTX_MOUNT_VIEW_PROOF=1
cargo test -p rustx-execution-client --locked \
    --test bootstrap range_read_from_private_mount_view \
    -- --exact --ignored --nocapture
cargo test -p rustx --all-features --locked \
    --test process app_server::explicit_composition_reads_from_client_mount_namespace \
    -- --exact --ignored --nocapture
