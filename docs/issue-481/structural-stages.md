# Authorized structural stages after 5457e1c3

The user authorized all three directions on 2026-10-18, superseding the prior
scope freeze. This remains issue 481 / draft PR 484. No merge is authorized.
The prior final combination at `5457e1c34106b8d49de0edfb40b49ca8267d0383`
is now the structural baseline, using preserved optimized executable SHA-256
`3757014962aa24e831f8562cfd04d729f4435690d575cba17f9e3a0c2f9cfd55`.
Its exact Rust/test/Cargo trees match measured `99da5d68`. Fresh interleaved
baseline samples will accompany each independently preserved stage executable.

The baseline passed FULL and Merge verification in [run 39103028315](https://github.com/Caismis/rustX/actions/runs/39103028315). That result does not validate later stages.

## 1. Exact parsing metadata reuse

Expose a borrowed metadata visitor from Source's exact-byte index: original
span, capped recursive count, container/object flags, bounded decoded key,
ordered parent path and encoded path. Apply fragment span selection before
preview generation. The capture owner continues to serialize/authenticate the
same facts and original chunks. Compare every generated page byte and root
against the previous byte-reading algorithm in test-only differential fixtures,
including lexical numbers/u64, aliases, dense/large fields and rejected duplicate
keys or unrepresentable numeric values. No persistent format change in this stage. For capped counts above 32 and spans
at most 16 KiB, retain the bounded original validator: a truncated preview alone
does not prove serde depth/numeric validity. The common complete-preview case
reuses its already validated node count.

## 2. Append validation certificate

Introduce only a disposable, connection-local proof that the existing append
validator checked the relevant history. Cold/open/invalidation paths retain full
validation under the write transaction. A warm certificate must match mutation
epoch, SQLite data/schema versions, frontier and checkpoint generation. Unknown
relevant same-connection changes, external writers, schema changes and failed
transactions invalidate it. Validate newly introduced identities and retain the
Ledger/Pending duplicate authorities. Only successful COMMIT can publish an
advanced certificate; provisional batch state cannot escape rollback. Initially
checkpoint changes take the full validation path. Audit hook columns and watched
tables independently of the existing manifest certificates.

The explicitly accepted tradeoff is delayed discovery of arbitrary out-of-band
historical identity corruption: every append need not reread every historical
identity solely to discover it immediately. First-open/full validation, actual
read validation, explicit integrity checks, tracked mutation invalidation and
observed corruption/I/O errors remain. This is not permission to bless changed
Original witnesses or weaken atomicity, P<U<C, recovery or cold read bounds.

## 3. Bounded fact sorting and compact authenticated pages

Replace the small-fact SQL spool with a vector bounded by actual encoded bytes
and entry count; sort and reject duplicate keys, spilling larger inputs to the
bounded existing disk path. Replace page-level JSON/base64 overhead with a
versioned compact encoding, authenticated over exact encoded page bytes. Bound
both byte size and fanout count, retaining the 64 KiB maximum physical page.
Advance development format gates/readers/docs/tests together, reject unsupported
layouts, and provide no migration or legacy reader. Prove page/depth/read-piece,
preview and cold allocation bounds; no large Source parse/rebuild on a small read.

## Measurement and delivery gates

Each stage gets a commit, immutable optimized binary, at least three interleaved
paired comparisons and retained adverse/failed samples. Marginal append separates
cold and warm behavior at histories 0/64/512/2048 from full HTTP context costs.
The final combined comparison retains the same external local HTTP/SSE provider,
actual pinned Pi Durable SQLite and full ordered oracles. Record CPU/wall,
sampled anonymous memory, DB/WAL/temp and process-attributed writes. Diagnostic
profiling runs outside timings. No claim of whole-request constant time or Pi
parity. Each delivery awaits parent review; final exact-head FULL/Merge must pass.
