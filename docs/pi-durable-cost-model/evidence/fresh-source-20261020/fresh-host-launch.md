# Fresh-host launch plan (review required; no product build performed)

The two adjacent `fresh-host-*-plan.json` files are planned inputs, not verified
build identities. Run baseline first and ownership second only after independent
review. Create `/workspace/pr485-fresh-study/evidence` before entry; both plans
use the same fresh target for dependency reuse and separate evidence directories.
No cleanup is performed. A failed/partial build is retained and not retried.

Each build pins its own source checkout and src tree. Both use the recorded
Rust/Cargo 1.98.1 and LLVM/LLD 22.1.8, O3/debug0, empty root features, two jobs,
no incremental compilation and the ordinary library plus integration conformance.
The installed toolchain directory is named `stable`; direct validated binaries
avoid rustup installation. Libclang 19 is at `/lib/x86_64-linux-gnu`, not the old
`/usr/lib/llvm-19/lib`. No linker override is added; final ELF comment is checked.

Preserve each ordinary library, conformance, all three bin helpers and target
shared libraries before the next build. Their original target paths, individual
hashes, source paths and profiles are recorded. Shared target helpers are restored
from the selected revision's manifest outside `controller.run`, retaining each
compiled `CARGO_MANIFEST_DIR` and fake-provider source checkout. No old baseline
identity, geometry/scoped placeholder, transferred executable or old speed ratio
is used. Workload/controller/provider hashes are recorded for both new builds.

For a later reviewed cohort, explicitly set:

- `RUSTX_BENCH_WORKSPACE=/workspace/pr485-fresh-study`
- `RUSTX_BENCH_REPO=/workspace/pr485-baseline`
- `RUSTX_BENCH_BASELINE_BUILD=/workspace/pr485-fresh-study/evidence/build-baseline`
- `RUSTX_BENCH_CANDIDATE_BUILD=/workspace/pr485-fresh-study/evidence/build-ownership`

Retain canonical --before/--after/--output validation and the fixed 18-product
schedule. This document does not authorize timing or launch the cohort.

Resource limits remain: build entry 18 GiB, cohort entry 16 GiB, build growth
2 GiB, cohort growth 1 GiB, 0.5-second sampling, one-hour build deadline, half-hour
cohort deadline and 180-second product deadline. An additional build disk stop
at 18 GiB gives a 2 GiB margin above the protected cohort reserve; the original
6 GiB emergency stop remains for cohorts. Any new OOM event, anon+shmem >=14 GiB
for three seconds, or memory full avg10 >=10% for five seconds stops owned work.
Observation/diagnostic failures retain fail-closed cleanup. Do not enlarge the
2 GiB cold-build cap: if it stops the first build, report measured allocated
use/free space and retained output before considering any revised budget.
