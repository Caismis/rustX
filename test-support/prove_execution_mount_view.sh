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
#    Client in the same bwrap layout. Product fs/readFile, fs/getMetadata
#    and fs/readDirectory calls serve the marker from the Client's private
#    mount view while the Server's own view of the same absolute workspace
#    path proves the marker stays NotFound and the Server-only decoy stays
#    unreachable, before and after. The launcher is a fixture artifact
#    only; production resolves the sibling executable directly and has no
#    override.
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
