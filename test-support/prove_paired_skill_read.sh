#!/bin/sh
# Verify the paired #612/#613 release against a real Client in a separate
# filesystem view. Run only from a checkout containing both changes:
#
#   sh test-support/prove_paired_skill_read.sh
#
# This stages matching native binaries and checks that the exact ignored test
# exists before running it. CARGO_TARGET_DIR and Cargo profile settings remain
# available to callers so an existing prepared build can be reused.
set -eu
cd "$(dirname "$0")/.."
if [ ! -x /usr/bin/bwrap ]; then
    echo "error: the paired release proof requires Linux and /usr/bin/bwrap" >&2
    exit 2
fi
records=$(mktemp)
trap 'rm -f "$records"' EXIT
RUSTX_PREPARED_JSON_OUT="$records" python3 test-support/prepare_native_tests.py -- \
    cargo test -p rustx --all-features --locked --no-run --lib
harness=$(python3 - "$records" <<'PY'
import json
import sys

with open(sys.argv[1], encoding="utf-8") as stream:
    rows = (json.loads(line) for line in stream)
    paths = [
        row["executable"] for row in rows
        if row.get("reason") == "compiler-artifact"
        and row.get("target", {}).get("name") == "rustx"
        and "lib" in row.get("target", {}).get("kind", ())
        and row.get("profile", {}).get("test")
        and row.get("executable")
    ]
if len(paths) != 1:
    raise SystemExit(f"expected one staged rustx library harness, got {paths!r}")
print(paths[0])
PY
)
selector=local_runtime::session_runtime_manager::tests::configuration::session_creation::paired_release::synced_catalog_location_and_attachment_use_real_read
if ! "$harness" "$selector" --exact --ignored --list | grep -Fx "$selector: test" >/dev/null; then
    echo "error: the paired release test is absent from the staged harness" >&2
    exit 1
fi
RUSTX_MOUNT_VIEW_PROOF=1 "$harness" "$selector" --exact --ignored --nocapture
