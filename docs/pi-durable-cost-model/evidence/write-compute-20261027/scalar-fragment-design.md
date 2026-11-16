# Eager request-scalar sharing: read-only design, 2026-10-27

Base inspected: 694db002c96ed93c610280c14467cbac1f8a1a73 (retained D).
No stable-fragment implementation has been made. The separately authorized
bounded-subtree validation patch is described at the end; it is not a substitute
for the larger capture/storage change below.

## Verified current capabilities and costs

- `composed::index_selection` builds a full allowlisted scalar Source with an
  empty messages array on every request. `fragments::scalar` then walks field
  geometry, computes eager previews, spools F/C facts, encodes authenticated index
  pages, hashes them, and issues content-addressed INSERT OR IGNORE statements.
- `index::Builder`, `index::{get,load_blob,store_blob}`, and `Fragment` already
  implement bounded authenticated storage and eager navigation. They must remain
  the reader representation. E's on-read Source reconstruction must not return.
- Deduplicating `request_source_pages` happens after all producer work. Scalar
  Fact start/end offsets include the dynamic request prefix. Changes in prefix
  length can change later Facts/page hashes even when tool definitions are equal.
- Existing message composition already separates byte fragments from dynamic
  holes and maps local spans to absolute spans. `read::{selected,child,piece}` are
  the concrete extension points. This does not require a generic graph/rope or a
  second field indexing implementation.
- Frozen inspection has 24 serialized top-level fields despite the serializer's
  capacity hint of 27. They are: contributions; request_id; attempt_id; step_id;
  retry_number; assistant_message_id; model; protocol; max_output_tokens;
  context_window_tokens; reasoning_enabled; profile; options;
  omitted_option_count; effective_system_prompt; predecessor;
  previous_system_prompt; messages; messages_truncated; tools; tools_truncated;
  usage; failure; generation. Tools is currently ordinal 19; messages is 17.
- Snapshot storage is a separate remaining cost: `encode_snapshot` clones and
  serializes all scalar metadata, while `stored_snapshot_digest` binds the full
  metadata and native selected root. The semantic-input fingerprint also mixes
  history and scalar configuration. None is an existing stable config identity.
- `capability_revision` and process resource revisions are not adequate reuse
  keys: Snapshot explicitly freezes tools by value because revision metadata is
  not historical definition authority. Current configuration comparison objects
  are likewise not durable immutable capture identities.
- Source has an existing bounded byte carrier: 16 KiB resident bytes, one-way
  file spill, a separate indexing stage (`Layout::unindexed` / `from_layout`).
  Public `SeekableObservationSource::new` currently couples encoding and indexing.

## Recommended smallest first layer: share only the tools subtree

The large/dense tools subtree is the clearest first unit. Do not begin by
splitting every scalar field or by normalizing all native Snapshot storage.
The latter is independently useful but expands this change into native
reconstruction, fingerprint and idempotency semantics.

1. Keep the per-request scalar's existing typed allowlist, order and all dynamic
   values, but encode `tools: []` as a second exact hole beside `messages: []`.
   Refactor the current borrowed `Tools` serializer so the actual tool view is
   authored once and reusable by the whole-request producer and a narrow
   `{"tools": actual_tools}` capture wrapper. Its JsonSchema remains the real
   TraceRequestDetail schema; do not serialize tools through generic Value, which
   can change native-u64 structural handling.
2. Encode that tools wrapper once into the existing bounded byte carrier while
   computing a domain-separated SHA-256 over exact structural bytes. Look up a
   durable immutable tools-fragment record by that digest. On a miss, index the
   already encoded carrier and eagerly capture its tools value span. On a hit,
   load and verify only the bounded fragment descriptor. Do not first build a
   Source just to obtain its hash, and do not serialize a second time on misses.
3. The immutable record contains the input digest, the existing `Fragment`, the
   tools root's capped node count and child count, and the captured top-level
   ordinal. Its metadata checksum covers all of these. Its byte and Fact data
   use existing request_source_pages. It is durable representation reuse,
   rather than a process cache or a live-configuration lookup. A matching row
   with malformed/mismatched metadata is an error, not a miss/rebuild fallback.
4. A request root stores the exact tools Fragment descriptor and counts it bound
   at capture. This keeps existing readers independent of a new per-operation
   descriptor lookup; the extra descriptor is bounded. The lookup table is
   needed only when capturing a new request, and contains no duplicate body.
   Alternatively, storing only a descriptor hash reduces root metadata but adds
   one authenticated lookup to every read operation; that should not be chosen
   without measuring the small-field read cost.
5. Keep existing request source/native selected-root/frontier bindings, optional
   display-layout savepoint semantics, and execution transactions. Change the
   store schema version once; no old shape, migration or fallback remains.

### Exact subtree placement and aliases

A tools-only wrapper gives the tools field ordinal 0, while the actual request
has ordinal 19. Do not silently retain that wrong Fact ordinal. Capture the
actual hole's parent and ordinal from the per-request base Source and explicitly
bind the standalone subtree's top-level Fact to that placement. The digest key
must include the placement/schema discriminator, or reuse must assert the stored
placement equals the actual hole. Descendant paths, parent paths and ordinals
remain those produced by the ordinary Source traversal: they already start with
`tools` and require no recursive path rewriting. This is a narrow top-level
attachment, not general permissive alias navigation.

Keep the original `@ordinal` aliases, `$key` synthetic facts, encoded key spans,
short-key names and existing 3600/4096 path limits. The root child directory is
still the per-request scalar's C index, so top-level order is captured rather
than reimplemented as a second hardcoded allowlist. Nested tools children use
the shared fragment's existing C index. Validate the actual plain `tools` hole,
its empty-array width of two bytes, and its position after `messages`.

### Minimal Root and read changes

Replace the implicit single scalar hole with two fixed, validated holes; do not
introduce an arbitrary fragment sequence. Root retains sequence/native/source
hash, the per-request base Fragment, and messages_start/messages_end. Add tools
Fragment, tools_start/tools_end in base coordinates, tools_children and capped
tools_nodes. The scalar's messages and tools placeholders must each have width
2, ordered and non-overlapping. New length is:

    base.length + (selected_messages_length - 2) + (tools.length - 2)

All lengths/additions retain SAFE checks. Root checksum covers these descriptors.
Read-side loading checks counts, lengths, hole order and original native/frontier
bindings before using offsets.

- `selected`: existing message descent is unchanged except for the explicit
  message-length calculation. A path beginning with `tools` selects an eager
  Fact from the shared fragment and adds the actual tools origin (base tools
  start plus message expansion). Other paths select the base Fact and translate
  each endpoint by the expansion of each hole it follows. Root spans include
  both expansions. No eager field preview is replaced by on-demand parsing.
- `child`: root children continue using base C entries. Nested tools children
  use the shared C index. Message child selection remains unchanged.
- Root node count: for no selected messages, replace the base empty-tools node
  by tools_nodes, saturating at 33. If base nodes is already 33, replacement by
  a nonempty JSON value cannot reduce it. Existing selected-message handling
  already proves the full root exceeds the preview gate. Do not use an
  approximate node count below the 32-node gate.
- `piece`: before/between/after holes, read base fragment bytes using translated
  coordinates; the messages hole uses existing sequence_piece, and the tools
  hole uses existing fragments::bytes. This is two fixed holes, not a new rope.
- `preview`, field headers, page limits and read_exact/range chunk limits keep
  their current semantics. A range crossing either hole or both must produce
  the exact old bytes, including commas, quotes, escapes and array brackets.

## What it can and cannot remove

A reuse hit can remove tools Source parsing/layout nodes, all tools preview
construction, F/C spooling/sorting, tool-page encoding/hash and attempted page
inserts. Relative tool offsets become independent of request-prefix length;
therefore those Fact pages can actually remain immutable across requests.
The request's small base scalar, root/frontier and dynamic values still need
capture. The first use of every distinct tools view still does full validation.

This version still serializes and hashes the exact tools view once per request
to establish reuse, and >16 KiB encoded carriers still spill to a temporary file.
It does not remove full native snapshot_json storage or semantic/native digest
work. Eliminating even that identity scan requires a native immutable config
identity established when tools are frozen, with changed-configuration bindings
and native historical reconstruction normalized around it. That is a separate
semantic storage change, not a safe use of capability_revision as a cache key.

No numerical speedup or write reduction is inferred. Existing byte pages already
sometimes deduplicate; the net durable write reduction depends on displaced
field-page churn versus the new bounded root/descriptor data. New reader work
should remain a fixed offset selection plus existing indexed lookup, but a
small fixed overhead still needs explicit read measurement.

## Extension to model/settings after the first layer

The same representation can later share the stable contiguous middle (model
through previous_system_prompt) and the stable tail (messages_truncated through
generation, containing tools), with the first six fields as a dynamic prefix
and the existing messages tree between them. Existing persist_part supports
non-JSON prefix/suffix slices with complete captured fields, exactly as message
fragments do. Capture must preserve all 24 original top-level ordinals.

This broader form needs shared root child placement metadata and root preview
node-count composition; it is substantially more code than the tools-only
second-hole change. It should not be added speculatively to the first patch.
Changes to model/protocol/options/omission count/system prompt/profile/reasoning
must select a new exact content version; private options absent from the public
allowlist do not need a different public fragment when their visible omission
count and other visible fields are unchanged. Native snapshot integrity remains
independent and must continue to bind the full actual private configuration.

## Tests and evidence required before retention

- Reuse the existing independent legacy_request_source_bytes and full field
  header oracle. Compare complete root bytes, every field/header/key span,
  child pages, root preview and 257-byte ranges for first capture and reuse.
- Run the previous explicit Reader workload on small and near-16 KiB dense
  tools schemas; include >=16 KiB sources, warm and reopened connection states.
  Compare field/page32/root-preview/range, not only whole-request reconstruction.
- Vary prefix digit widths, contribution sizes, selected history widths and
  retry_number while sharing tools. Read both old and new request roots after
  capture, settlement and database reopen. Include the empty messages case.
- Change tool ID/name/description/schema/order, empty tools and return to a prior
  version. Prove exact old historical bytes persist, and only unchanged views
  reuse a descriptor. Test model/config changes independently from tools.
- Escaped Unicode, native u64 versus opaque numeric values, 31/32/33-node preview
  gates, 16383/16384/16385-byte spans, 8192-byte chunks, 65536-byte index-page
  bounds, huge/ambiguous keys, ordinary duplicate keys and deep/invalid values.
- Corrupt registry digest/descriptor/checksum, root hole offsets/counts, shared
  pages, child paths and native/frontier identity independently. A corrupt hit
  must fail at capture; corrupted page bytes must fail when read, as today.
  Check rollback leaves no partially bound request root. Do not claim this
  establishes an independent original-capture witness against coordinated edits.
- Test counters must show unchanged tools create no tools Source nodes, previews,
  Fact-spool rows or page-build work after the first capture; instrument the
  exact removed work rather than treating INSERT OR IGNORE as free.
- Use paired ordinary O3 full lifecycle comparisons including CPU, process writes,
  memory and adverse samples; separately retain read diagnostics and fresh CPU
  attribution. Preserve every unsuccessful candidate and setup failure.

## Scope and size judgment

Likely production files: trace/frozen.rs (shared tools view), read_layout.rs
(two-stage encoding using the existing carrier), composed.rs (Root/capture),
composed/fragments.rs (attached eager subtree), composed/read.rs (second hole),
composed/schema.rs, sqlite.rs schema version. Focused differential/corruption and
reader tests add separate modules. A tools-only implementation is approximately
several hundred production lines, probably 300-500 depending on reuse of the
existing encoder; tests are comparable or larger. This is a design estimate,
not a measured diff. A full middle/tail config layout is more likely 600-900
production lines plus tests. Both need a separate candidate and review; neither
belongs in the current seal/staging patch by incidental expansion.

## Authorized smaller patch now prepared

Only fragments.rs and metadata_tests.rs were edited, without running builds or
tests. capture_facts remembers one successfully validated bounded subtree span.
A later >32-node field entirely inside that span uses 33. Original count and
Source count both already saturate at 33 and count original duplicate entries.
A successful parent serde parse proves descendant numeric/string validity and
satisfies a stricter relative depth bound; failed validation never extends the
span. All eager previews, Facts and authenticated page bytes remain unchanged.

New tests compare the independent original producer's exact page bytes and
accept/reject outcomes, and isolate validation byte reads from preview reads.
They cover dense <16 KiB whole-source validation, >16 KiB separated bounded
subtrees, reserved-key duplicates, ordinary duplicates, hidden 1e400 values,
depth 123..130, node gates and exact 16383/16384/16385-byte sources.

Fresh D CPU samples supplied by root show original_node_count only one sample
in each small-history mode; persist_part accounts for 33/253 and 38/269 no-live
samples, and 33/374 and 44/503 live samples. The interval patch is an explicit
removal of redundant work, but is not supported as a principal CPU improvement.
