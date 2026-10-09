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

## Staged-file permissions and cleanup certainty repair

The review of `30b9c1a1` accepted the Save design and found two gaps.

**P1, staged permissions.** F was created `0666`, so the umask decided its
mode: `0644` under `0022`, `0666` under `0000`. Other local users could read, and
under a permissive umask even modify, the staged bytes in a traversable
destination directory. That widened the trust boundary beyond writers of the
parent. Invariant: no umask grants group or others access to staged bytes. F is
now created `0600` by the exclusive `open` itself. A umask only removes bits, and
there is no later `chmod` window. The published destination is the same inode,
so it is `0600` too; that is the documented default.

**P2, cleanup certainty.** Cleanup counted an uninspectable destination as one
of F's links. Consider `link` → `EIO` without creating anything, then `lstat` →
`EACCES`, then `unlink` → `EIO` without removing anything. F still had
`nlink = 1`, and cleanup was reported complete. Invariant: cleanup is reported
complete only on proof. Only a destination seen naming F accounts for a link.
Cleanup now has three results, carried as `staged: "remains" | "unknown"` on
`SavedDelivery.residue`, `DeliveryResidueError` and `DeliveryUncertainError`:

- **removed:** the count is at most the proven links.
- **unknown:** the count cannot be read, or its one extra link might be the
  uninspectable destination's.
- **remains:** any other link.

`LocalEffect.residue`, and therefore reporting after `/files` retires, covers
both `remains` and `unknown`. The publication outcome and its commit point are
unchanged.

| Publication evidence | Cleanup evidence | Synchronization | Result |
| --- | --- | --- | --- |
| Destination names F | unlink succeeds | real files | saved; staged name gone; destination `nlink` 1 |
| Destination names F | unlink `EIO`, nothing removed | `unlink` seam | saved, residue `remains`; staged and destination one inode, `nlink` 2 |
| Definite refusal (real `EEXIST`) | unlink succeeds | real files | refused; clean |
| Definite refusal | unlink `EIO` | `unlink` seam | `DeliveryResidueError` `remains`; staged file holds every byte, `nlink` 1; existing destination unchanged |
| `link` `EIO` (nothing created), destination absent / foreign | unlink `EIO` | `link` and `unlink` seams | uncertain, residue `remains` |
| `link` `EIO`, destination `lstat` `EACCES` | unlink `EIO`; real F `nlink` 1 | `link`, `lstat`, `unlink` seams | uncertain, residue **`unknown`**, message says removal could not be established; F on disk with every byte; destination absent; `LocalEffect.residue` set |
| `link` `EIO`, destination `EACCES` | unlink succeeds, `nlink` 0 | same seams | uncertain, no residue |
| `link` `EIO`, destination `EACCES` | link count unreadable | handle `stat` seam (second call) | uncertain, residue `unknown` |
| Destination names F | link count unreadable | handle `stat` seam | saved, residue `unknown` |
| Destination names F | staged name replaced | `link` seam (case 6 above) | only the handle's facts reported |

Permissions are checked by mode bits on real files. A test inside the runner
checks the requested creation mode and the mode while a write is parked before
publication, of the published file, while cancelling after sync, and of a
residue. Then `test/support/save-under-umask.ts` runs Save in its own child
process, one per umask `0000`, `0002` and `0022`, so the runner's process-wide
umask is never changed. In each child, a control file created `0666` proves the
umask was in force. Mid-write, synced, before-link, published, cancelled and
residue modes are all `0600`, and a cancelled or refused save leaves no
destination. The real stdio and WebSocket integration saves assert `0600`. The
macOS Node CI job runs both files on APFS. A check under another effective user
would need privileges, so it is not part of the suite.

Negative controls, each applied alone and restored byte-identical:

| Control | Tests that failed |
| --- | --- |
| Staged file created `0666` | in-runner privacy test; per-umask child test |
| Old accounting: an uninspectable destination counts as a proven link | cleanup-evidence test (the combined failure) |
| An unreadable link count taken as removal | cleanup-evidence test |

## macOS inherited-ACL trust boundary

The review of `4a9256f2` accepted the Save design and its `0600` creation, and
found that the documentation equated mode bits with effective access. Phrases
such as "private to you whatever the filesystem" and "processes that cannot
modify the parent cannot access the staged bytes" do not hold when the
destination directory carries an inheritable ACL.

Issue #454 requires authorized byte reads and a client-local Save of the
original bytes. It does not require confidentiality against principals that the
user-chosen destination directory authorizes. The contract is therefore
corrected, not the mechanism:

- **Mode bits.** Save requests `0600` at creation; no umask can set a group or
  other bit.
- **Effective access.** Access is the filesystem's whole authorization model.
  The chosen directory's policy, ACLs included, is trusted, and Save neither
  strips nor rewrites it.

The previous section's "no umask grants group or others access" now reads as
a statement about mode bits only.

**Platform semantics, from the sources:**

- **ACLs are checked before the mode bits.** In XNU (`bsd/vfs/vfs_subr.c`,
  `vnode_authorize_simple`), a deny entry returns `EACCES`, an allow entry for
  every requested right grants access, and only rights the ACL left undecided
  fall back to the mode bits. So an inherited allow entry can grant read or
  write on a `0600` file.
- **Inheritance happens at creation.** `vn_attribute_prepare` applies the
  parent's inheritable entries (`kauth_acl_inherit`) before the file exists,
  whenever the mount has extended security (APFS and HFS+ by default). An
  `open(O_CREAT|O_EXCL, 0600)` therefore inherits them, with no window.
- **Write access to content does not grant delete.** Delete is authorized by
  the file's own `delete` right or by the parent's `delete_child` right (or
  its POSIX write bit) under the sticky-bit rule. A `delete` entry the file
  inherits does let its holder remove the file's name, but `delete` cannot add
  a name. chmod(1) documents the same rule.
- **`link` adds no authorization.** Its `KAUTH_VNODE_LINKTARGET` check is
  reduced to an immutability check.
- **Linux** POSIX default ACLs are masked by the creation mode, so `0600`
  yields `mask::---` and no effective rights for named users or groups.
  Measured on tmpfs: a default ACL granting `user:nobody:rw-` gave the
  Save-mode file `user:nobody:rw-  #effective:---`, while a `0666` control
  file kept `rw-`.
- **No enforcement without overriding the user's policy.** Node exposes no
  call to create a file with an explicit ACL or without inheritance (macOS
  `openx_np` with a `filesec`; there is no `O_TMPFILE` there). Enforcing more
  than `0600` would mean rewriting the ACL after creation (a window, and
  overriding the user's sharing policy) or a native helper. The review rules
  out both, and the issue does not need them.

**Regression, run on the real macOS filesystem.** The test is "requests 0600
under an inherited macOS ACL, which Save neither strips nor rewrites", in
`test/deliveries.test.ts`. It runs in the existing macOS CI step "TUI
client-local Save" and is skipped on other platforms. Steps:

1. Use `/bin/chmod +a`, without privilege, to give a temporary directory two
   `file_inherit,only_inherit` entries: `user:nobody allow read,write` and
   `user:<runner> allow execute`.
2. Save through the real implementation, with its second write parked.
3. While the write is parked, check that the staged file has the requested
   `0600` mode and exactly the two entries, marked `inherited`.
4. After publication, check that the saved file is byte-exact, `0600`, the
   only directory entry, and carries exactly those entries. Check that the
   directory's own ACL is unchanged.
5. Check that `access(X_OK)` succeeds on the saved `0600` file: the kernel
   grants the inherited execute right, which the mode lacks. A `0600` control
   file outside the policy gets `EACCES`.
6. Remove the ACLs with `chmod -N` before deleting the files.

What the test does not prove:

- That `nobody` can actually read the file. Exercising another principal needs
  privilege, so that entry is shown inherited and its effect is the documented
  evaluation order.
- Behaviour on other volume types. The test covers the runner's APFS volume
  only.
- That the test catches a Save that strips or rewrites the ACL. Its negative
  controls could not run locally, because the test needs macOS.

Linux test behaviour is unchanged.

## Publication point, cleanup evidence and link classification repair

The review of `101f4a0d` found three gaps. This section supersedes earlier
statements here that the writer "commits immediately before the physical write",
that cleanup is judged by the handle's link count against the publication's
observation, and that `EEXIST` from `link` is a definite refusal.

**P1, publication before transmission.** `Outgoing::next()` committed a delivery
response as soon as it was dequeued; the adapter then awaited `write_all`/`send`.
Counterexample: the response is dequeued and committed, then the pipe is full and
the write stays pending; the credential is revoked; the pipe drains, and the success
(bytes or native path) is transmitted after a revocation that completed before any
of its bytes left. Invariant: a cancellation or revocation that completes before the
transport accepts a response's first bytes prevents that response.

Change: `Outgoing::next()` now yields the undecided `Outbound`, and each adapter
calls `Outbound::poll_hand_off(accept)`, where `accept` is its one non-suspending
acceptance step. `Publication::poll_hand_off` decides the record under the
operation's lock, offers it, and settles the request only if `accept` is ready, so
`Pending` leaves the request undecided. There is one state machine in
`delivery_access.rs` and one writer protocol in `transport/mod.rs`; the adapters
keep only their mechanics:

- **stdio:** the first `poll_write` to the non-blocking pipe that takes bytes;
  the rest of the record follows.
- **WebSocket:** after `poll_flush` shows every earlier frame on the socket,
  `start_send` hands the frame to tungstenite, then the same poll moves it out
  of the split sink's slot. tungstenite writes it at once; if the socket buffer
  is full right then, it holds the frame as its one buffered message. That is
  the documented WebSocket gap, since the library exposes no writability check
  before it takes a frame.

The test-only `before_publication` pause, which modelled the old boundary, is
gone. In-process callers keep `Publication::publish`, which decides and settles
at once.

**P2, stale link evidence in cleanup.** Cleanup counted one link for the
destination whenever publication had been observed. Counterexample: after the
publication check, someone moves the staged name to `M` and removes the
destination; the unlink gets `ENOENT`; F has one link, at `M`; cleanup said
`removed`. Invariant: cleanup concludes only from observations taken after its
unlink, each conclusion resting on one observation that is atomic on its own:

- **removed:** `fstat` count 0, or the destination's own `lstat` naming F with
  count 1.
- **remains:** count 2 or more, the staged name still naming F, or the
  destination naming F with count 2 or more.
- **unknown:** otherwise.

Publication evidence is no longer an input to cleanup.

**P3, `EEXIST` as definite refusal.** `link` → `EEXIST` was a refusal. On
NFSv3, NFSv4.0 or SMB, a retransmitted link that already succeeded returns
`EEXIST`. If the entry is then removed or replaced before the `lstat`, Save
reported "not saved" for a file it may have created. Invariant: Save claims
publication only on identity evidence, and refusal only on evidence that rules
publication out under the supported semantics; otherwise the outcome is
uncertain.

Change:

- An entry already at the destination is refused, as "already exists", before
  admission and before any `link` is dispatched, so that refusal is definite.
- `EEXIST` is removed from the definite-refusal codes, so `EEXIST` from the
  link itself is published if the destination names F, and uncertain otherwise.
- The remaining codes are rejections that `link(2)` makes without creating
  anything, and that a repeated request cannot produce unless someone also
  changed the staged name or the parent.
- Admission (the abort check) moved into `publish`, after the existence check,
  and stays synchronous with the `link` dispatch.

| Regression | Synchronization | What it proves |
| --- | --- | --- |
| `delivery_publication_linearizes_at_the_transports_first_accepted_byte` (stdio) | `Valve` writer: with no budget, `poll_write` takes nothing and records the offered bytes | **queued:** response produced (probe `completed`), never offered while the writer is stuck on record 29, cancel accepted, failure for 30. **cancel / credential / detach / close:** the success for 30 offered and refused, then revocation, then the valve opens: that id's typed failure. **published:** one byte granted; the next refused offer is exactly the rest of the success; then `delivery/cancel` returns `false` and the full success arrives. **shutdown:** success offered, transport shut down, no response for 30, connection closed. Every case: one response per id, no duplicates, unrelated `server/info` answered, all read permits back. |
| `websocket_delivery_publication_is_decided_when_tungstenite_takes_the_frame` | `Valve` under the WebSocket stream | Behind an unsent frame (29), response 30 is never handed to tungstenite and the credential removal wins. Once tungstenite took frame 30 (seen as the refused offer), the removal does not retract it: the documented WebSocket gap. |
| "reports cleanup only on fresh evidence taken after its unlink, never on the publication's" | `SaveFiles`/handle seams, real files | 16 rows, including the review's counterexample (moved staged name, removed destination → `unknown`, the moved file intact, nothing deleted). Also: destination replaced → `removed`; staged name moved with the destination intact → `remains` (count 2); staged name replaced by a foreign file → `removed` (destination's `lstat` count 1); unlink `EACCES` → `remains`; uninspectable destination; unreadable counts; combined uncertainty plus residue. |
| test 13, no-clobber test | real `link` with substituted acknowledgement | Retransmitted `EEXIST` after removal or replacement → uncertain; an existing entry → refused with no link dispatched; `EACCES` → refused; an error without a code → uncertain; exactly one `link` per save, never removing or rolling back the destination; two contending saves → the loser is uncertain (`EEXIST`, foreign). |

Negative controls, each applied alone from a byte-checked backup and restored:

| Control | Result |
| --- | --- |
| Decide once at dequeue (the old `next()` commit) | stdio test fails for `cancel` ("the cancel wins"); run per case, `credential`, `detach` and `close` each fail with the success escaping. `queued`, `published` and `shutdown` hold under both designs, as expected. |
| Cleanup trusts the publication's observation (`removed` when published and count 1) | cleanup-evidence test fails at the review's counterexample ("never removed on stale evidence") |
| `EEXIST` back in the definite-refusal codes | test 13 (retransmitted `EEXIST`) and the no-clobber test (contending saves) fail |

## WebSocket hand-off, revocation order and dispatched-link classification repair

The review of `74b7a2ad` found three remaining gaps. This section supersedes the
previous section where they differ: its WebSocket bullet (`start_send` into the split
sink's slot, then a flush in the same poll) and its statement that some link error
codes are definite refusals. It also supersedes the description of a revocation
concurrent with the decision step as "ordered after the decision" without a
synchronization relation.

**P1a, `SplitSink` slot taken as acceptance.** `SplitSink::start_send` (futures-util
0.3.33) only stores the frame in the sink's own slot. `poll_flush` must first obtain
the `BiLock` shared with `SplitStream`, and only then forwards the slot to
tungstenite. The writer treated that `poll_flush` returning `Pending` as acceptance
and settled the request. Counterexample: the reader holds the `BiLock`; the writer
stores the success in the slot and gets `Pending`; the request is settled; the
credential is revoked; the reader releases the lock; a later flush forwards the
stale success. In the current composition both halves are polled by one task, so
the lock was never contended there. Correctness therefore rested on an unstated
composition property, and on tokio-tungstenite's private `ready` flag, not on the
code. Invariant: a delivery success is never settled while it is held only in an
adapter staging slot.

Change: `websocket::Socket` owns the one `WebSocketStream` in a mutex shared by the
reader and the writer, each holding it for one non-suspending poll. Under that lock
`Socket::poll_hand_off` flushes earlier frames and pongs, waits for tungstenite's
`poll_ready`, and then decides and hands the frame over with tungstenite's
synchronous `start_send`. The record is either the writer's undecided `Outbound`,
or tungstenite's. Stages: queued record → (no adapter slot) → tungstenite accepted
(the publication point) → kernel socket buffer → peer. The one-message gap between
tungstenite and the kernel is unchanged and documented. The Product Host lane
uses the same `Socket`, `Outbound` and `Publication`, so it no longer has its own
send path or its own `SplitSink`.

**P1b, revocation not ordered against publication.** The decision ran under the
request's state lock, which ordered it against `delivery/cancel`, but credential
revocation (a token), close, detach and drain do not take that lock. A thread
could check the authority, another could complete a revocation, and the first
could then publish. Invariant: a cancellation or revocation that completes before
the acceptance prevents the success; one that overlaps the hand-off is ordered by
an explicit lock.

Change: one host-wide `delivery_access::Revocations` (a reader-writer lock).
`Publication::poll_hand_off` and the in-process `publish` hold its shared side from
the decision through the acceptance. Every revocation of delivery authority runs in
`Revocations::revoke`, the exclusive side: `bind_delivery_access` and
`bind_product_host` (old credential dropped inside it),
`revoke_delivery_access` (drain), `close` (authority and every attachment in one
revocation), `release_route` (detach), and Product Host disconnect.
`delivery/cancel` keeps the request's state lock, which the hand-off also holds.
Lock order: WebSocket stream, then revocations, then request state. Revocations take
the route table before the revocation order and never take the others. Neither
side suspends or waits for I/O. Residency ending is not a revocation: a route pins
residency until after its detach.

**P2, dispatched-link error codes as definite refusals.** The remaining
definite-refusal set (`ENOENT`, `EACCES`, `EROFS`, …) was still applied to a
dispatched link. Counterexample: the original link creates the destination;
someone moves the staged name and removes the destination; the retransmission
answers `ENOENT`; Save reported "not saved". Invariant: definite non-publication
only on evidence that rules a previous publication out.

Change: the definite-refusal set is gone. Refusal is definite only before dispatch:
the preflight `lstat(destination)` must answer `ENOENT`. An existing entry is
refused as "already exists"; any other answer (`ENAMETOOLONG`, `ENOTDIR`, `EACCES`, …)
as "cannot be inspected", with the cause kept. After dispatch: published if the
destination names F, otherwise `DeliveryUncertainError`, whatever the error. The
unsupported-link codes only add an explanation to the uncertain message. Cleanup is
unchanged.

| Regression | Synchronization | What it proves |
| --- | --- | --- |
| `websocket_writer_stages_nothing_while_the_reader_holds_the_stream` | real `WebSocketStream` over a `Valve`; a read `Hold` blocks the reader thread inside `poll_read` holding the stream; `Socket::waited` reports a contended lock | (1) success produced via `connection.reply` and wrapped as `Outbound`; (2–3) the writer, on another thread, waits for the stream, and tungstenite has taken 0 bytes; (4) `revoke_delivery_access` completes; (5) the reader releases; (6) the writer resumes; (7) the handed-over record and the frame the peer receives are id 30's `unauthorized` failure, never the success; permits back. |
| `delivery_revocation_overlapping_a_hand_off_is_ordered_after_its_acceptance` (stdio) | a write `Hold` blocks `poll_write` with the decided success in hand; `Revocations::waited` / probe `cancel_waited` report a waiting revocation or cancel | **credential / detach / close:** the revocation, on another thread, reports that it waits and has not completed. The release lets the pipe accept, the success for 30 stands, and the revocation completes after. A later delivery 34 is `unauthorized` / `stale_attachment`. **cancel:** the cancel waits and then returns `false`, and sibling delivery 34 succeeds. Every case: one response per id, `server/info` answered (except after close), permits back, no deadlock between retirement and publication. |
| `delivery_publication_linearizes_at_the_transports_first_accepted_byte` (stdio, kept) | `Valve` refusals | Revocation or cancel completed before the acceptance → typed failure for the same id. The credential case now uses the production `revoke_delivery_access`. `published` additionally revokes after the first accepted byte, and the full success still arrives. |
| `websocket_delivery_publication_is_decided_when_tungstenite_takes_the_frame` (kept) | `Valve` | Behind an unsent frame, revocation wins; a frame tungstenite took stands. |
| test 13 | real `link` with substituted acknowledgement; counted dispatches | Existing destination, a real `ENAMETOOLONG` name and an injected `EACCES` → refused with no link. Ordinary save → published. Retransmitted `EEXIST` → published or uncertain by identity. Original link, staged name moved and destination removed, `ENOENT` → uncertain (and with the destination still naming F → published). Destination replaced, `ENOENT` → uncertain, and the replacement is untouched. Permission change: `EACCES` with the destination naming F → published, and with it hidden → uncertain. A genuine refusal (`EACCES`, nothing created) → uncertain with the original cause. One `link` per save, never a rollback. |
| tests 3, 9, 11, 12, 14 and the cleanup rows | as before | A staged name moved, or a parent renamed, before the link → uncertain plus residue (was "not saved"). Cancellation before dispatch → nothing linked. After dispatch the link decides. An ambiguous error combined with residue reports both. |

Negative controls, each applied alone from a byte-checked backup and restored (sha256 verified):

| Control | Result |
| --- | --- |
| A. The WebSocket writer decides and stages the record before it holds the stream, as `SplitSink`'s slot did | `websocket_writer_stages_nothing…` fails: the handed-over record is the success (`Some(true)`). |
| B. Publication does not take the revocation order | the overlap test fails for `credential`, and, run per case, for `detach` and `close`: "… completed between the decision and the transport's acceptance". `cancel` passes, as designed, because it is ordered by the request lock. |
| C. The request lock is released before the acceptance | the overlap test's `cancel` case fails the same way. |
| D. A dispatched link's `ENOENT` is a definite refusal again | test 13 (case 4), test 3 and test 9 fail. |
| E. The preflight lets the link decide for an uninspectable destination | test 13 (`ENAMETOOLONG`, no link dispatched) fails. |

## Atomic credential rotation repair

The review of `432dcc14` found that credential rotation was not one transition.
This section supersedes the previous section's description of `bind_delivery_access`
and `bind_product_host` ("the old credential dropped inside it").

**Root cause.** Both methods swapped the credential slot under the slot's mutex,
released it, and only then dropped the previous grant in `Revocations::revoke`.
Between the two steps the new credential authenticated, a removal was already
visible, and the previous grant's token, and so every connection or Product Host
socket token minted from it, was still uncancelled. A publication could decide a
success on that old authority after the replacement became observable.
Counterexample: A is bound and an A-admitted connection has a publication in
progress; `bind_delivery_access(Some(B))` installs B, then waits for the
publication; B authenticates while A's connection can still publish.

**Invariant.** Replacing or removing a credential is one linearizable authority
transition: once the new state is observable the previous grant is revoked, and
no previous authority publishes a success afterwards.

**Repair.** One helper, `AppServerHost::rotate`, used by both bind methods. It
takes the exclusive side of `Revocations`, then the slot's mutex, and, holding
both, drops the previous grant (cancelling its token and every child token) and
installs the next. That critical section is the linearization point.
Authentication takes only the slot's mutex, so it sees one side. Publication holds
the shared side across decision and acceptance, so it is ordered by the lock.
Rotations take the exclusive side one at a time, in one total order. Lock order:
route table, then revocation order, then credential slot; authentication takes
only the slot; publication takes the WebSocket stream, then the revocation order,
then request state. Nothing takes the slot and then the revocation order, and
token cancellation runs no App Server code.

| Regression | Synchronization | What it proves |
| --- | --- | --- |
| `delivery_credential_rotation_is_one_transition_ordered_against_publication` (cases: rotate to B, remove) | an A-admitted WebSocket connection; a write `Hold` matching only id 30's success frame parks it inside tungstenite's acceptance; `Revocations::waited` counts waiting revocations | The rotation reports waiting and has not completed. Meanwhile B does not authenticate, and A does (state before). The release lets the success stand; the rotation completes; the token minted meanwhile and the connection's token are cancelled; only B authenticates (none after removal); the connection's next delivery is `unauthorized`; one response per id; permits back. |
| `product_host_credential_rotation_is_one_transition_ordered_against_publication` | a real Product Host socket admitted with A over a `Valve`; its id-0 success frame held | Same ordering on the Product Host path: B refused while the rotation waits, A then revoked, the accepted success stands with the delivered bytes, only B authenticates. The delivery-access credential, its token and an ordinary connection are unaffected. |
| `overlapping_delivery_credential_rotations_take_one_total_order` | the held publication; rotations to B, C and none started one after another, each observed waiting (counter 1, 2, 3) | The cfg(test) rotation log, recorded inside the critical section, holds exactly those three installations. The final authentication state is the last one's; every superseded credential is refused; the A connection is revoked; the publication stands. |
| `authentication_racing_a_rotation_sees_one_side_of_it` | an authentication with A parked (`ReadProbe::authenticating`) while it holds the slot; the rotation to B reports waiting for the slot (`credential_waited`) | The authentication returns a token minted from A, the state before; the rotation then cancels it; afterwards only B authenticates. |

Negative control, from a byte-checked backup (sha256 verified on restore): the
original replace-then-revoke body, with the test hooks kept. The delivery and Product
Host rotation tests fail with "B observable before A is revoked", and the removal
case alone fails with "A, before the rotation" (the removal was visible before A's
tokens were revoked). The overlapping-rotations and authentication-race tests pass
under the control: they verify ordering properties that the old code also met.

## Validation

See the pull request for the final command list and results; the PR description
records the exact pass/fail counts of the final head.
