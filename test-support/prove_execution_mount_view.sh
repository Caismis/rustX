#!/bin/sh
# Manual opt-in evidence for the Linux distinct-mount-view read (a read-only
# T02 fragment of the execution-boundary plan).
#
#   test-support/prove_execution_mount_view.sh
#
# It exports RUSTX_MOUNT_VIEW_PROOF=1 and runs exactly the #[ignore]d
# `range_read_from_private_mount_view` integration test. The test spawns the
# real rustx-execution-client under
#
#   bwrap --ro-bind / / --ro-bind <physical_dir> <workspace> -- <client> ...
#
# so the same absolute workspace path resolves to a different physical
# directory inside the client's mount namespace than the server sees, and the
# server asserts the marker stays NotFound on its side before and after the
# remote range RPC. Requires Linux and bubblewrap; no CI privileges assumed.
set -eu
cd "$(dirname "$0")/.."
if ! command -v bwrap >/dev/null 2>&1; then
    echo "error: the mount-view proof requires bubblewrap (bwrap)" >&2
    exit 2
fi
exec env RUSTX_MOUNT_VIEW_PROOF=1 cargo test \
    -p rustx-execution-client --locked \
    --test bootstrap range_read_from_private_mount_view \
    -- --exact --ignored --nocapture
