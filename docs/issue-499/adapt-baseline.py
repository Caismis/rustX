#!/usr/bin/env python3
"""Install the current concrete startup benchmark into archived main #498.

Usage: python3 docs/issue-499/adapt-baseline.py /absolute/baseline/tree
Archive 00978bbe3072b211efe835950f7f99666e1cc02d first. This copies exactly the
current workload, runtime, adapter and transport fixture, with one API seam:
prepare_initial uses main's prepare_candidate instead of the new initial-live
entry point. No baseline production behavior or timing policy is changed.
The JSON written to stdout records all copied bytes and the explicit seam.
"""

from hashlib import sha256
import json
from pathlib import Path
import sys


def digest(data: bytes) -> str:
    return sha256(data).hexdigest()


def main() -> None:
    if len(sys.argv) != 2:
        raise SystemExit(__doc__)
    source_root = Path(__file__).resolve().parents[2]
    baseline = Path(sys.argv[1]).resolve(strict=True)
    if baseline == source_root:
        raise SystemExit("The destination must be a separate archived baseline tree.")
    relative = Path("src/capabilities/coordinator/tests/performance_startup.rs")
    coordinator_path = baseline / "src/capabilities/coordinator.rs"
    original_coordinator = coordinator_path.read_text()
    if "mod startup_performance_tests" in original_coordinator:
        raise SystemExit("The baseline has already been adapted.")
    if "prepare_startup_candidate" in original_coordinator:
        raise SystemExit("The destination contains redesigned startup product code.")
    if "pub async fn prepare_candidate(" not in original_coordinator:
        raise SystemExit("The destination does not expose the expected baseline API.")
    source = (source_root / relative).read_text()
    start = source.index("async fn prepare_initial(")
    end = source.index("fn coordinator(", start)
    baseline_prepare = """async fn prepare_initial(
    owner: &CapabilityCoordinator,
) -> Result<super::PreparedCapabilityCandidate, super::CapabilityPreparationError> {
    owner.prepare_candidate().await
}

"""
    adapted = source[:start] + baseline_prepare + source[end:]
    adapted = adapted.replace(
        "use std::collections::{BTreeMap, BTreeSet};",
        "use std::collections::BTreeMap;",
        1,
    )
    if "prepare_startup_candidate" in adapted or "BTreeSet" in adapted:
        raise SystemExit("The prepare_initial seam did not match the current fixture.")
    model_relative = relative.with_name("performance_startup_model.rs")
    model = (source_root / model_relative).read_bytes()
    (baseline / relative).write_text(adapted)
    (baseline / model_relative).write_bytes(model)
    hook = (
        '\n#[cfg(all(test, feature = "mcp-fixture"))]\n'
        '#[path = "coordinator/tests/performance_startup.rs"]\n'
        'mod startup_performance_tests;\n'
    )
    coordinator_path.write_text(original_coordinator + hook)
    print(json.dumps({
        "expected_baseline_revision": "00978bbe3072b211efe835950f7f99666e1cc02d",
        "semantics": "unchanged baseline production; same concrete measurement workload and runtime fixture",
        "api_seams": [
            "prepare_initial: replace prepare_startup_candidate(default Skill outcome, empty required set, armed shared deadline, uncancelled owner) with prepare_candidate(); preserve baseline failure semantics",
            "remove import used only by that signature and append one cfg(test,mcp-fixture) module hook",
        ],
        "fixtures": [
            {
                "path": str(relative),
                "source_sha256": digest(source.encode()),
                "installed_sha256": digest(adapted.encode()),
            },
            {
                "path": str(model_relative),
                "source_sha256": digest(model),
                "installed_sha256": digest(model),
            },
        ],
        "coordinator_before_sha256": digest(original_coordinator.encode()),
        "coordinator_after_sha256": digest((original_coordinator + hook).encode()),
    }, indent=2))


if __name__ == "__main__":
    main()
