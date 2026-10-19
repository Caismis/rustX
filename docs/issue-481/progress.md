# Issue 481 implementation checkpoints

## Base and environment

Work began in the user's saved cloud environment on 2026-10-18. The isolated
branch is `codex/issue-481-source-canonical`; its verified base is
`3c355f152d405a201482d53b6e45497df4f44a7f`, the merged #480 commit.
`git fetch origin` and `git merge-base --is-ancestor` succeeded. The original
`/workspace/rustX` checkout on `work` was clean and remains untouched.

Neither desktop commit `a867a91cc06ca5b172dc2ed7b9e57f6c7ff96642` nor
`4c9a0ab9c3a6acd843e8cd366daf9e33cf568608` was present in this checkout's
object database. `git ls-remote` found no named fair-provider branch. The
inspected Actions artifact listing did not identify that harness. This is a
reconstruction, not a copy of the offline desktop work. Desktop numbers and
the old in-process Pi faux-provider ratios are not cloud baselines.

The parent confirmed that the create API admitted `gpt-6-astra medium, Standard`.
The session's tools do not independently expose these runtime settings.
No permissions or security settings were changed.

## Build inputs

Rust 1.98.1 was installed through rustup into the existing workspace toolchain
root. Node is v24.19.0. The first release build failed because libclang's default
include search did not locate `stdarg.h`. The installed GCC 14 headers are
used explicitly through `BINDGEN_EXTRA_CLANG_ARGS`; this is a build-environment
setting, not a repository/compiler-version change.

```sh
PATH=/workspace/.cargo/bin:$PATH \
CARGO_HOME=/workspace/.cargo RUSTUP_HOME=/workspace/.rustup \
CARGO_TARGET_DIR=/workspace/issue-481-target \
BINDGEN_EXTRA_CLANG_ARGS='-isystem /usr/lib/gcc/x86_64-linux-gnu/14/include' \
cargo test --release --locked --test conformance --no-run --message-format=json
```

Pi is `earendil-works/pi` at
`90d863f7c1183b965f099855715d6e88c7b8a52c`, using its Durable Harness and
Node SQLite backend. `npm ci --ignore-scripts` installs the pinned lockfile.
The ordinary offline AI package build requires absent generated model catalogs.
Do not run model hydration: it contacts real provider APIs. Instead compile
Chord and telemetry using their package build scripts, copy
`benchmarks/runtime/pi-tsconfig.json` to
`packages/ai/tsconfig.benchmark.json`, and run the pinned checkout's
`node_modules/.bin/tsc -p packages/ai/tsconfig.benchmark.json`.
That compiles the core and production OpenAI Completions adapter dependency
graph without catalog generation or source modifications. `--conditions=source`
is retained for packages that expose their source condition.

## Harness status

`controller.py` owns product and provider as sibling processes. The product
announces its fixture workspace before the controller starts the shared
Python HTTP/SSE provider. The provider stays alive through product
checkpoint/close/reopen/cleanup and process exit. It is never reaped by the
product. Provider startup/control/teardown are explicit overhead, with
startup and teardown reported separately.

Both engines use `runtime_benchmark.py` responses, gates and chunk scripts,
`chat-model`, streaming usage and 1,024 max completion tokens. Pi uses
`openAICompletionsApi()`; no faux stream substitutes for its production
adapter. Both receive the same ordered user/assistant history and initial
UTC time context. Only valid generated timestamp and exact fixture workspace
identities are normalized for expected-workload comparison; wire/captured
context comparisons preserve them exactly.

Pi's four native coding-tool schemas and rustX's seven schemas remain
explicit and independently checked. `compare-http.mjs` checks the full core
request while retaining separate schema identities; this is not full request
equality or a claim of product contract equivalence.

CPU and process-write snapshots cover explicitly named runtime and verification
phases. Evidence JSON is written after the final cleanup counter endpoint;
controller process wall includes that evidence output and is separately named.
Driver work and verification allocations remain in the measured process.
No claim of pure engine cost, true peak RSS/PSS, allocation count, or physical
storage-device writes is made. Formal measurements must not overlap builds.

All six Pi smoke scenarios passed the reconstructed HTTP path, including
live/captured/reopened context equality and exact results. Eight independent
oracle tests passed, including negative cases. These smoke runs overlapped
Rust compilation and are not performance evidence. Failed attempts are
retained under `/workspace/issue-481-evidence` alongside their logs.

The fresh baseline, Source-only and canonical incremental/combined measurements
are complete; see `baseline.md`, `source.md`, `canonical.md` and `combined.md`.
All 20 focused append tests and 367 durable tests pass. The optimized combined
build, five Rust, eight Node and nine legacy v2 Python tests, formatting,
and all-target/all-feature Clippy pass. Draft PR #484 retains explicit RSS and
runtime-wall regressions. Exact-head FULL/independent review remain required;
the authoritative current status and head/base/main identities are in the PR.
No merge or auto-merge is authorized.

Review follow-up: 14 new current-controller/summary tests plus nine historical
v2 tests pass; 59 CI-script tests pass. CI runs both Python suites. All 252 runtime
samples / 5,922 requests revalidate with stricter lifecycle checks, with unchanged
headline medians. See `review.md` and `evidence/pair-inputs` for reviewable inputs
and controlled diagnostic findings. Original wall and memory limitations remain.
