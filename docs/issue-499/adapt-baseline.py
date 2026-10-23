#!/usr/bin/env python3
"""Install only the #499 timing fixture into an archived main baseline containing #498.

Usage: python3 docs/issue-499/adapt-baseline.py /absolute/baseline/tree
The destination must already be an archive of the baseline commit, not a product
checkout with new startup APIs. No production behavior is changed.
"""
from pathlib import Path
import sys

root = Path(__file__).resolve().parents[2]
baseline = Path(sys.argv[1]).resolve()
relative = Path("src/capabilities/coordinator/tests/performance_startup.rs")
text = (root / relative).read_text()
text = text.replace("use std::collections::{BTreeMap, BTreeSet};", "use std::collections::BTreeMap;")
text = text.replace("use crate::tools::mcp::catalog_cache::{CatalogCacheKey, McpCatalogCache};\n", "")
start = text.index("    let cache_eligible_hits = bindings")
end = text.index("    let tool_runtime =", start)
text = text[:start] + "    let cache_eligible_hits = 0;\n" + text[end:]
start = text.index("async fn prepare_initial(")
end = text.index("fn coordinator(", start)
text = text[:start] + """async fn prepare_initial(
    owner: &CapabilityCoordinator,
) -> Result<super::PreparedCapabilityCandidate, super::CapabilityPreparationError> {
    owner.prepare_candidate().await
}

""" + text[end:]
start = text.index("    let initial_connecting_sources = candidate")
end = text.index("    let snapshot = owner.commit(candidate)", start)
text = text[:start] + "    let initial_connecting_sources = 0;\n    let initial_cached_sources = 0;\n" + text[end:]
start = text.index("    let readiness = owner.startup_readiness();")
end = text.index("    let (\n        (\n            discovery_settled_ms", start)
text = text[:start] + """    let settle = async {
        (
            milliseconds(start.elapsed()),
            true,
            (initially_ready > 0).then_some(composition_ready_ms),
            (initially_ready == servers).then_some(composition_ready_ms),
            None,
        )
    };
""" + text[end:]
text = text.replace("cache_lookups: servers,", "cache_lookups: 0,")
text = text.replace("            assert!(owner.settle_failed_startup().await.is_empty());", "            owner.drain_conversation_owned().await.unwrap();")
text = text.replace("    let lifecycle = owner.preparation_lifecycle().unwrap();\n    assert!(lifecycle.activate());\n", "")
text = text.replace("    assert!(lifecycle.begin_drain());\n", "")
for unavailable in ["CatalogCache", "Connecting", "prepare_startup_candidate", "startup_readiness()", "prepare_ready_captured_inputs"]:
    assert unavailable not in text, unavailable
(baseline / relative).write_text(text)
model_relative = relative.with_name("performance_startup_model.rs")
model = (root / model_relative).read_text()
start = model.index("    let cache_eligible_hits = bindings")
end = model.index("    let tool_runtime =", start)
model = model[:start] + "    let cache_eligible_hits = 0;\n" + model[end:]
model = model.replace("CapabilitySourceState, ", "")
start = model.index("    let initial_connecting_sources = owner")
end = model.index("    let adapter:", start)
model = model[:start] + "    let initial_connecting_sources = 0;\n" + model[end:]
model = model.replace("            assert!(owner.settle_failed_startup().await.is_empty());", "            owner.drain_conversation_owned().await.unwrap();")
start = model.index("    if let Some(mut readiness) = owner.startup_readiness()")
end = model.index("    runtime.shutdown().await.unwrap();", start)
model = model[:start] + model[end:]
(baseline / model_relative).write_text(model)
coordinator = baseline / "src/capabilities/coordinator.rs"
hook = '\n#[cfg(all(test, feature = "mcp-fixture"))]\n#[path = "coordinator/tests/performance_startup.rs"]\nmod startup_performance_tests;\n'
existing = coordinator.read_text()
assert "mod startup_performance_tests" not in existing, "baseline already adapted"
coordinator.write_text(existing + hook)
