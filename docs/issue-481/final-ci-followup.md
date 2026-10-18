# Final CI follow-up

[FULL 39115318759](https://github.com/Caismis/rustX/actions/runs/39115318759)
on `0c3f692abeaab590215cc953b66b28d0cf64f99e` passed Linux FULL, both browser
shards and all other selected jobs except macOS FULL. Its unit/in-crate stage
finished with 3,318 passed, one failed and three ignored; the later macOS external
boundary stage did not run. Merge verification correctly failed.

The failure was cleanup in
`t18_stopping_real_unix_source_is_unavailable_without_a_false_execution_outcome`:
`InteractionNotPending` at `tests/boundary/subagent/conformance/observation.rs:960`.
The fixture released the Root registry's seal gate before sending the Leaf's
cleanup cancellation. Middle could then finish `report_and_drain`, shut down and
retire the nested Leaf interaction before that command.

The fix sends the direct-owner Leaf cancellation while the existing physical seal
gate remains held, records its result, then releases the gate before asserting
both the unchanged source-isolation checks and strict cancellation success.
Errors are not swallowed. There are no sleeps, retries, timeout changes or relaxed
assertions. The non-stopping route scenario and production code are unchanged.

The previous failure remains evidence; a new exact-head FULL and Merge result is
required and recorded in the PR. Production source, Cargo inputs and the five
measured benchmark fixtures remain identical to measured `7ed053952b34705419e8b4ffffcc24ae3e423b0d`.
