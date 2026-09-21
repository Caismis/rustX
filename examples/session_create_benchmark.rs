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
//! [`reservation_counters`] and [`fs_operation_counters`]: the head reads the
//! storage owner's reservation, legacy-layout-probe and filesystem-operation
//! counters; the baseline copy returns `(None, None)` and `FsCounts::default()`.
//! Everything else uses only APIs that exist at the base revision.
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

/// Benchmark-local snapshot of the create-path filesystem operation counters.
/// Defined here (not imported) so the same source compiles against the base,
/// which has no such counters.
#[derive(Debug, Default, Clone, Copy)]
struct FsCounts {
    create_open: u64,
    mkdir: u64,
    write: u64,
    fsync: u64,
    rename: u64,
    dir_fsync: u64,
    catalog_logical_bytes_written: u64,
}

impl FsCounts {
    fn delta(self, before: Self) -> Self {
        Self {
            create_open: self.create_open.saturating_sub(before.create_open),
            mkdir: self.mkdir.saturating_sub(before.mkdir),
            write: self.write.saturating_sub(before.write),
            fsync: self.fsync.saturating_sub(before.fsync),
            rename: self.rename.saturating_sub(before.rename),
            dir_fsync: self.dir_fsync.saturating_sub(before.dir_fsync),
            catalog_logical_bytes_written: self
                .catalog_logical_bytes_written
                .saturating_sub(before.catalog_logical_bytes_written),
        }
    }
}

/// Revision-specific create-path filesystem-operation counters.
///
/// REVISION-SPECIFIC BODY — at base 0083f64d replace the marked body with
/// `FsCounts::default()`.
fn fs_operation_counters() -> FsCounts {
    // === BEGIN REVISION-SPECIFIC BODY (replace with `FsCounts::default()` at base 0083f64d) ===
    let snapshot = rustx::runtime::local_storage::fs_operations::snapshot();
    FsCounts {
        create_open: snapshot.create_open,
        mkdir: snapshot.mkdir,
        write: snapshot.write,
        fsync: snapshot.fsync,
        rename: snapshot.rename,
        dir_fsync: snapshot.dir_fsync,
        catalog_logical_bytes_written: snapshot.catalog_logical_bytes_written,
    }
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
    /// Create-path filesystem operation deltas over the timed loop. Logical
    /// syscall counts, not physical device I/O.
    fs_create_open_during_creates: u64,
    fs_mkdir_during_creates: u64,
    fs_write_during_creates: u64,
    fs_fsync_during_creates: u64,
    fs_rename_during_creates: u64,
    fs_dir_fsync_during_creates: u64,
    /// Logical payload bytes supplied to the catalog write operation during
    /// the timed loop. Not a sampled file length and not physical device bytes.
    catalog_logical_bytes_written_during_creates: u64,
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
    let fs_before = fs_operation_counters();
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
    let fs = fs_operation_counters().delta(fs_before);
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
        fs_create_open_during_creates: fs.create_open,
        fs_mkdir_during_creates: fs.mkdir,
        fs_write_during_creates: fs.write,
        fs_fsync_during_creates: fs.fsync,
        fs_rename_during_creates: fs.rename,
        fs_dir_fsync_during_creates: fs.dir_fsync,
        catalog_logical_bytes_written_during_creates: fs.catalog_logical_bytes_written,
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
            "catalog_bytes_note": "catalog_logical_bytes_written_during_creates is the logical payload supplied to the catalog write operation, not a sampled file length and not physical device bytes",
            "fs_ops_note": "create-path filesystem operation counts are logical syscall invocations over the timed loop, not physical device I/O",
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
