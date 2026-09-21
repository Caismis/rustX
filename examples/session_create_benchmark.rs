//! Reproducible Session-create benchmark of the real native path (Issue #387).
//!
//! One source file, two revisions. Compiled against the base revision it
//! measures the pre-#387 runtime-wide identity scan; compiled against the
//! reservation head it measures the same workload with bounded identity
//! reservation. The workload, durability semantics and publication path are
//! identical on both sides: every measured call is the real
//! `SessionController::create_session`.
//!
//! The ONLY revision-specific code is the marked body of
//! [`reservation_counters`]: the head reads the storage owner's reservation and
//! legacy-layout-probe counters, the baseline copy returns `(None, None)`.
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
//! starting Session count; sessions are never appended to one fixture and then
//! relabelled as having the original count.

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

#[derive(serde::Serialize)]
struct CreateResult {
    existing_sessions_at_start: usize,
    existing_sessions_after_seed: usize,
    measured_creates: usize,
    /// Wall time of the measured create loop divided by the number of creates.
    wall_us_per_create: f64,
    /// Process utime+stime delta over the measured loop, per create.
    cpu_us_per_create: f64,
    /// Peak sample is not collected; this is point-in-time RSS at the end.
    rss_bytes_after: u64,
    rss_bytes_before: u64,
    store_opens_during_creates: u64,
    reservation_count_during_creates: Option<u64>,
    legacy_layout_probes_during_creates: Option<u64>,
    /// Sum of `catalog.json` lengths observed immediately before each measured
    /// create: a logical payload estimate, not a physical device-write counter.
    catalog_payload_bytes_estimate: u64,
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
    let existing_after_seed = controller
        .list_sessions(None, 0, 32)
        .await
        .map_err(|error| format!("{error:?}"))?
        .sessions
        .len();
    let units = SystemUnits::capture();
    let rss_before = rss_bytes(&units);
    let cpu_before = cpu_seconds(&units);
    let opens_before = conversation_store_open_count();
    let (reservations_before, probes_before) = reservation_counters();
    let mut catalog_payload = 0_u64;
    let started = Instant::now();
    for _ in 0..params.creates {
        catalog_payload += catalog_len(&params.root);
        controller
            .create_session(template.clone())
            .await
            .map_err(|error| format!("{error:?}"))?;
    }
    let wall = started.elapsed();
    let cpu = cpu_seconds(&units) - cpu_before;
    let opens = conversation_store_open_count() - opens_before;
    let (reservations_after, probes_after) = reservation_counters();
    #[allow(clippy::cast_precision_loss)]
    let creates = params.creates as f64;
    Ok(CreateResult {
        existing_sessions_at_start: params.existing,
        existing_sessions_after_seed: existing_after_seed,
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
        catalog_payload_bytes_estimate: catalog_payload,
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
            "catalog_payload_note": "sum of catalog.json lengths before each create: logical payload bytes, not device writes",
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
