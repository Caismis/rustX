//! Reproducible resource benchmark of the real native Session list/search
//! path (Issue #386).
//!
//! One source file, two revisions. Compiled against the base revision
//! (previews derived at list time) it measures the old design; compiled
//! against the persisted-projection head it measures the new one. The ONLY
//! revision-specific code is the marked body of [`seed_previews`]: the head
//! populates persisted previews through the real repair seam, the baseline
//! copy deletes the body (base derives previews at list time, so there is
//! nothing to seed). Everything else uses only APIs that exist at the base
//! revision.
//!
//! Durability is never weakened: seeding commits through the same fsync'd
//! owners as production, and every measured call is the real
//! `SessionController::list_sessions` (the app server's own path into
//! `SessionCatalog::list_page`).
//!
//! Run:
//! ```text
//! cargo run --release --example session_list_benchmark -- \
//!     --root /tmp/rustx-bench --sessions 128 --messages 10 --reps 50 \
//!     --json /tmp/rustx-bench.json
//! ```

use std::path::{Path, PathBuf};
use std::time::Instant;

use rustx::durable::{ConversationStore, SqliteConversationStore, conversation_store_open_count};
use rustx::local_runtime::configuration::SessionConfigInput;
use rustx::local_runtime::session::{
    SESSION_LIST_PAGE_LIMIT, SessionPersistentState, SessionSummary,
};
use rustx::local_runtime::session_controller::SessionController;
use rustx::message::content::TextBlock;
use rustx::message::types::{
    AssistantContentBlock, AssistantMessageBlock, InboundKind, MessageBlock, UserContentBlock,
    UserMessageBlock, UserSource,
};
use rustx::runtime::identity::MessageId;

struct Params {
    root: PathBuf,
    sessions: usize,
    messages: usize,
    reps: usize,
    json: Option<PathBuf>,
}

fn parse_args() -> Result<Params, String> {
    let mut root = None;
    let mut sessions = 128_usize;
    let mut messages = 10_usize;
    let mut reps = 50_usize;
    let mut json = None;
    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        let value = args.next().ok_or_else(|| format!("{arg} needs a value"))?;
        match arg.as_str() {
            "--root" => root = Some(PathBuf::from(value)),
            "--sessions" => {
                sessions = value.parse().map_err(|_| "bad --sessions".to_owned())?;
            }
            "--messages" => {
                messages = value.parse().map_err(|_| "bad --messages".to_owned())?;
            }
            "--reps" => reps = value.parse().map_err(|_| "bad --reps".to_owned())?,
            "--json" => json = Some(PathBuf::from(value)),
            other => return Err(format!("unknown argument {other}")),
        }
    }
    let root = root.ok_or_else(|| "--root <dir> is required".to_owned())?;
    if sessions == 0 || messages == 0 || reps == 0 {
        return Err("--sessions, --messages and --reps must be positive".to_owned());
    }
    Ok(Params {
        root,
        sessions,
        messages,
        reps,
        json,
    })
}

/// One user text block, the same shape the scripted suites seed.
fn user(id: &str, value: &str) -> MessageBlock {
    MessageBlock::User(UserMessageBlock {
        id: MessageId::new(id),
        content: vec![UserContentBlock::Text(TextBlock {
            text: value.to_owned(),
        })],
        source: UserSource::Human,
        kind: InboundKind::Message,
        timestamp: None,
    })
}

fn assistant(id: &str, value: &str) -> MessageBlock {
    MessageBlock::Assistant(AssistantMessageBlock {
        id: MessageId::new(id),
        content: vec![AssistantContentBlock::Text(TextBlock {
            text: value.to_owned(),
        })],
    })
}

/// The first user message of session `index` — the preview subject. Even
/// sessions start with `alpha`, odd with `beta`, so a preview search for
/// `alpha` matches a known half of the rows. Lengths vary; every third
/// session carries a long multi-script first line past the 120-character
/// bound.
fn subject_text(index: usize) -> String {
    let prefix = if index.is_multiple_of(2) {
        "alpha"
    } else {
        "beta"
    };
    match index % 3 {
        0 => format!(
            "{prefix} topic {index}: {}",
            "a very long discussion of the persisted display projection with émojis and 汉字 "
                .repeat(4)
        ),
        1 => format!("{prefix} topic {index}: short brief"),
        _ => format!("{prefix} topic {index}: a medium length opening message with unicode café"),
    }
}

/// Seeds `params.sessions` Sessions through the real owners: the controller's
/// own session creation, one canonical history per root conversation appended
/// through the durable store with production durability, and a name on every
/// fourth Session (half `north-*`, half `south-*`).
async fn seed(params: &Params) -> Result<Vec<rustx::local_runtime::session::SessionId>, String> {
    let controller = SessionController::open(&params.root).map_err(|error| format!("{error:?}"))?;
    let workspace = params.root.join("workspace");
    std::fs::create_dir_all(&workspace).map_err(|error| error.to_string())?;
    let template = SessionPersistentState::from_input(&SessionConfigInput::new(workspace));
    let mut ids = Vec::with_capacity(params.sessions);
    for index in 0..params.sessions {
        let session = controller
            .create_session(template.clone())
            .await
            .map_err(|error| format!("{error:?}"))?
            .session;
        let access = controller
            .acquire_session(&session.id, None)
            .await
            .map_err(|error| format!("{error:?}"))?;
        let store = SqliteConversationStore::open(
            access.node.conversation_id.clone(),
            &access.database_path,
        )
        .map_err(|error| format!("{error:?}"))?;
        for ordinal in 0..params.messages {
            let message = if ordinal % 2 == 0 {
                let text = if ordinal == 0 {
                    subject_text(index)
                } else {
                    format!("follow-up {ordinal} of session {index}")
                };
                user(&format!("m-{index}-{ordinal}"), &text)
            } else {
                assistant(
                    &format!("m-{index}-{ordinal}"),
                    &format!("answer {ordinal}"),
                )
            };
            store
                .append_canonical(&message)
                .map_err(|error| format!("{error:?}"))?;
        }
        drop(store);
        if index.is_multiple_of(4) {
            let region = if index.is_multiple_of(8) {
                "north"
            } else {
                "south"
            };
            controller
                .rename_session(&session.id, &format!("{region}-project-{index}"))
                .await
                .map_err(|error| format!("{error:?}"))?;
        }
        ids.push(session.id);
    }
    Ok(ids)
}

/// Populates the persisted display projection of every seeded Session.
///
/// REVISION-SPECIFIC BODY — everything between the BEGIN/END markers is the
/// one deliberate difference between the two benchmarked revisions:
///
/// - head (persisted projection): populates previews through the real
///   explicit repair seam, exactly the backfill a reopen/compose/recovery
///   would run;
/// - base 1857d65f (derive at list time): delete the marked body. Base keeps
///   no projection, so there is nothing to seed; the workload side derives
///   each row's preview per page, which is precisely what the benchmark
///   measures.
async fn seed_previews(root: &Path, session_ids: &[rustx::local_runtime::session::SessionId]) {
    // Kept outside the markers so the emptied baseline body has no unused
    // bindings.
    let _ = (root, session_ids);
    // === BEGIN REVISION-SPECIFIC BODY (delete at base 1857d65f) ===
    let controller = SessionController::open(root).expect("reopen the seeded controller");
    for id in session_ids {
        let repair = controller
            .repair_display_preview(id)
            .await
            .expect("repair runs over the seeded history");
        assert!(
            matches!(
                repair,
                rustx::local_runtime::session_controller::DisplayPreviewRepair::Published
                    | rustx::local_runtime::session_controller::DisplayPreviewRepair::AlreadyPresent
            ),
            "every seeded session has a renderable first message, got {repair:?}"
        );
    }
    // === END REVISION-SPECIFIC BODY ===
}

/// System unit facts, captured once: clock ticks per second (for the
/// `/proc/self/stat` CPU counters) and the page size (for `/proc/self/statm`
/// resident pages). Both come from `getconf`, with the ubiquitous Linux
/// values as documented fallbacks.
struct SystemUnits {
    ticks_per_second: f64,
    page_size: u64,
}

impl SystemUnits {
    fn capture() -> Self {
        #[allow(clippy::cast_precision_loss)] // Display metric; exactness is not claimed.
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

/// Process CPU time in seconds: `/proc/self/stat` utime + stime (fields 14
/// and 15, in clock ticks) over the units' ticks-per-second.
#[allow(clippy::cast_precision_loss)] // Display metric; exactness is not claimed.
fn cpu_seconds(units: &SystemUnits) -> f64 {
    let stat = std::fs::read_to_string("/proc/self/stat").expect("stat");
    // The comm field (2) may contain spaces or ')'; fields resume after the
    // last ") ", so field 3 (state) is index 0 of the remainder.
    let remainder = stat.rsplit_once(") ").expect("stat comm terminator").1;
    let fields: Vec<&str> = remainder.split_whitespace().collect();
    let utime: u64 = fields[11].parse().expect("stat utime (field 14)");
    let stime: u64 = fields[12].parse().expect("stat stime (field 15)");
    (utime + stime) as f64 / units.ticks_per_second
}

/// Resident set size in bytes: `/proc/self/statm` resident pages (field 2)
/// times the system page size — the same quantity `/proc/self/status`
/// reports as `VmRSS` — sampled at call time (point-in-time RSS, not a peak).
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

#[derive(serde::Serialize)]
struct WorkloadResult {
    name: &'static str,
    query: Option<String>,
    offset: usize,
    limit: usize,
    reps: usize,
    /// Rows returned by the single untimed warmup call.
    warmup_rows: usize,
    wall_ms: f64,
    ops_per_sec: f64,
    cpu_ms: f64,
    cpu_us_per_op: f64,
    /// RSS after the workload; see `rss_bytes` for the exact definition.
    rss_bytes_after: u64,
    store_opens: u64,
    store_opens_per_op: f64,
}

async fn measure(
    controller: &SessionController,
    units: &SystemUnits,
    name: &'static str,
    query: Option<&str>,
    offset: usize,
    reps: usize,
) -> Result<WorkloadResult, String> {
    // One untimed warmup pass per workload.
    let warmup = controller
        .list_sessions(query, offset, SESSION_LIST_PAGE_LIMIT)
        .await
        .map_err(|error| format!("{error:?}"))?;
    let started = Instant::now();
    let cpu_before = cpu_seconds(units);
    let opens_before = conversation_store_open_count();
    for _ in 0..reps {
        let page = controller
            .list_sessions(query, offset, SESSION_LIST_PAGE_LIMIT)
            .await
            .map_err(|error| format!("{error:?}"))?;
        std::hint::black_box(&page);
    }
    let wall = started.elapsed();
    let cpu = cpu_seconds(units) - cpu_before;
    let opens = conversation_store_open_count() - opens_before;
    #[allow(clippy::cast_precision_loss)] // Display metrics; exactness is not claimed.
    let ops = reps as f64;
    #[allow(clippy::cast_precision_loss)] // Display metrics; exactness is not claimed.
    let opens_per_op = opens as f64 / ops;
    Ok(WorkloadResult {
        name,
        query: query.map(str::to_owned),
        offset,
        limit: SESSION_LIST_PAGE_LIMIT,
        reps,
        warmup_rows: warmup.sessions.len(),
        wall_ms: wall.as_secs_f64() * 1_000.0,
        ops_per_sec: ops / wall.as_secs_f64(),
        cpu_ms: cpu * 1_000.0,
        cpu_us_per_op: cpu * 1_000_000.0 / ops,
        rss_bytes_after: rss_bytes(units),
        store_opens: opens,
        store_opens_per_op: opens_per_op,
    })
}

#[tokio::main(flavor = "current_thread")]
async fn main() -> Result<(), String> {
    let params = parse_args()?;
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

    let ids = seed(&params).await?;
    seed_previews(&params.root, &ids).await;

    let units = SystemUnits::capture();
    // Real usage: the app server holds one controller; every repetition of
    // every workload reuses this one handle.
    let controller = SessionController::open(&params.root).map_err(|error| format!("{error:?}"))?;
    let middle = params.sessions / 2;
    let last = params.sessions.saturating_sub(SESSION_LIST_PAGE_LIMIT);
    let id_probe = ids[middle].as_str();
    let id_substring = &id_probe[id_probe.len() - 8..];
    let mut workloads = Vec::new();
    for (name, query, offset) in [
        ("first_page", None, 0),
        ("middle_page", None, middle),
        ("last_page", None, last),
        // The random suffix of one UUIDv7 identity: matches ~1 row.
        ("search_session_id", Some(id_substring), 0),
        // Every eighth session is named north-*: ~12.5% of rows (half of the
        // named quarter).
        ("search_name", Some("north-project"), 0),
        // Even sessions open with "alpha": a known ~50% of rows.
        ("search_preview", Some("alpha"), 0),
    ] {
        workloads.push(measure(&controller, &units, name, query, offset, params.reps).await?);
    }

    let report = serde_json::json!({
        "environment": {
            "profile": if cfg!(debug_assertions) { "debug" } else { "release" },
            "sessions": params.sessions,
            "messages_per_session": params.messages,
            "reps_per_workload": params.reps,
            "page_limit": SESSION_LIST_PAGE_LIMIT,
            "workload_path": "SessionController::list_sessions (the app server's path into SessionCatalog::list_page)",
            "cpu_source": "/proc/self/stat utime+stime (clock ticks) over getconf CLK_TCK, delta over the timed loop",
            "rss_source": "/proc/self/statm resident pages x getconf PAGE_SIZE (== VmRSS), sampled after each workload (point-in-time RSS)",
            "store_opens_source": "rustx::durable::conversation_store_open_count() delta over the timed loop",
        },
        "workloads": workloads,
    });
    let pretty = serde_json::to_string_pretty(&report).map_err(|error| error.to_string())?;
    println!("{pretty}");
    if let Some(path) = &params.json {
        std::fs::write(path, format!("{pretty}\n")).map_err(|error| error.to_string())?;
    }
    Ok(())
}

// Referenced so the benchmark's seeding stays honest about the row shape it
// searches: the workloads above never inspect rows, they measure the path.
#[allow(dead_code)]
fn row_preview(row: &SessionSummary) -> Option<&str> {
    row.preview.as_deref()
}
