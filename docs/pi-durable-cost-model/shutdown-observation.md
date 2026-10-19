# Inspecting a dormant host after shutdown

Review of b42faa94 found that a conversation which completed without an observer could retain its bootstrap transcript, occupancy and trace in the first post-shutdown checkpoint. Demand hydration correctly refused to restart the cancelled reader, but checkpoint construction then reused the stale derived head. A two-turn fixture with distinct nonzero usage reproduced the stale transcript at that exact head. The failed run and source diff are retained in the evidence archive.

Explicit inspection now reads the bounded derived heads into a request-owned checkpoint seed at its captured shutdown cut, using the existing inspection Store. It does not install a new projection, advance the cursor, restart the projection reader or change reader cancellation/join. Subsequent inspection still validates the original cut and deadline and propagates Store errors. Generic transcript and trace reads remain available at that same cut.

The regression fixture also exposed a separate ordering gap: closing the projection queue immediately after native shutdown could discard its queued Shutdown observation. Shutdown now uses the existing synchronous CPU-only native publication path. A parked-worker test proves publication completes before native shutdown returns, independently of worker scheduling.

Three deterministic regressions cover publication with a parked worker, two dormant turns followed by shutdown and repeated reattachment, and dormant-to-subscriber hydration without an earlier checkpoint. The shutdown test verifies exact transcript, latest occupancy, projection and trace heads, unchanged cursor/cut, generic pagination, and exclusion of an authoritative append made after shutdown. The direct subscription test gates physical read entry and completion.

This is a correctness repair for the observer-demand stage. It makes no performance claim. Validation passed **492 distinct tests**: 112 host/scripted-host, 84 projection, and 296 App Server manager/protocol/real-transport tests. The three regressions are included. The matching real child was rebuilt; root opt-level 0 was used for correctness only. Formatting and whitespace checks pass. Final exact-head FULL/Merge and independent review remain required.

[Verified evidence archive](evidence/shutdown-observation-validation.tar.xz) and [manifest](evidence/shutdown-observation-validation.tar.manifest.json) retain the red reproductions, initial fixture diagnostics, exact validated source diff, build commands and identities, and all 492 test names. Originals are preserved.
