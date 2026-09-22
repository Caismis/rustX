//! Reproducible Session-create benchmark of the real native path (Issue #387).
//!
//! One source file, two revisions. Compiled against the base revision it
//! measures the pre-#387 runtime-wide identity scan; compiled against the
//! reservation head it measures the same workload with bounded identity
//! reservation. The workload, durability semantics and publication path are
//! identical on both sides: every measured call is the real
//! `SessionController::create_session`.
//!
//! The ONLY revision-specific code is in the marked bodies of
//! [`reservation_counters`] and [`logical_operation_counters`]: the head reads
//! the storage owner's reservation, legacy-layout-probe and logical-operation
//! counters; the baseline copy returns `(None, None)` and `None`. A base run
//! therefore reports the logical counters as `null` (not instrumented), never
//! as measured zeros. Everything else uses only APIs that exist at the base
//! revision.
//!
//! ## What the operation counters mean (and do not)
//!
//! The counters are **rustX logical owner operations**, not syscall counts.
//! Each is incremented once per operation *requested* by a named rustX owner at
//! its call site. One request may map to zero, one or many OS syscalls (for
//! example `create_dir_all` or `write_all`), and **SQLite-internal filesystem
//! work is not observed at all** except for the single rustX
//! `SqliteConversationStore::open` request. Physical device I/O (page cache,
//! filesystem compression, device scheduling) is not measurable from these
//! counters. Scripts reading this report must keep the three evidence layers
//! separate:
//!
//! 1. rustX logical owner operations (this report's `logical_*` fields);
//! 2. `SQLite` / library-internal filesystem work (excluded; only the open
//!    request is counted);
//! 3. physical device I/O (excluded; needs an external process/filesystem
//!    boundary such as a block-device trace).
//!
//! Run:
//! ```text
//! cargo run --release --example session_create_benchmark -- \
//!     --root /tmp/rustx-create --existing 100 --creates 30 \
//!     --json /tmp/rustx-create.json
//! ```
//!
//! Scaling runs use a fresh `--root` per point and record the *actual*
//! starting Session count, paginated to exhaustion. A timed batch grows its
//! own population, so the report also records the ending count: a batch that
//! begins at N and performs C creates runs against N, N+1, ... N+C-1, never a
//! fixed N relabelled as C fixed-N samples.

use std::path::{Path, PathBuf};
use std::time::Instant;

use rustx::durable::conversation_store_open_count;
use rustx::local_runtime::configuration::SessionConfigInput;
use rustx::local_runtime::session::SessionPersistentState;
use rustx::local_runtime::session_controller::SessionController;

struct Params {
    root: PathBuf,
    existing: usize,
    creates: usize,
    json: Option<PathBuf>,
}

fn parse_args() -> Result<Params, String> {
    let mut root = None;
    let mut existing = 0_usize;
    let mut creates = 150_usize;
    let mut json = None;
    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        let value = args.next().ok_or_else(|| format!("{arg} needs a value"))?;
        match arg.as_str() {
            "--root" => root = Some(PathBuf::from(value)),
            "--existing" => {
                existing = value.parse().map_err(|_| "bad --existing".to_owned())?;
            }
            "--creates" => creates = value.parse().map_err(|_| "bad --creates".to_owned())?,
            "--json" => json = Some(PathBuf::from(value)),
            other => return Err(format!("unknown argument {other}")),
        }
    }
    let root = root.ok_or_else(|| "--root <dir> is required".to_owned())?;
    if creates == 0 {
        return Err("--creates must be positive".to_owned());
    }
    Ok(Params {
        root,
        existing,
        creates,
        json,
    })
}

/// Revision-specific storage-owner counters.
///
/// REVISION-SPECIFIC BODY — everything between the BEGIN/END markers is the
/// one deliberate difference between the two benchmarked revisions:
///
/// - head (#387): read the reservation and legacy-layout-probe counters;
/// - base 0083f64d: replace the body with `(None, None)`.
fn reservation_counters() -> (Option<u64>, Option<u64>) {
    // === BEGIN REVISION-SPECIFIC BODY (replace with `(None, None)` at base 0083f64d) ===
    (
        Some(rustx::runtime::local_storage::conversation_reservation_count()),
        Some(rustx::runtime::local_storage::conversation_legacy_layout_probe_count()),
    )
    // === END REVISION-SPECIFIC BODY ===
}

/// Benchmark-local snapshot of the rustX logical owner-operation counters.
/// Defined here (not imported) so the same source compiles against the base,
/// which has no such counters. Every field is one rustX operation *request*,
/// never a syscall count and never a byte of physical device I/O.
#[derive(Debug, Default, Clone, Copy)]
struct LogicalCounts {
    reservation_marker_create: u64,
    reservation_marker_write: u64,
    reservation_marker_file_sync: u64,
    reservation_namespace_mkdir: u64,
    reservation_namespace_dir_sync: u64,
    session_allocation_mkdir: u64,
    conversation_allocation_mkdir: u64,
    sqlite_store_open_request: u64,
    catalog_temp_open: u64,
    catalog_payload_write: u64,
    catalog_file_sync: u64,
    catalog_rename: u64,
    catalog_directory_sync: u64,
    catalog_logical_bytes_written: u64,
}

impl LogicalCounts {
    fn delta(self, before: Self) -> Self {
        Self {
            reservation_marker_create: self
                .reservation_marker_create
                .saturating_sub(before.reservation_marker_create),
            reservation_marker_write: self
                .reservation_marker_write
                .saturating_sub(before.reservation_marker_write),
            reservation_marker_file_sync: self
                .reservation_marker_file_sync
                .saturating_sub(before.reservation_marker_file_sync),
            reservation_namespace_mkdir: self
                .reservation_namespace_mkdir
                .saturating_sub(before.reservation_namespace_mkdir),
            reservation_namespace_dir_sync: self
                .reservation_namespace_dir_sync
                .saturating_sub(before.reservation_namespace_dir_sync),
            session_allocation_mkdir: self
                .session_allocation_mkdir
                .saturating_sub(before.session_allocation_mkdir),
            conversation_allocation_mkdir: self
                .conversation_allocation_mkdir
                .saturating_sub(before.conversation_allocation_mkdir),
            sqlite_store_open_request: self
                .sqlite_store_open_request
                .saturating_sub(before.sqlite_store_open_request),
            catalog_temp_open: self
                .catalog_temp_open
                .saturating_sub(before.catalog_temp_open),
            catalog_payload_write: self
                .catalog_payload_write
                .saturating_sub(before.catalog_payload_write),
            catalog_file_sync: self
                .catalog_file_sync
                .saturating_sub(before.catalog_file_sync),
            catalog_rename: self.catalog_rename.saturating_sub(before.catalog_rename),
            catalog_directory_sync: self
                .catalog_directory_sync
                .saturating_sub(before.catalog_directory_sync),
            catalog_logical_bytes_written: self
                .catalog_logical_bytes_written
                .saturating_sub(before.catalog_logical_bytes_written),
        }
    }
}

/// Revision-specific rustX logical owner-operation counters.
///
/// REVISION-SPECIFIC BODY — at base 0083f64d replace the marked body with
/// `None` (the base has no such counters). The `Option` is required so the base
/// can report "not instrumented" as `null`; clippy cannot see the base stub.
#[allow(clippy::unnecessary_wraps)]
fn logical_operation_counters() -> Option<LogicalCounts> {
    // === BEGIN REVISION-SPECIFIC BODY (replace with `None` at base 0083f64d) ===
    let snapshot = rustx::runtime::local_storage::logical_operations::snapshot();
    Some(LogicalCounts {
        reservation_marker_create: snapshot.reservation_marker_create,
        reservation_marker_write: snapshot.reservation_marker_write,
        reservation_marker_file_sync: snapshot.reservation_marker_file_sync,
        reservation_namespace_mkdir: snapshot.reservation_namespace_mkdir,
        reservation_namespace_dir_sync: snapshot.reservation_namespace_dir_sync,
        session_allocation_mkdir: snapshot.session_allocation_mkdir,
        conversation_allocation_mkdir: snapshot.conversation_allocation_mkdir,
        sqlite_store_open_request: snapshot.sqlite_store_open_request,
        catalog_temp_open: snapshot.catalog_temp_open,
        catalog_payload_write: snapshot.catalog_payload_write,
        catalog_file_sync: snapshot.catalog_file_sync,
        catalog_rename: snapshot.catalog_rename,
        catalog_directory_sync: snapshot.catalog_directory_sync,
        catalog_logical_bytes_written: snapshot.catalog_logical_bytes_written,
    })
    // === END REVISION-SPECIFIC BODY ===
}

struct SystemUnits {
    ticks_per_second: f64,
    page_size: u64,
}

impl SystemUnits {
    fn capture() -> Self {
        #[allow(clippy::cast_precision_loss)]
        Self {
            ticks_per_second: getconf("CLK_TCK").map_or(100.0, |value| value as f64),
            page_size: getconf("PAGE_SIZE").unwrap_or(4096),
        }
    }
}

fn getconf(name: &str) -> Option<u64> {
    std::process::Command::new("getconf")
        .arg(name)
        .output()
        .ok()
        .and_then(|output| String::from_utf8(output.stdout).ok())
        .and_then(|stdout| stdout.trim().parse().ok())
}

#[allow(clippy::cast_precision_loss)]
fn cpu_seconds(units: &SystemUnits) -> f64 {
    let stat = std::fs::read_to_string("/proc/self/stat").expect("stat");
    let remainder = stat.rsplit_once(") ").expect("stat comm terminator").1;
    let fields: Vec<&str> = remainder.split_whitespace().collect();
    let utime: u64 = fields[11].parse().expect("stat utime (field 14)");
    let stime: u64 = fields[12].parse().expect("stat stime (field 15)");
    (utime + stime) as f64 / units.ticks_per_second
}

fn rss_bytes(units: &SystemUnits) -> u64 {
    let statm = std::fs::read_to_string("/proc/self/statm").expect("statm");
    let resident_pages: u64 = statm
        .split_whitespace()
        .nth(1)
        .expect("statm resident field")
        .parse()
        .expect("statm resident pages");
    resident_pages * units.page_size
}

fn catalog_len(root: &Path) -> u64 {
    std::fs::metadata(root.join("sessions/catalog.json")).map_or(0, |m| m.len())
}

/// Authoritative Session-domain count: page through the metadata-only list to
/// exhaustion. Never opens a `ConversationStore`.
async fn count_sessions(controller: &SessionController) -> Result<usize, String> {
    let mut offset = 0_usize;
    let mut total = 0_usize;
    loop {
        let page = controller
            .list_sessions(None, offset, 32)
            .await
            .map_err(|error| format!("{error:?}"))?;
        total += page.sessions.len();
        match page.next_offset {
            Some(next) => offset = next,
            None => return Ok(total),
        }
    }
}

#[derive(serde::Serialize)]
struct CreateResult {
    /// The requested fixture size.
    existing_sessions_requested: usize,
    /// The authoritative observed Session count before the timed creates. The
    /// batch grows the fixture, so subsequent operations run against
    /// `existing_sessions_observed_start + i` Sessions, not a fixed N.
    existing_sessions_observed_start: usize,
    /// The authoritative observed Session count after the timed creates.
    existing_sessions_observed_end: usize,
    /// Number of timed creates; the batch population spans
    /// `[existing_sessions_observed_start, existing_sessions_observed_end]`.
    measured_creates: usize,
    /// Wall time of the measured create loop divided by the number of creates.
    wall_us_per_create: f64,
    /// Process utime+stime delta over the measured loop, per create.
    cpu_us_per_create: f64,
    /// Point-in-time RSS samples, not peak RSS.
    rss_bytes_after: u64,
    rss_bytes_before: u64,
    store_opens_during_creates: u64,
    reservation_count_during_creates: Option<u64>,
    legacy_layout_probes_during_creates: Option<u64>,
    /// rustX logical owner-operation deltas over the timed loop. Each is one
    /// requested operation, NOT a syscall count and NOT physical device I/O.
    /// `null` means the revision has no such counters (base), never a measured
    /// zero.
    logical_reservation_marker_create: Option<u64>,
    logical_reservation_marker_write: Option<u64>,
    logical_reservation_marker_file_sync: Option<u64>,
    logical_reservation_namespace_mkdir: Option<u64>,
    logical_reservation_namespace_dir_sync: Option<u64>,
    logical_session_allocation_mkdir: Option<u64>,
    logical_conversation_allocation_mkdir: Option<u64>,
    /// rustX `SqliteConversationStore::open` requests. SQLite-internal
    /// filesystem work is excluded from every counter in this report.
    logical_sqlite_store_open_request: Option<u64>,
    logical_catalog_temp_open: Option<u64>,
    logical_catalog_payload_write: Option<u64>,
    logical_catalog_file_sync: Option<u64>,
    logical_catalog_rename: Option<u64>,
    logical_catalog_directory_sync: Option<u64>,
    /// Logical payload bytes supplied to the catalog write request during
    /// the timed loop. Not a sampled file length and not physical device bytes.
    logical_catalog_bytes_written: Option<u64>,
    catalog_bytes_final: u64,
}

async fn run(params: &Params) -> Result<CreateResult, String> {
    if params.root.exists()
        && std::fs::read_dir(&params.root)
            .map_err(|error| error.to_string())?
            .next()
            .is_some()
    {
        return Err(format!(
            "--root {} must be absent or empty",
            params.root.display()
        ));
    }
    std::fs::create_dir_all(&params.root).map_err(|error| error.to_string())?;
    let controller = SessionController::open(&params.root).map_err(|error| format!("{error:?}"))?;
    let workspace = params.root.join("workspace");
    std::fs::create_dir_all(&workspace).map_err(|error| error.to_string())?;
    let template = SessionPersistentState::from_input(&SessionConfigInput::new(workspace));
    for _ in 0..params.existing {
        controller
            .create_session(template.clone())
            .await
            .map_err(|error| format!("{error:?}"))?;
    }
    // Authoritative fixture count. A single page is capped at the list page
    // limit (32); paginate to exhaustion so `--existing 100`/`1000` is not
    // silently reported as 32.
    let existing_observed_start = count_sessions(&controller).await?;
    if existing_observed_start != params.existing {
        return Err(format!(
            "seeded fixture size {} does not match requested --existing {}",
            existing_observed_start, params.existing
        ));
    }
    let units = SystemUnits::capture();
    let rss_before = rss_bytes(&units);
    let cpu_before = cpu_seconds(&units);
    let opens_before = conversation_store_open_count();
    let (reservations_before, probes_before) = reservation_counters();
    let logical_before = logical_operation_counters();
    let started = Instant::now();
    for _ in 0..params.creates {
        controller
            .create_session(template.clone())
            .await
            .map_err(|error| format!("{error:?}"))?;
    }
    let wall = started.elapsed();
    let cpu = cpu_seconds(&units) - cpu_before;
    let opens = conversation_store_open_count() - opens_before;
    let (reservations_after, probes_after) = reservation_counters();
    let logical = logical_operation_counters()
        .zip(logical_before)
        .map(|(after, before)| after.delta(before));
    let existing_observed_end = count_sessions(&controller).await?;
    #[allow(clippy::cast_precision_loss)]
    let creates = params.creates as f64;
    Ok(CreateResult {
        existing_sessions_requested: params.existing,
        existing_sessions_observed_start: existing_observed_start,
        existing_sessions_observed_end: existing_observed_end,
        measured_creates: params.creates,
        wall_us_per_create: wall.as_secs_f64() * 1_000_000.0 / creates,
        cpu_us_per_create: cpu * 1_000_000.0 / creates,
        rss_bytes_after: rss_bytes(&units),
        rss_bytes_before: rss_before,
        store_opens_during_creates: opens,
        reservation_count_during_creates: reservations_after
            .zip(reservations_before)
            .map(|(after, before)| after.saturating_sub(before)),
        legacy_layout_probes_during_creates: probes_after
            .zip(probes_before)
            .map(|(after, before)| after.saturating_sub(before)),
        logical_reservation_marker_create: logical.map(|l| l.reservation_marker_create),
        logical_reservation_marker_write: logical.map(|l| l.reservation_marker_write),
        logical_reservation_marker_file_sync: logical.map(|l| l.reservation_marker_file_sync),
        logical_reservation_namespace_mkdir: logical.map(|l| l.reservation_namespace_mkdir),
        logical_reservation_namespace_dir_sync: logical.map(|l| l.reservation_namespace_dir_sync),
        logical_session_allocation_mkdir: logical.map(|l| l.session_allocation_mkdir),
        logical_conversation_allocation_mkdir: logical.map(|l| l.conversation_allocation_mkdir),
        logical_sqlite_store_open_request: logical.map(|l| l.sqlite_store_open_request),
        logical_catalog_temp_open: logical.map(|l| l.catalog_temp_open),
        logical_catalog_payload_write: logical.map(|l| l.catalog_payload_write),
        logical_catalog_file_sync: logical.map(|l| l.catalog_file_sync),
        logical_catalog_rename: logical.map(|l| l.catalog_rename),
        logical_catalog_directory_sync: logical.map(|l| l.catalog_directory_sync),
        logical_catalog_bytes_written: logical.map(|l| l.catalog_logical_bytes_written),
        catalog_bytes_final: catalog_len(&params.root),
    })
}

#[tokio::main(flavor = "current_thread")]
async fn main() -> Result<(), String> {
    let params = parse_args()?;
    let result = run(&params).await?;
    let report = serde_json::json!({
        "environment": {
            "profile": if cfg!(debug_assertions) { "debug" } else { "release" },
            "workload_path": "SessionController::create_session (real native prepare+publish)",
            "cpu_source": "/proc/self/stat utime+stime (clock ticks) over getconf CLK_TCK, delta over the timed loop",
            "rss_source": "/proc/self/statm resident pages x getconf PAGE_SIZE (== VmRSS), point-in-time samples",
            "store_opens_source": "rustx::durable::conversation_store_open_count() delta over the timed loop",
            "catalog_bytes_note": "logical_catalog_bytes_written is the logical payload supplied to the catalog write request, not a sampled file length and not physical device bytes",
            "logical_operations_note": "every logical_* field is one rustX owner operation request over the timed loop, NOT a syscall count; create_dir_all/write_all may map to zero or many syscalls. null means the revision has no such counters, never a measured zero",
            "excluded_library_io_note": "SQLite-internal filesystem work (journal, schema, identity binding, internal fsyncs) is NOT observed; only logical_sqlite_store_open_request (one rustX SqliteConversationStore::open call) is counted",
            "excluded_device_io_note": "physical device I/O is NOT measured by these counters; it requires an external process/block-device boundary",
            "excluded_owner_note": "the once-per-root sessions/ directory setup and catalog-root create_dir_all no-op requests are not instrumented and are excluded",
            "population_note": "the timed batch runs against existing_sessions_observed_start, +1, ... existing_sessions_observed_end; it is not N independent fixed-N creates",
            "count_source": "authoritative metadata-only Session list paginated to exhaustion; never opens a ConversationStore",
        },
        "result": result,
    });
    let pretty = serde_json::to_string_pretty(&report).map_err(|error| error.to_string())?;
    println!("{pretty}");
    if let Some(path) = &params.json {
        std::fs::write(path, format!("{pretty}\n")).map_err(|error| error.to_string())?;
    }
    Ok(())
}
