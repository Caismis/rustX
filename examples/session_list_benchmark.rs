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
    SESSION_LIST_PAGE_LIMIT, SessionId, SessionListPage, SessionPersistentState,
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

/// The display name `seed` gives Session `index`.
fn seeded_name(index: usize) -> Option<String> {
    if !index.is_multiple_of(4) {
        return None;
    }
    let region = if index.is_multiple_of(8) {
        "north"
    } else {
        "south"
    };
    Some(format!("{region}-project-{index}"))
}

/// What one seeded Session must be listed as, known from seeding alone.
struct SeededRow {
    id: SessionId,
    name: Option<String>,
    preview: String,
}

fn seeded_rows(ids: &[SessionId]) -> Vec<SeededRow> {
    ids.iter()
        .enumerate()
        .map(|(index, id)| SeededRow {
            id: id.clone(),
            name: seeded_name(index),
            preview: expected_preview(&subject_text(index)),
        })
        .collect()
}

/// The documented one-line preview: whitespace runs collapse to one space,
/// and a line past 120 characters keeps 119 and marks the cut with `…`.
fn expected_preview(subject: &str) -> String {
    let line = subject.split_whitespace().collect::<Vec<_>>().join(" ");
    if line.chars().count() <= 120 {
        return line;
    }
    let kept = line.chars().take(119).collect::<String>();
    format!("{}\u{2026}", kept.trim_end())
}

/// The page the Session list contract prescribes for the seeded rows: creation
/// order, a trimmed case-insensitive substring match on identity, name or
/// preview, then `offset` and `limit` over the matching rows.
fn expected_page<'a>(
    rows: &'a [SeededRow],
    query: Option<&str>,
    offset: usize,
    limit: usize,
) -> (Vec<&'a SeededRow>, Option<usize>) {
    let query = query.map(|value| value.trim().to_lowercase());
    let matching = rows
        .iter()
        .filter(|row| {
            query.as_ref().is_none_or(|query| {
                row.id.as_str().to_lowercase().contains(query)
                    || row
                        .name
                        .as_ref()
                        .is_some_and(|name| name.to_lowercase().contains(query))
                    || row.preview.to_lowercase().contains(query)
            })
        })
        .collect::<Vec<_>>();
    let page = matching
        .iter()
        .skip(offset)
        .take(limit)
        .copied()
        .collect::<Vec<_>>();
    let end = offset + page.len();
    (page, (end < matching.len()).then_some(end))
}

/// Rejects any page that differs from the expectation in its rows, their
/// order, their displayed facts, or its continuation.
fn check_page(
    expected: &(Vec<&SeededRow>, Option<usize>),
    actual: &SessionListPage,
) -> Result<(), String> {
    let (rows, next_offset) = expected;
    if actual.sessions.len() != rows.len() {
        return Err(format!(
            "expected {} rows, got {}",
            rows.len(),
            actual.sessions.len()
        ));
    }
    for (position, (row, summary)) in rows.iter().zip(&actual.sessions).enumerate() {
        if summary.id != row.id
            || summary.name != row.name
            || summary.preview.as_deref() != Some(row.preview.as_str())
        {
            return Err(format!(
                "row {position}: expected {} {:?} {:?}, got {} {:?} {:?}",
                row.id, row.name, row.preview, summary.id, summary.name, summary.preview
            ));
        }
    }
    if actual.next_offset != *next_offset {
        return Err(format!(
            "expected next offset {next_offset:?}, got {:?}",
            actual.next_offset
        ));
    }
    Ok(())
}

/// Seeds `params.sessions` Sessions through the real owners: the controller's
/// own session creation, one canonical history per root conversation appended
/// through the durable store with production durability, and a name on every
/// fourth Session (half `north-*`, half `south-*`).
async fn seed(params: &Params) -> Result<Vec<SessionId>, String> {
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
        if let Some(name) = seeded_name(index) {
            controller
                .rename_session(&session.id, &name)
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
    /// Rows returned by the single untimed warmup call, checked against the
    /// seeded fixture before timing starts.
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
    rows: &[SeededRow],
    name: &'static str,
    query: Option<&str>,
    offset: usize,
    reps: usize,
) -> Result<WorkloadResult, String> {
    let expected = expected_page(rows, query, offset, SESSION_LIST_PAGE_LIMIT);
    let invalid = |error: String| format!("workload {name} returned a wrong page: {error}");
    // One untimed warmup pass per workload; a wrong result is never timed.
    let warmup = controller
        .list_sessions(query, offset, SESSION_LIST_PAGE_LIMIT)
        .await
        .map_err(|error| format!("{error:?}"))?;
    check_page(&expected, &warmup).map_err(invalid)?;
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
    let rows = seeded_rows(&ids);
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
        workloads
            .push(measure(&controller, &units, &rows, name, query, offset, params.reps).await?);
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

/// The workload checker must refuse wrong pages, so a broken list path can
/// never be reported as a fast successful workload.
#[cfg(test)]
mod tests {
    use super::*;
    use rustx::local_runtime::session::{SessionNodeId, SessionSummary};

    const ID_PREFIX: &str = "ses_00000000-0000-7000-8000-";

    /// Canonical UUIDv7 identities whose last group spells the row index.
    fn rows(count: usize) -> Vec<SeededRow> {
        let ids = (0..count)
            .map(|index| SessionId::new(format!("{ID_PREFIX}{index:012}")))
            .collect::<Vec<_>>();
        seeded_rows(&ids)
    }

    fn indices(page: &[&SeededRow]) -> Vec<usize> {
        page.iter()
            .map(|row| row.id.as_str()[ID_PREFIX.len()..].parse().unwrap())
            .collect()
    }

    fn listed(rows: &[&SeededRow], next_offset: Option<usize>) -> SessionListPage {
        SessionListPage {
            sessions: rows
                .iter()
                .map(|row| SessionSummary {
                    ownership_generation: "1".into(),
                    cwd: PathBuf::from("/workspace"),
                    id: row.id.clone(),
                    name: row.name.clone(),
                    preview: Some(row.preview.clone()),
                    updated_at: chrono::DateTime::UNIX_EPOCH,
                    active_node: SessionNodeId::new("node_00000000-0000-7000-8000-000000000000"),
                })
                .collect(),
            next_offset,
        }
    }

    #[test]
    fn expected_pages_follow_offsets_limits_and_search_fields() {
        let rows = rows(10);
        for (query, offset, page, next) in [
            (None, 0, vec![0, 1, 2, 3], Some(4)),
            (None, 6, vec![6, 7, 8, 9], None),
            (None, 8, vec![8, 9], None),
            (None, 10, vec![], None),
            (Some("  NORTH-project "), 0, vec![0, 8], None),
            (Some("south-project"), 0, vec![4], None),
            (Some("alpha"), 0, vec![0, 2, 4, 6], Some(4)),
            (Some("alpha"), 4, vec![8], None),
            (Some("-000000000003"), 0, vec![3], None),
            (Some("8000-00000000000"), 4, vec![4, 5, 6, 7], Some(8)),
            (Some("absent"), 0, vec![], None),
        ] {
            let expected = expected_page(&rows, query, offset, 4);
            assert_eq!(indices(&expected.0), page, "{query:?} at {offset}");
            assert_eq!(expected.1, next, "{query:?} at {offset}");
        }
        assert_eq!(rows[1].preview, "beta topic 1: short brief");
        assert!(
            rows[0]
                .preview
                .starts_with("alpha topic 0: a very long discussion")
        );
        assert!(rows[0].preview.ends_with('\u{2026}'));
        assert!(rows[0].preview.chars().count() <= 120);
    }

    #[test]
    fn check_page_accepts_the_exact_page_and_rejects_every_other() {
        let rows = rows(10);
        let expected = expected_page(&rows, Some("alpha"), 0, 4);
        check_page(&expected, &listed(&expected.0, expected.1)).unwrap();
        let empty = expected_page(&rows, Some("absent"), 0, 4);
        check_page(&empty, &listed(&[], None)).unwrap();

        assert!(check_page(&expected, &listed(&[], None)).is_err());
        let same_count = [&rows[1], &rows[3], &rows[5], &rows[7]];
        assert!(check_page(&expected, &listed(&same_count, expected.1)).is_err());
        let mut reordered = expected.0.clone();
        reordered.swap(0, 1);
        assert!(check_page(&expected, &listed(&reordered, expected.1)).is_err());
        let mut page = listed(&expected.0, expected.1);
        page.sessions[2].preview = None;
        assert!(check_page(&expected, &page).is_err());
        let mut page = listed(&expected.0, expected.1);
        page.sessions[0].name = None;
        assert!(check_page(&expected, &page).is_err());
        assert!(check_page(&expected, &listed(&expected.0, None)).is_err());
    }

    #[test]
    fn an_invalid_expectation_is_rejected_against_a_correct_page() {
        let rows = rows(10);
        let correct = expected_page(&rows, None, 0, 4);
        let page = listed(&correct.0, correct.1);
        for wrong in [
            expected_page(&rows, None, 1, 4),
            expected_page(&rows, None, 0, 3),
            expected_page(&rows, Some("beta"), 0, 4),
            expected_page(&rows[..4], None, 0, 4),
        ] {
            assert!(check_page(&wrong, &page).is_err());
        }
    }
}
