# Issue #454 unified `present` delivery: architecture record and validation

Validated on Linux x86_64 on 2026-10-08 in the independent worktree
`../rustX-issue-454`, on branch `issue-454-unified-present`. It is based on fetched
`origin/main` `26def795775884d07e013dc51939028a52cb56b2`. The primary checkout was
not modified.

## Ownership after this change

| Concern | Owner | Evidence |
| --- | --- | --- |
| Execution, cancellation, terminal settlement and commit | Native `present` Tool, ordinary Tool Plane, `AgentLoopExecution::commit_tool_result_batch` (unchanged) | `present.rs` order/duplicate, malformed-tail and post-validation cancellation tests. The boundary scenarios show exactly one committed Tool message. |
| Delivery truth | `deliveries` on a successfully committed canonical Tool-result message | Web `presentedDeliveries`, TUI `pageDeliveries`/`resultCommitted`. Failed results, cancelled results, tool JSON, prose and foreground settlements yield nothing (unit tests in both clients). |
| Delivery bytes and native location | `app_server::delivery_access`, one native core with every fence | Product Host scenario (unchanged behavior) and new delivery-access scenario share it. |
| Who may enter | Transport authentication only: the Product Host secret lane, stdio-owner delegation (`--stdio-delivery-access`), or the separate WebSocket credential (`--delivery-access-token-file`) | Process, transport and scripted tests below. |
| Presentation | Web: Harness-derived `PresentRow`/`PresentedFileCard` via `bindings/present.ts`. TUI: pure `tool-present` renderer and `/files` selector | Web/TUI unit tests and e2e. |
| Client-local effects | TUI `app-server/delivery-files.ts` only | Renderer purity contract test; selector dispatches intents only. |

No delivery database, registry, DSH Session event, Agent Loop branch, provider
behavior, generic filesystem RPC or Artifact Store change was added.

## Authorization threat boundary

- An ordinary authenticated connection can supply exact coordinates and a
  trusted-looking client name (`rustx-tui`, `rustx-product-host`), but `delivery/read`
  and `delivery/locate` still fail `session_file_read/unauthorized` before lookup.
  The Product Host secret offered on the ordinary lane, the transport token reused
  as a delivery credential, an empty or wrong credential, or a delivery credential
  without the transport token: each is refused at the handshake.
- A granted connection reaches only its own attachments (`stale_attachment` for
  another connection's target). Invented coordinates and non-delivering indexes are
  `unavailable`; index ≥ 8 is `invalid_params`.
- Native credential removal revokes live grants. Connection close cancels the
  connection's authority. With a gate held after leaf open and before bytes, close
  publishes no bytes, the permit stays owned until the blocking read physically
  settles, and then both permits return.
- Fork reads resolve the original Conversation and root. A same-named file in an
  unrelated Session root is `unavailable`. Mutable replacement, deletion
  (`missing`), oversize (`too_large`) and capacity (`capacity`) are explicit.
- Locations are verified leaves. The TUI opens locally only for its own spawned
  child, after its own `lstat` device/inode matches. A remote TUI never requests a
  location. Startup rejects reused secrets, group/world-readable credential files and
  cross-transport flags.
- The Product Host credential and lane are unchanged and never given to the TUI or
  browser. The browser never offers the delivery credential.

## Deterministic regressions added

Rust:
- `transport_granted_delivery_access_reads_and_locates_only_through_own_attachment`
  (boundary suite): capability reporting, forged names, ordinary rejection,
  own-attachment routing, bytes/location equality, fork original identity,
  unrelated roots, mutable/missing/oversize, capacity, gated close-before-bytes
  revocation and physical permit settlement, zero model requests.
- `websocket_delivery_access_is_a_separate_additive_revocable_credential`: real
  WebSocket handshakes and live revocation.
- `app_server_delivery_access_is_explicit_transport_composition`: real binary flag
  validation, stdio delegation and WebSocket credential.
- `location_is_the_verified_leaf_identity_without_a_size_bound`: descriptor walk,
  replacement, symlink, unrelated root and post-open revocation fence.

TUI:
- `deliveries.test.ts`: committed-only records, renderer/forgery cases, pre-commit
  card suppression, bounded decoding, no-clobber byte-exact save and cancellation,
  shared-host/leaf checks for Open, selector intents, stale-outcome fencing and
  explicit paging.
- `delivery-integration.test.ts` (real child and real socket, provider emulator):
  owned stdio save with Unicode/spaces/CRLF bytes, refusal to overwrite, mutable
  reopen, oversize and missing failures with no output, cancellation before write,
  verified local open. Remote without the credential shows metadata only and
  refuses bytes; a wrong credential is refused; with the credential, Save works
  client-side after reconnect and Open is never attempted. Both prove zero
  additional provider requests.
- `app.test.ts`: `/files` through real input routing, with no read without access
  and an exact-record read with access.

Web:
- `present.test.tsx`: lifecycle-only phases, call row never renders cards,
  keyboard disclosure, four-card summary, canonical order, separate Preview and
  Download intents, en/zh.
- `file-delivery.spec.ts` (e2e): updated for the Harness card DOM and the
  collapsed summary. It checks call-row phases, and that the browser's ordinary
  connection gets `delivery/read` refused as unauthorized.

## Review repairs

The independent review of `d35b929f` found four P1 findings and one P2 finding.
The first two had one cause: an ordinary-lane delivery request had no owner
between admission and transmission.

| Finding | Correction | Regression |
| --- | --- | --- |
| P1: Escape in `/files` abandoned only the local Promise; the native read kept running | Each `delivery/read`/`delivery/locate` is a `delivery_access::Operation` registered under its exact id before admission. New `delivery/cancel` cancels only that connection's own id and fails the request's native fences. The TUI client sends it when the action's signal aborts and keeps the correlation until the one terminal response. | `delivery_cancellation_is_request_scoped_at_every_native_interleaving` (before admission, after admission before open, after open before bytes, after settlement before publication, after publication, concurrent sibling, other/ordinary connections, reuse); TUI client correlation tests; real stdio cancel in `delivery-integration.test.ts` |
| P1: a produced response could be written after revocation | The response is queued with its `Publication`; `transport::Outgoing` commits it immediately before the physical write, rechecking cancellation, delivery authority and the attachment, and substitutes the same id's typed failure | `delivery_publication_commits_at_the_transport_writer` (real stdio writer parked after native settlement, before commit: credential, detach, close, locate; complementary unrevoked case; unrelated `server/info`; permits restored); `delivery_revocation_before_publication_commit_suppresses_sensitive_responses` |
| P1: Save prefilled the editable Input with the raw delivered name | `DestinationInput` holds only renderable text (refuses unrenderable `setValue` and whole pastes); the name prefills only when it renders as itself; outcome text is sanitized; `deliveryDestination` has no name fallback | `deliveries.test.ts`: ESC/CSI, OSC, C1, LF, CR, bidi override/isolate, ALM through the real Input render; Unicode/space names prefilled exactly and saved byte-exactly |
| P1: the location test assumed unlink + recreate yields a new inode | The original stays allocated (renamed aside) while the replacement is created; adds a swap after the owned open, which fails `Replaced` | `location_is_the_verified_leaf_identity_without_a_size_bound`, run 30 times consecutively |
| P2: retiring `/files` left the operation running | One abort scope per `/files` overlay, retired by `#closeOverlay`, which every ending passes through. Save's commit is now its atomic `link` (see the Save publication repair below). Open commits at the spawn. | `app.test.ts` retirement test (Escape, overlay replacement, snapshot replacement, disconnect, quit; the native read is cancelled and no file appears even when the bytes arrive late); `deliveries.test.ts` Save/Open commit-point orderings |

Negative controls: with the publication decision forced to `Ok`, the three Rust
publication/cancellation tests fail. With cancel not cancelling the request token,
the cancellation test fails ("never admitted"). With the interaction abort removed,
the app retirement test fails on overlay replacement. With the raw `Input` and raw
prefill, the hostile-name test fails on ESC/CSI.

The protocol stays v38: v38 is introduced by this unmerged PR, and main is v37.

## Save publication repair

The review of `956b1f53` found that Save still wrote directly to the destination
and cleaned up with `lstat` + device/inode compare + `unlink`. Another process could
replace the destination between the check and the unlink, and Save could report
success for a destination that no longer named its bytes. Destinations were also
trimmed, and every `lstat` error counted as absence.

(Superseded: the staging directory described here was replaced by a single
staged file in the filesystem ownership repair below.)

Save is now one staging and publication lifecycle in `delivery-files.ts`. It
allocates a private 0700 `mkdtemp` directory beside the destination, writes, syncs
and closes `file` there, and checks cancellation at publication admission. The
commit is `link(staged, destination)`, which atomically creates a new name and fails
with `EEXIST` for any existing entry. Cleanup unlinks only the staged name and
removes only that directory, and it never touches the destination. There is no
compatibility path or fallback, and the App Server protocol is unchanged.

| Scenario | Synchronization | Result |
| --- | --- | --- |
| Complete publication; no visible partial file; staging 0700, removed after success | second chunk write parked on a deferred | destination absent mid-write; exact bytes; `nlink` 1; directory holds only the file |
| Names `report.md`, ` report.md`, `report.md `, `  report final.md  `, `报告 final.md`, `naïve résumé — v2.txt`; `~/`, absolute, `..` kept | none (pure, then real files) | exact path strings; six distinct files, byte-exact |
| Existing file; destination created while staging; two complete saves admitted together; symlink; dangling symlink; directory | parked write; both `link` calls gated until both are admitted | `already exists`; existing entries unchanged; exactly one contender publishes; nothing created through a dangling link |
| `link` unsupported (`EPERM`, `ENOTSUP`, `EOPNOTSUPP`) | injected `link` failure | explicit refusal, destination absent, no staging |
| Cancel before staging, during the first write, between chunks, after sync before admission; destination created by another writer meanwhile | deferred read; write/sync hooks abort at the exact step | rejects with the abort reason; `link` never called; the other writer's file intact |
| Cancel after `link` was dispatched; cancel after success | `link` parked after the real link | resolves as saved; file kept |
| Write, short write, sync, close, `link`, `mkdtemp` failures | injected errors | destination absent, no staging |
| `link` reports an error but the destination names the staged inode; destination uninspectable (`EACCES`) | injected errors | saved; `DeliveryUncertainError` (refined by the certainty repair below) |
| Staging cleanup fails after publication; staged name not removable after cancellation; a foreign entry inside staging | injected `rmdir`/`unlink` errors; parked write | saved with residue warning; `DeliveryResidueError` with the original cancellation as cause; foreign entry kept, directory reported |
| Open path uninspectable | `chmod 000` directory | `cannot be inspected (EACCES)`, not "absent" |

Negative controls, each applied to `delivery-files.ts` alone and then restored:

| Control | Tests that failed |
| --- | --- |
| Direct write to the destination with check-then-unlink cleanup | 5, including no visible partial file, no-clobber and cancellation |
| `rename` as the commit | no-clobber; unsupported filesystem |
| A failed save unlinks the destination | no-clobber; cancellation (the other writer's file was deleted) |
| No admission check | cancellation after sync; residue |
| A cancel after dispatch rolls back | dispatched publication |
| `trim()` on the destination | spelling; exact-name save |
| Lexical `path.resolve` | spelling |
| Cleanup errors swallowed | residue |
| A link error trusted without inspecting the destination | ambiguous link failure |

## Publication certainty and staging-path repair

The review of `78d01f5d` found two defects in the Save publication step.

**P1.** After a failed `link`, an absent destination or one naming another file was
reported as a definite refusal. That observation describes the destination now, not
what the link did: the link may have created the entry, and someone may then have
removed or replaced it. `publish()` now decides by evidence. The destination naming
the staged inode means published. A definite-rejection code (`EEXIST`, path,
permission, read-only, space, quota, `EINVAL`, unsupported) means refused. Anything
else (`EIO`, no code) is uncertain. `DeliveryUncertainError` carries the link error
and a typed observation: `absent`, `foreign`, or `uninspectable` with its error.

**P2.** The `mkdtemp` prefix and the staged path were built with `path.join`, which
folds `..` lexically. For `link/../x` with `link` a symlink, that staged in a
different directory than the one the OS creates the destination in. Every Save path
is now built by concatenation (`childPath`), from the destination's own spelling of
its parent.

| Scenario | Synchronization | Result |
| --- | --- | --- |
| Real link committed, acknowledgement replaced by `EIO` or `EEXIST`, destination still names the staged inode | `link` seam: real link, then injected error | saved |
| Real link committed, destination removed, then `EIO` | `rmSync` inside the seam between commit and acknowledgement | `DeliveryUncertainError`, observed `absent`, cause `EIO` |
| Real link committed, destination replaced by another file, then `EIO` | `rename` over it inside the seam | uncertain, observed `foreign`; the replacement is unchanged |
| `EIO` and an uninspectable destination (`EACCES`) | injected `link` and `lstat` | uncertain; cause and inspection error both kept |
| Error without a code | injected | uncertain |
| `EEXIST` with a foreign destination; `EPERM`/`ENOTSUP`/`EOPNOTSUPP`; real pre-existing entries | injected and real | refused; destination unchanged; staging removed |
| `workspace/link -> ../other/nested/`, destination `workspace/link/../报告 final.md` (lexically `workspace`, really `other`) | parked second write; `link` recorder | staging and staged file in `other`, none in `workspace`; `link` receives the spelled strings; bytes at `other/报告 final.md`; cancelled and failed saves leave nothing anywhere; a residue path keeps its components, and `realpath(3)` resolves it into `other` |
| Relative and absolute paths with spaces and Unicode | real files | exact paths and bytes |

Negative controls, each applied alone to `delivery-files.ts` and then restored:

| Control | Test that failed |
| --- | --- |
| Absent or foreign destination taken as refusal (the previous code) | evidence test |
| Every coded error taken as refusal | evidence test |
| No positive identity evidence | evidence test |
| `path.join` for the `mkdtemp` prefix | symlink `..` test |
| `path.join` for the staged path | symlink `..` test |

## Cancellation capacity, staging ownership, Open contract and retirement repair

The review of `c4725dc4` found four defects.

**P1, cancellation under saturation.** `delivery/cancel` shared the two-slot
control lane, and a rejected cancel was discarded, so an aborted read could keep
running on the server. Simply adding capacity would have let a seventeenth request
end the connection. Invariant: while the connection is healthy, a cancellation of
an admitted delivery request reaches the server. The client lanes are now
`wait 4, admission 2, control 2, rpc 7, cancel 1`, which sums to the server's 16.
Only `delivery/cancel` may use `cancel`, and only through the client's own abort
path; `call()` cannot name it. Cancellations wait for that slot in abort order and
are dropped when their request settles first. The linearization point is unchanged:
the server's `Operations::cancel` against the writer's publication commit. A
refused cancellation ends the connection.

**P1, staging ownership** (superseded by the filesystem ownership repair below,
which removed the descriptor-pinned directory and its staging-specific tests).
After `mkdtemp`, each step re-resolved the staging
pathname, so a writer of the parent could rename or replace it between steps and
redirect the write, the link or cleanup. Invariant: a Save publishes only the bytes
it staged and removes only objects it can still show it owns. The parent (`O_PATH`)
and staging directory are held as descriptors and every later step goes through
`/proc/self/fd/<fd>`. The staging directory is checked to be this user's directory
before use. The empty directory is removed by name only while that name still
refers to the held directory. Systems without descriptor paths refuse Save before
creating anything. The publication commit (the `link` dispatch) and the evidence
classification are unchanged.

**P1, Open contract.** Open is now defined as best effort in the trusted owned-child
environment. The device/inode check is an availability check. The opener resolves
the pathname itself, and the result claims only acceptance or rejection by the
opener.

**P2, retirement.** Outcomes reported after `/files` retired were chosen by error
class, so an opener failure after launch was silently dropped. Each action now
records `LocalEffect.committed` in the same synchronous step as its commit (the
link dispatch or the opener spawn) and `residue` when staging remains. After
retirement, exactly those outcomes are reported, once, on the transient surface.

| Scenario | Synchronization | Result |
| --- | --- | --- |
| Control lane full (2 `job/cancel`), then a read aborted | data barriers on the fake transport log | the cancel is sent through its own slot |
| 15 requests in flight (all ordinary lanes full), three reads aborted together | abort order; responses injected one by one | one cancel in flight, never a 17th request; next cancel sent on the previous answer; one whose read published first is never sent; every slot recovered |
| Server refuses an owed cancel | injected error response | the connection ends with that cause; the read settles once |
| Real stdio `serve`: a read parked inside its descriptor read plus 14 ordinary requests parked before their operation; cancel as the 16th | `before_bytes` gate, `before_operation` gate | cancel answered `accepted: true` at once; read answers `delivery_cancelled` only after its physical settlement; permits restored; all 14 answered once; connection healthy |
| Staging renamed mid-write; replaced by a symlink to a foreign directory holding `file` | write hook | this save's bytes published; the foreign file is never written, linked or unlinked (same inode); the moved directory is emptied and reported, never chased |
| Staging replaced by a planted directory with `file`, between close and link; and after link, before cleanup | close hook; `link` seam | this save's bytes, not the planted ones; the planted file and directory are untouched; residue reported |
| Not published (`EEXIST`) and staging moved | write hook | `DeliveryResidueError`, destination untouched |
| Staging name replaced before it was opened; a staging directory that is not this user's | `mkdtemp` seam; `open` seam reporting another uid | refused before writing; nothing removed |
| Parent renamed between staging and link; a new directory takes its name | close hook | `ENOENT` refusal and no staging left in the moved parent; or published at the typed path, staging removed from the parent where it was made |
| No `/proc/self/fd` | `stat` seam | refused before `mkdtemp` |
| Open: verified file; replaced, missing, symlink, directory; replaced after verification; opener exit 4; opener missing | real files; `launch` seam | launch only for the verified file; result claims only acceptance; rejection and spawn failure reported as the opener's, with the launch committed |
| `/files` retired by snapshot or overlay replacement, before launch, or after launch with exit 0 or 3; Escape after launch | gated `locate` and opener; spies on transient feedback and `DeliverySelector.settle` | before launch: no launch and no report; after launch: exactly one transient report (accepted or rejected), never into the retired selector; Escape keeps the surface, which shows the acceptance, not "Cancelled" |

Negative controls, each applied alone and restored byte-identical from a backup:

| Control | Test that failed |
| --- | --- |
| `delivery/cancel` in the control lane (the previous code) | reserved-slot client test |
| No wait for the cancel slot (all cancels at once) | reserved-slot client test (the second cancel was rejected locally and lost) |
| Refused cancel swallowed | refused-cancel client test |
| Server admits one request fewer than the shared budget | `delivery_cancellation_is_admitted_as_the_sixteenth_in_flight_request` |
| Staged file written and linked by name | staging-ownership test |
| Staging directory removed by name without the identity check | staging-ownership test |
| No ownership check on the opened staging directory | staging-ownership test |
| No descriptor-path probe | unsupported-system test |
| Open launch commit not recorded | Open contract test; `/files` retirement app test |
| Post-retirement reporting by error class (the previous code) | `/files` retirement app test |

## Filesystem ownership and cross-platform Save repair

The review of `a1607e7a` found that the descriptor-pinned staging above still left
three pathname races, and that it had disabled Save on macOS:

- **A, creation identity.** The reopened staging directory was accepted because it
  was a directory owned by this UID. A same-UID substitute passes that check.
- **B, published bytes.** The link read the entry name `file`. After the staged
  handle was closed, a substitute at that name would have been published and
  reported as this save's bytes.
- **C, cleanup.** The `lstat` → compare → `rmdir` sequence could remove a
  substituted empty directory.

The decision was to converge on one Save contract, not to add more checks.

**Threat model.**

1. *A same-UID adversary* is not resisted, and no pathname API could resist one.
   Such a process can rewrite the destination, the staged file or this process.
2. *A different UID with write permission on the parent* is equivalent to the
   owner for entries in that directory, so it is inside the trust boundary. In a
   sticky directory the kernel stops other UIDs from renaming or removing this
   user's entries, so there they are excluded.
3. *Assumptions:* the destination's parent exists and its filesystem supports
   hard links. Every process that may modify that parent is trusted not to
   interfere while the save runs.
4. *An open descriptor* protects the identity of the file it names and the
   writes made through it. It protects no name.
5. *Atomic on Linux and macOS:* `O_CREAT|O_EXCL` creation (the ownership
   evidence) and `link(2)` creating the destination entry, never replacing one.
6. *Pathname-based, so judged only by evidence:* which file the staged name holds
   when `link` reads it, and what `unlink` removes. Neither platform offers a
   portable link-by-descriptor (`linkat(AT_EMPTY_PATH)` and `O_TMPFILE` are
   Linux-only, and Node exposes neither) or a conditional unlink.
7. *A successful publication establishes* that this save's link created the
   destination entry, and that `lstat` right after showed it naming the file
   this save created exclusively and wrote and synced through its own handle.
   It does not establish that the entry keeps that name, or that the directory
   entry is durable across a crash (the parent is not synced).

**Design.** This is git's loose-object publication pattern, kept to its minimum.

```text
open(<parent as spelled>/.rustx-save-<128-bit hex>, O_WRONLY|O_CREAT|O_EXCL)   held: F
  -> writes through F -> fsync F
  -> abort?                          admission
  -> link(staged name, destination)  commit: atomic, never replaces an entry
  -> lstat(destination) is F?        published | refused | uncertain
  -> unlink(staged name), once; F's link count decides residue; close F
```

The private directory, `O_PATH`, `/proc/self/fd`, the uid check and `rmdir` are
gone, and so is the Linux-only gate in `/files`. There is:

- **one owner**, the handle the exclusive create returned;
- **one commit**, the `link` dispatch;
- **one cancellation boundary**, at admission;
- **one cleanup**, one `unlink`, judged by F's link count.

Against A, ownership comes from the exclusive create's handle, so nothing is
re-resolved. Against B, a successful link is not publication: only a destination
naming F's device and inode after the commit is. Against C, no directory is ever
removed, and residue is judged by F's link count, not by the name. What remains
is stated, not hidden: within the excluded case, `link` may publish a substitute
(reported as uncertain) and `unlink` may remove one (F reported as residue).
Removed obsolete parts: the `stat`, `mkdtemp` and `rmdir` seams, and the
`/proc` refusal.

Each case below uses real files with interposition at the named boundary:

| # | Scenario | Synchronization boundary | Proven effect |
| --- | --- | --- | --- |
| 1 | A file, a symlink to a foreign file, or a directory planted at the staged name before it is created | `open` seam, immediately before the exclusive create | `EEXIST`; the planted entry has the same device/inode and is unchanged; the symlink target is never written; destination absent; `LocalEffect` untouched |
| 2 | Staged name replaced by another same-user file (different inode) | write seam, before chunk 2 | `DeliveryUncertainError` `linked: true`, `foreign`; destination is their inode with their bytes; this save's moved file has every byte, through its handle; residue reported |
| 3 | Staged file renamed during writing | write seam, before chunk 1 | `ENOENT` refusal, `DeliveryResidueError` ("linked elsewhere"); the moved file is this save's inode with every byte; destination absent |
| 4 | Staged name replaced by a symlink to a foreign file | write seam, before chunk 1 | uncertain, `foreign` (Linux links the symlink, macOS its target); the target is never written and keeps its inode; this save's file reported |
| 5 | Staged name replaced after the data is synced, before the link | sync seam, after the real sync | as 2: never "published" |
| 6 | Staged file renamed and its name replaced before cleanup | `link` seam, after the real link | saved, destination is this save's inode; residue reported because the file is still linked elsewhere; the substitute at the staged name was unlinked, which is the documented limit for an excluded actor |
| 7 | Staged file renamed after the link, before cleanup | `link` seam | saved with residue; the moved name and the destination are one inode |
| 8 | A foreign empty directory substituted immediately before removal | `unlink` seam, before the real unlink | `unlink` fails (`EISDIR` on Linux, `EPERM` on macOS); the directory survives, empty; residue reported |
| 9 | Destination parent renamed between staging and link; or a new directory takes its name | sync seam | `ENOENT` refusal and residue; the staged file holds every byte in the renamed parent; nothing in the new directory; destination absent |
| 10 | Destination created by another writer while staging; two saves admitted to `link` together | parked write; both `link` calls gated | `already exists`, their file keeps its inode; exactly one contender publishes |
| 11 | Cancel before staging, during and between writes, after sync | deferred read; write and sync seams | abort reason; no `link`; nothing left |
| 12 | Cancel after the link was dispatched | `link` seam, after the real link | saved; `committed` set at dispatch |
| 13 | Real link with an `EIO`/`EEXIST` acknowledgement; removed or replaced destination; successful link then destination replaced (`linked: true`); uninspectable destination | `link` and `lstat` seams | saved only when the destination is the staged inode; otherwise uncertain, with what was observed |
| 14 | Unlink fails after publication, after cancellation, and with an uncertain outcome | `unlink` seam | saved with residue; `DeliveryResidueError` keeping the cancellation; uncertain with residue; the retained file is the one written |
| 15 | Normal Save: Unicode and space names, `..` after a symlink | parked write | the destination's device/inode is the staged file's; staged name `.rustx-save-<32 hex>`; `link` receives both spelled paths; no residue |
| — | `/files` Save from a local child and from a remote host | `DeliverySelector.settle` spy | identical byte-exact save for both ownerships; Save never locates |

**macOS.** The `Desktop adapter and Host (macOS Node)` job now builds the `rustx`
binary next to the supervisor; the supervisor already compiles the library. It
syncs the provider emulator and runs `node --test test/deliveries.test.ts
test/delivery-integration.test.ts` with `RUSTX_REQUIRE_PROVIDER_EMULATOR=1`. That
runs every case above on APFS, plus byte-exact Save from a real stdio child and
a real WebSocket App Server.

**Cancellation-capacity regression.** `AsyncGate` (test-only) now counts the
callers currently parked. The scenario waits for the read's held permit, then for
exactly 14 parked operations, before it sends the cancel. After the cancel's
`accepted: true` it asserts:

- the permit is still held, and still 14 parked: nothing settled or was dropped;
- the read answers `delivery_cancelled` once, after its physical settlement;
- the 14 answer exactly once each after release, and the parked count returns
  to 0;
- a further request on the same connection is answered, and permits are at
  baseline.

Negative controls, each applied alone and restored byte-identical:

| Control | Tests that failed |
| --- | --- |
| A successful link taken as publication, without the post-commit check | 2/5, 4, 13 |
| Cleanup judged by the name (`unlink` success or `ENOENT` means removed) | 2/5, 3, 4, 6/7/8, 9 |
| Staged file created without `O_EXCL` | 1 |
| Identity taken from the staged name just before the link (stat → link) | 2/5, 4 |
| Save offered only for a local child | `/files` local and remote Save |
| Server admits one request fewer than 16 | `delivery_cancellation_is_admitted_as_the_sixteenth_in_flight_request` (the connection ends) |
| Test-side check: 13 requests sent while waiting for 14 parked | the same test, by its liveness bound: the barrier counts, a single arrival does not satisfy it |

## Validation

See the pull request for the final command list and results; the PR description
records the exact pass/fail counts of the final head.
