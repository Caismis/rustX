//! The parent side of the FND-06 conformance harness.
//!
//! A [`Lab`] owns one temporary root that holds everything a real child
//! runtime needs — a model catalog, a runtime configuration, a model-visible
//! workspace with project instructions and a Skill, and a runtime-private
//! root containing the durable conversation database. The parent can edit
//! every one of those files while a child is running, which is exactly what
//! the FND-01 resource-authority cases require.
//!
//! [`Child`] is one spawned child process plus its control channel. The two
//! rendezvous a test uses are:
//!
//! ```text
//! wait_reached(boundary)  the child is parked inside a durable transition
//! wait_note(text)         the child announced a fact and is blocked reading
//! ```
//!
//! Both are exact happens-after proofs, so [`Child::sigkill`] always kills a
//! process that is provably not executing durable work.
//!
//! [`Durable`] is the post-mortem view: it reopens the same database the dead
//! child owned and exposes the durable authority plus real recovery.

use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::{UnixListener, UnixStream};
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Child as OsChild, Command, Stdio};
use std::sync::Arc;

use chrono::{DateTime, TimeZone, Utc};

use crate::durable::{
    ConversationStore, SqliteConversationStore, TRANSCRIPT_BOOTSTRAP_PAGE_LIMIT, TranscriptEntry,
};
use crate::events::types::{RuntimeEvent, RuntimeEventEnvelope};
use crate::local_runtime::session::{SessionCatalog, SessionId, SessionNode};
use crate::message::types::MessageBlock;
use crate::model::RequestSnapshot;
use crate::runtime::identity::ConversationId;
use crate::runtime::process_death::{CONTROL_ENV, GATE_ENV, GATE_NTH_ENV};
use crate::runtime::recovery::{RecoveryReport, recover};
use crate::runtime::types::RuntimeClock;

use super::{CHILD_TEST, ROOT_ENV, SCENARIO_ENV};

/// The outer liveness guard of one child interaction.
///
/// Nothing in this suite *proves* anything with a duration: every conformance
/// assertion is anchored to a durable boundary or a control rendezvous. This
/// bound exists only so a broken harness fails loudly instead of hanging a CI
/// worker.
pub(crate) const LIVENESS: std::time::Duration = std::time::Duration::from_mins(2);

/// The conversation lineage every child of this suite composes.
pub(crate) const CONVERSATION: &str = "conversation-fnd06";

/// The one fixture model reference of this suite.
pub(crate) const MODEL: &str = "fixture/fnd06";

/// A fixed clock, so a repeated recovery produces byte-identical facts.
#[derive(Debug, Clone, Copy)]
pub(crate) struct FixedClock;

impl RuntimeClock for FixedClock {
    fn now(&self) -> DateTime<Utc> {
        Utc.with_ymd_and_hms(2026, 8, 25, 12, 0, 0)
            .single()
            .expect("valid fixed time")
    }
}

/// The model catalog a child inherits. The bindings a child actually uses are
/// resolved from the in-process fixture registry; this file exists because the
/// composition contract requires an explicit catalog path.
fn models_json() -> String {
    serde_json::json!({
        "providers": {
            "fixture": {
                "baseUrl": "https://fixture.invalid/v1",
                "apiKey": "fixture-key-fixture",
                "models": [{
                    "id": "fnd06",
                    "protocol": "openai_chat_completions",
                    "contextWindow": 1_000_000,
                    "maxOutputTokens": 4096,
                    "capabilities": {
                        "inputModalities": ["text"],
                        "outputModalities": ["text"],
                        "toolCalls": true,
                        "reasoning": false
                    },
                    "compat": {"chatReasoningReplay": "omit"},
                    "requestParams": {}
                }]
            }
        }
    })
    .to_string()
}

/// The runtime configuration a child composes from.
fn runtime_json(read_approval: &str, include_todo: bool) -> String {
    let mut default_tools = vec!["read", "bash", "execution", "subagent"];
    if include_todo {
        default_tools.push("todo");
    }
    serde_json::json!({
        "schemaVersion": 5,
        "agentId": "agent-fnd06",
        "model": {"model": MODEL},
        "approvalMode": "policy",
        "context": {"reserveTokens": 0, "keepRecentTokens": 0},
        "nativeTools": {
            "read": {"approval": read_approval},
            "bash": {"execution": "model_selectable"}
        },
        "defaultTools": default_tools,
        // One named subagent definition (Issue #144). The instruction
        // document is a workspace resource the parent generation freezes;
        // the child never reads this configuration.
        "subagents": {
            "maxConcurrent": 4,
            "definitions": {
                "explore": {
                    "description": "Read-only exploration of the shared workspace.",
                    "instructionsFile": ".agents/subagents/explore/instructions.md",
                    "tools": {"builtin": ["read"]}
                }
            },
            "main": ["explore"],
            "workflow": []
        }
    })
    .to_string()
}

/// Rewrites a child lab configuration with the native Todo tool enabled.
/// Only the Issue #130 process-death scenarios need this extra model-facing
/// tool; the rest of FND-06 retains its original bounded catalog.
pub(crate) fn write_runtime_config_with_todo(root: &Path) {
    std::fs::write(root.join("rustx.jsonc"), runtime_json("never", true))
        .expect("rustx.jsonc with Todo");
}

/// One temporary lab: the complete on-disk world of one conformance case.
pub(crate) struct Lab {
    dir: tempfile::TempDir,
}

impl Lab {
    /// Creates the R1 world: project instructions, one discovered Skill, one
    /// ordinary workspace file, a catalog, and a runtime configuration.
    pub(crate) fn new() -> Self {
        let dir = tempfile::tempdir().expect("FND-06 lab directory");
        let lab = Self { dir };
        std::fs::create_dir_all(lab.workspace().join(".agents/skills/alpha"))
            .expect("skill directory");
        std::fs::create_dir_all(lab.workspace().join(".agents/subagents/explore"))
            .expect("subagent resources");
        std::fs::write(
            lab.workspace()
                .join(".agents/subagents/explore/instructions.md"),
            "You are a read-only exploration subagent. Answer the delegated task with the \
             capabilities your definition authorized.\n",
        )
        .expect("explore instructions");
        std::fs::create_dir_all(lab.root().join("private")).expect("runtime-private root");
        std::fs::write(lab.root().join("models.jsonc"), models_json()).expect("models.jsonc");
        lab.write_runtime_config("never");
        lab.write_project_instructions("R1 project instructions.");
        lab.write_skill_frontmatter("alpha", "R1 alpha summary");
        std::fs::write(lab.workspace().join("note.txt"), "R1 note body\n").expect("note.txt");
        lab
    }

    pub(crate) fn root(&self) -> &Path {
        self.dir.path()
    }

    pub(crate) fn workspace(&self) -> PathBuf {
        self.root().join("workspace")
    }

    /// The durable conversation database of every child of this lab.
    pub(crate) fn database(&self) -> PathBuf {
        self.root().join("private/artifacts/conversation.sqlite")
    }

    pub(crate) fn write_runtime_config(&self, read_approval: &str) {
        std::fs::write(
            self.root().join("rustx.jsonc"),
            runtime_json(read_approval, false),
        )
        .expect("rustx.jsonc");
    }

    /// Replaces the loaded `AGENTS.md` project-instruction file.
    pub(crate) fn write_project_instructions(&self, body: &str) {
        std::fs::write(self.workspace().join("AGENTS.md"), format!("{body}\n")).expect("AGENTS.md");
    }

    /// Rewrites one Skill's frontmatter metadata *and* its body.
    pub(crate) fn write_skill_frontmatter(&self, name: &str, description: &str) {
        self.write_skill(name, description, &format!("{description} body"));
    }

    /// Writes one Skill package with an explicit description and body.
    pub(crate) fn write_skill(&self, name: &str, description: &str, body: &str) {
        let dir = self.workspace().join(".agents/skills").join(name);
        std::fs::create_dir_all(&dir).expect("skill directory");
        std::fs::write(
            dir.join("SKILL.md"),
            format!("---\nname: {name}\ndescription: {description}\n---\n\n{body}\n"),
        )
        .expect("SKILL.md");
    }

    /// Deletes one already-discovered Skill package from disk.
    pub(crate) fn remove_skill(&self, name: &str) {
        std::fs::remove_dir_all(self.workspace().join(".agents/skills").join(name))
            .expect("remove skill package");
    }

    /// Spawns one child process running `scenario`, optionally armed to freeze
    /// at `gate`.
    pub(crate) fn spawn(&self, scenario: &str, gate: Option<&str>) -> Child {
        self.spawn_nth(scenario, gate, 1)
    }

    /// Spawns one child armed to freeze at the `nth` occurrence of `gate`.
    pub(crate) fn spawn_nth(&self, scenario: &str, gate: Option<&str>, nth: usize) -> Child {
        let socket = self.root().join(format!("control-{scenario}-{nth}.sock"));
        let _ = std::fs::remove_file(&socket);
        let listener = UnixListener::bind(&socket).expect("FND-06 control listener");
        let mut command = Command::new(std::env::current_exe().expect("test binary"));
        command
            .args([
                "--exact",
                CHILD_TEST,
                "--quiet",
                "--nocapture",
                "--test-threads",
                "1",
            ])
            .env(SCENARIO_ENV, scenario)
            .env(ROOT_ENV, self.root())
            .env(CONTROL_ENV, &socket)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::piped())
            // The child owns its own process group, so the harness can end the
            // child *and* every OS process it started with one deterministic
            // `killpg`, leaving no orphan behind.
            .process_group(0);
        if let Some(gate) = gate {
            command
                .env(GATE_ENV, gate)
                .env(GATE_NTH_ENV, nth.to_string());
        }
        let mut process = command.spawn().expect("spawn the FND-06 child");
        let stderr = process.stderr.take();
        let control = match accept_within(&listener, &mut process, LIVENESS) {
            Ok(control) => control,
            Err(reason) => {
                let _ = process.kill();
                let _ = process.wait();
                panic!("{reason}\n--- child stderr ---\n{}", drain(stderr));
            }
        };
        control
            .set_read_timeout(Some(LIVENESS))
            .expect("control read timeout");
        let writer = control.try_clone().expect("control writer");
        Child {
            pid: process.id(),
            process: Some(process),
            stderr,
            reader: BufReader::new(control),
            writer,
        }
    }

    /// Reopens the durable authority the dead child owned.
    pub(crate) fn durable(&self) -> Durable {
        Self::durable_at(&ConversationId::new(CONVERSATION), &self.database())
    }

    /// Reopens one explicitly named durable conversation.
    pub(crate) fn durable_at(conversation: &ConversationId, path: &Path) -> Durable {
        let store = SqliteConversationStore::open(conversation.clone(), path)
            .expect("reopen the durable conversation");
        Durable {
            store: Arc::new(store),
        }
    }

    /// The durable native Session catalog the dead child published.
    ///
    /// This is the product's own reader over the product's own file, so the
    /// parent reads the same authority the next process would: no harness
    /// re-implementation of the catalog format sits in between.
    pub(crate) fn catalog(&self) -> SessionCatalog {
        SessionCatalog::open_existing(&self.root().join("private"))
            .expect("read the native Session catalog")
            .expect("a native Session catalog was published")
    }

    /// The durable authority of one catalog lineage.
    pub(crate) fn lineage(&self, session: &SessionId, conversation: &ConversationId) -> Durable {
        let path = self.catalog().database_path(session, conversation);
        Self::durable_at(conversation, &path)
    }

    /// Whether any Session or node in the catalog names `conversation`.
    ///
    /// An uncommitted destination has a complete database on disk, so "does
    /// the file exist" proves nothing. The catalog is the only authority for
    /// whether a lineage is reachable at all.
    pub(crate) fn catalog_names(&self, conversation: &ConversationId) -> bool {
        self.catalog().names_conversation(conversation)
    }

    /// The catalog's currently active `(Session, node)` pair.
    pub(crate) fn active_lineage(&self) -> (SessionId, SessionNode) {
        let (session_id, node, _) = self
            .catalog()
            .active_lineage()
            .expect("the catalog names an active lineage");
        (session_id, node)
    }
}

/// Accepts the child's single control connection within the liveness bound.
///
/// The loop is a *liveness* guard, never a correctness proof: the conformance
/// rendezvous are the control lines this connection then carries. A child that
/// exited before connecting is reported immediately with its own diagnostics
/// instead of waiting out the bound.
fn accept_within(
    listener: &UnixListener,
    process: &mut OsChild,
    bound: std::time::Duration,
) -> Result<UnixStream, String> {
    listener
        .set_nonblocking(true)
        .expect("non-blocking control listener");
    let deadline = std::time::Instant::now() + bound;
    loop {
        match listener.accept() {
            Ok((stream, _)) => {
                stream
                    .set_nonblocking(false)
                    .expect("blocking control stream");
                return Ok(stream);
            }
            Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                if let Ok(Some(status)) = process.try_wait() {
                    return Err(format!(
                        "the FND-06 child exited with {status:?} before it connected"
                    ));
                }
                if std::time::Instant::now() >= deadline {
                    return Err("the FND-06 child never connected to its control socket".to_owned());
                }
                std::thread::sleep(std::time::Duration::from_millis(1));
            }
            Err(error) => return Err(format!("FND-06 control accept failed: {error}")),
        }
    }
}

/// Reads whatever the dead child wrote to its standard error.
fn drain(stderr: Option<std::process::ChildStderr>) -> String {
    let Some(mut stderr) = stderr else {
        return String::new();
    };
    let mut buffer = String::new();
    let _ = std::io::Read::read_to_string(&mut stderr, &mut buffer);
    buffer
}

/// One control line from a child.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum ChildLine {
    /// The child is parked inside this durable boundary.
    Reached(String),
    /// The child announced a fact and is now blocked reading its next command.
    Note(String),
}

/// One spawned child process and its control channel.
pub(crate) struct Child {
    pid: u32,
    process: Option<OsChild>,
    stderr: Option<std::process::ChildStderr>,
    reader: BufReader<UnixStream>,
    writer: UnixStream,
}

impl Child {
    /// Reads the next control line.
    fn next_line(&mut self) -> ChildLine {
        let mut line = String::new();
        let read = match self.reader.read_line(&mut line) {
            Ok(read) => read,
            Err(error) => {
                let stderr = self.take_stderr();
                panic!(
                    "the FND-06 child control channel failed: {error}\n--- child stderr ---\n{stderr}"
                );
            }
        };
        if read == 0 {
            let stderr = self.take_stderr();
            panic!(
                "the FND-06 child exited before it announced anything\n--- child stderr ---\n{stderr}"
            );
        }
        let value: serde_json::Value =
            serde_json::from_str(line.trim_end()).expect("a JSON control line");
        match value["kind"].as_str() {
            Some("reached") => ChildLine::Reached(
                value["boundary"]
                    .as_str()
                    .expect("a boundary name")
                    .to_owned(),
            ),
            Some("note") => {
                ChildLine::Note(value["text"].as_str().expect("a note text").to_owned())
            }
            other => panic!("unexpected FND-06 control line kind {other:?}"),
        }
    }

    /// Blocks until the child parks inside `boundary`.
    pub(crate) fn wait_reached(&mut self, boundary: &str) {
        loop {
            match self.next_line() {
                ChildLine::Reached(reached) => {
                    assert_eq!(reached, boundary, "the child parked at another boundary");
                    return;
                }
                ChildLine::Note(_) => {}
            }
        }
    }

    /// Blocks until the child announces `note` and blocks reading its next
    /// command.
    pub(crate) fn wait_note(&mut self, note: &str) {
        loop {
            match self.next_line() {
                ChildLine::Note(text) if text == note => return,
                ChildLine::Note(_) => {}
                ChildLine::Reached(boundary) => {
                    panic!("the child parked at {boundary} while {note} was expected")
                }
            }
        }
    }

    /// Reads control lines, releasing every rendezvous, until the child
    /// announces `note`.
    pub(crate) fn resume_until(&mut self, note: &str) {
        loop {
            match self.next_line() {
                ChildLine::Note(text) if text == note => return,
                ChildLine::Note(_) => self.resume(),
                ChildLine::Reached(boundary) => {
                    panic!("the child parked at {boundary} while {note} was expected")
                }
            }
        }
    }

    /// Blocks until the child announces a note starting with `prefix` and
    /// returns the whole note.
    pub(crate) fn wait_note_prefixed(&mut self, prefix: &str) -> String {
        loop {
            match self.next_line() {
                ChildLine::Note(text) if text.starts_with(prefix) => return text,
                ChildLine::Note(_) => {}
                ChildLine::Reached(boundary) => {
                    panic!("the child parked at {boundary} while {prefix}… was expected")
                }
            }
        }
    }

    /// Releases a child blocked in a control rendezvous.
    pub(crate) fn resume(&mut self) {
        writeln!(self.writer, "{{\"kind\":\"go\"}}").expect("resume the FND-06 child");
        self.writer.flush().expect("flush the resume command");
    }

    /// Kills the child and returns everything it wrote to standard error.
    fn take_stderr(&mut self) -> String {
        if let Some(mut process) = self.process.take() {
            let _ = process.kill();
            let _ = process.wait();
        }
        drain(self.stderr.take())
    }

    /// Kills the child's whole process group and reaps it.
    ///
    /// This is the process-death boundary itself. `SIGKILL` is uncatchable, so
    /// the child runs no shutdown path, flushes nothing, and settles nothing:
    /// whatever the durable authority holds afterwards was committed before
    /// the signal.
    pub(crate) fn sigkill(&mut self) {
        let Some(mut process) = self.process.take() else {
            return;
        };
        let group =
            nix::unistd::Pid::from_raw(i32::try_from(self.pid).expect("a representable child pid"));
        let _ = nix::sys::signal::killpg(group, nix::sys::signal::Signal::SIGKILL);
        let status = process.wait().expect("reap the FND-06 child");
        assert!(
            status.code().is_none(),
            "the FND-06 child exited on its own with {status:?} instead of being killed"
        );
    }
}

impl Drop for Child {
    fn drop(&mut self) {
        // A failing assertion must still leave no child process and no orphan
        // behind: the whole group goes, unconditionally.
        if let Some(mut process) = self.process.take() {
            let group = nix::unistd::Pid::from_raw(i32::try_from(self.pid).unwrap_or(-1));
            let _ = nix::sys::signal::killpg(group, nix::sys::signal::Signal::SIGKILL);
            let _ = process.wait();
        }
    }
}

/// The durable authority of a dead child, reopened by its parent.
pub(crate) struct Durable {
    store: Arc<SqliteConversationStore>,
}

impl Durable {
    pub(crate) fn store(&self) -> &dyn ConversationStore {
        self.store.as_ref()
    }

    /// The complete Event Journal in durable sequence order.
    pub(crate) fn journal(&self) -> Vec<RuntimeEventEnvelope> {
        const PAGE: usize = 64;
        let mut cursor = None;
        let mut events = Vec::new();
        loop {
            let page = self
                .store
                .read_events(cursor, PAGE)
                .expect("Event Journal page");
            if page.events.is_empty() {
                break;
            }
            events.extend(page.events);
            cursor = page.next_sequence;
        }
        events
    }

    /// The durable sequence of the first event matching `predicate`.
    pub(crate) fn sequence_of(&self, predicate: impl Fn(&RuntimeEvent) -> bool) -> Option<u64> {
        self.journal()
            .into_iter()
            .find(|envelope| predicate(&envelope.event))
            .map(|envelope| envelope.sequence)
    }

    /// Whether the journal holds at least one event matching `predicate`.
    pub(crate) fn has_event(&self, predicate: impl Fn(&RuntimeEvent) -> bool) -> bool {
        self.journal()
            .iter()
            .any(|envelope| predicate(&envelope.event))
    }

    /// How many journal events match `predicate`.
    pub(crate) fn count_events(&self, predicate: impl Fn(&RuntimeEvent) -> bool) -> usize {
        self.journal()
            .iter()
            .filter(|envelope| predicate(&envelope.event))
            .count()
    }

    /// The canonical Message Ledger.
    pub(crate) fn canonical(&self) -> Vec<MessageBlock> {
        self.store.load_canonical().expect("canonical ledger")
    }

    /// The current model-visible Conversation Surface.
    pub(crate) fn surface(&self) -> Vec<MessageBlock> {
        let head = self.store.load_head().expect("durable head");
        self.store
            .load_messages(&head.active_message_ids)
            .expect("surface messages")
    }

    /// Every persisted request snapshot, oldest first.
    pub(crate) fn request_snapshots(&self) -> Vec<RequestSnapshot> {
        let mut cursor = None;
        let mut snapshots = Vec::new();
        loop {
            let page = self
                .store
                .read_request_snapshots(cursor, 32)
                .expect("request snapshot page");
            if page.snapshots.is_empty() {
                break;
            }
            snapshots.extend(page.snapshots);
            cursor = page.next_sequence;
        }
        snapshots
    }

    /// The bootstrap transcript page of the reopened conversation.
    pub(crate) fn transcript(&self) -> Vec<TranscriptEntry> {
        self.store
            .load_transcript_page(None, TRANSCRIPT_BOOTSTRAP_PAGE_LIMIT)
            .expect("transcript page")
            .entries
    }

    /// The settlement kinds of every publication stream that is still
    /// unsettled at reopen.
    pub(crate) fn unsettled_publications(
        &self,
    ) -> Vec<crate::publication::PublicationStreamRecord> {
        self.store
            .load_unsettled_publication_streams()
            .expect("unsettled publication streams")
    }

    /// Runs the real startup recovery pipeline over this durable authority.
    pub(crate) fn recover(&self) -> RecoveryReport {
        recover(self.store.as_ref(), &FixedClock).expect("recovery succeeds")
    }
}
