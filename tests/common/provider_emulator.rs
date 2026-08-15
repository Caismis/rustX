//! The Rust launcher for the external scripted provider emulator.
//!
//! This module owns **process mechanics only**: command construction,
//! readiness parsing, the base URL, the control API, diagnostics capture,
//! and child cleanup. It contains no scenario semantics and no provider
//! protocol — those live in `test-support/fake-provider/`, and duplicating
//! either here would recreate the second provider implementation issue #47
//! exists to remove.
//!
//! ```text
//! Rust conformance test
//!   -> ProviderEmulator::start("scenario")   uv run fake-provider --port 0
//!   -> LocalConversationRuntime::compose     catalog baseUrl = emulator
//!   -> real adapter -> real HTTP/SSE ------> the Python process
//!   -> await_gate / release_gate / requests  the control API
//!   -> finish()                              assert the scenario report
//! ```

#![allow(dead_code)] // each helper is used by some conformance tests

use std::path::PathBuf;
use std::process::Stdio;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::process::{Child, Command};

/// The environment variable that turns "uv is missing, skip" into a hard
/// failure. CI sets it, so a broken toolchain in the pipeline can never be
/// reported as a green conformance run.
pub const REQUIRE_VARIABLE: &str = "RUSTX_REQUIRE_PROVIDER_EMULATOR";

/// The upper bound on a control-plane barrier wait. Deadlock protection
/// only: ordering is always established by the observation itself.
const AWAIT_TIMEOUT: Duration = Duration::from_secs(20);

/// A running scenario in the external provider process.
pub struct ProviderEmulator {
    child: Option<Child>,
    client: reqwest::Client,
    host: String,
    port: u16,
    scenario: String,
    stderr: Arc<Mutex<String>>,
}

impl ProviderEmulator {
    /// Starts one scenario on an ephemeral loopback port.
    ///
    /// Returns `None` when `uv` is not installed and the harness is not
    /// required, so a checkout without the Python toolchain still runs the
    /// rest of the suite.
    ///
    /// # Panics
    ///
    /// Panics when the process cannot be launched, does not print its
    /// readiness record, or reports a scenario the caller did not ask for.
    pub async fn start(scenario: &str) -> Option<Self> {
        let required = std::env::var_os(REQUIRE_VARIABLE).is_some();
        if which("uv").is_none() {
            assert!(
                !required,
                "{REQUIRE_VARIABLE} is set but uv is not on PATH; the provider \
                 emulator cannot run"
            );
            eprintln!(
                "uv unavailable; the issue 47 provider-emulator conformance scenario \
                 {scenario} was not exercised"
            );
            return None;
        }

        let project = project_root();
        let mut child = Command::new("uv")
            .arg("run")
            .arg("--project")
            .arg(&project)
            .arg("--frozen")
            .arg("fake-provider")
            .arg("--scenario")
            .arg(scenario)
            .arg("--port")
            .arg("0")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            // Belt and braces beside the stdin-EOF contract: a panicking
            // test never leaves the provider running.
            .kill_on_drop(true)
            .spawn()
            .expect("spawn the provider emulator");

        let stderr = Arc::new(Mutex::new(String::new()));
        let sink = Arc::clone(&stderr);
        let mut diagnostics = BufReader::new(child.stderr.take().expect("stderr pipe")).lines();
        tokio::spawn(async move {
            while let Ok(Some(line)) = diagnostics.next_line().await {
                sink.lock().expect("stderr lock").push_str(&line);
                sink.lock().expect("stderr lock").push('\n');
            }
        });

        let mut stdout = BufReader::new(child.stdout.take().expect("stdout pipe"));
        let mut line = String::new();
        let read = tokio::time::timeout(AWAIT_TIMEOUT, stdout.read_line(&mut line))
            .await
            .expect("the provider emulator printed its readiness record in time")
            .expect("read the readiness record");
        assert!(
            read > 0,
            "the provider emulator exited before readiness: {}",
            stderr.lock().expect("stderr lock")
        );
        let ready: serde_json::Value =
            serde_json::from_str(&line).expect("the readiness record is one JSON object");
        assert_eq!(ready["ready"], serde_json::json!(true), "{ready}");
        assert_eq!(ready["scenario"], serde_json::json!(scenario), "{ready}");

        // The remaining stdout (the final report) is drained so the child
        // never blocks on a full pipe.
        let report_sink = Arc::clone(&stderr);
        tokio::spawn(async move {
            let mut rest = String::new();
            let _ = stdout.read_line(&mut rest).await;
            if !rest.trim().is_empty() {
                let mut sink = report_sink.lock().expect("stderr lock");
                sink.push_str("report: ");
                sink.push_str(&rest);
            }
        });

        Some(Self {
            child: Some(child),
            client: reqwest::Client::new(),
            host: ready["host"].as_str().expect("host").to_owned(),
            port: u16::try_from(ready["port"].as_u64().expect("port")).expect("port range"),
            scenario: scenario.to_owned(),
            stderr,
        })
    }

    /// The provider root, which is the Anthropic Messages `baseUrl`.
    #[must_use]
    pub fn base_url(&self) -> String {
        format!("http://{}:{}", self.host, self.port)
    }

    /// The OpenAI-family `baseUrl` (`.../v1`).
    #[must_use]
    pub fn openai_base_url(&self) -> String {
        format!("{}/v1", self.base_url())
    }

    /// Blocks until the provider reaches a named gate.
    ///
    /// This is a real barrier inside the provider process, not a poll: it
    /// returns only once the gate has been reached, so an action performed
    /// afterwards is provably ordered after everything the gate follows.
    ///
    /// # Panics
    ///
    /// Panics when the gate is never reached within the deadlock timeout.
    pub async fn await_gate(&self, name: &str) -> serde_json::Value {
        self.await_observation("gate_reached", Some(name)).await
    }

    /// Blocks until the provider observes the client closing the connection.
    ///
    /// # Panics
    ///
    /// Panics when no disconnect is observed within the deadlock timeout.
    pub async fn await_client_disconnect(&self) -> serde_json::Value {
        self.await_observation("client_disconnected", None).await
    }

    async fn await_observation(&self, kind: &str, name: Option<&str>) -> serde_json::Value {
        let url = format!(
            "{}/__control/observations/await?kind={kind}&timeoutMs={}{}",
            self.base_url(),
            AWAIT_TIMEOUT.as_millis(),
            name.map(|name| format!("&name={name}")).unwrap_or_default(),
        );
        let response = self.client.get(&url).send().await.expect("control request");
        let status = response.status();
        let body: serde_json::Value = response.json().await.expect("control JSON");
        assert!(
            status.is_success(),
            "the provider never reached {kind}{}: {body}\n{}",
            name.map(|name| format!(" ({name})")).unwrap_or_default(),
            self.diagnostics()
        );
        body
    }

    /// Releases a named gate, letting the suspended response continue.
    ///
    /// # Panics
    ///
    /// Panics when the scenario declares no such gate.
    pub async fn release_gate(&self, name: &str) {
        let response = self
            .client
            .post(format!(
                "{}/__control/gates/{name}/release",
                self.base_url()
            ))
            .send()
            .await
            .expect("control request");
        assert!(
            response.status().is_success(),
            "releasing the gate {name} failed: {}",
            response.text().await.unwrap_or_default()
        );
    }

    /// Every provider request the runtime actually sent, in arrival order.
    ///
    /// # Panics
    ///
    /// Panics when the control API is unreachable.
    pub async fn requests(&self) -> Vec<serde_json::Value> {
        let body: serde_json::Value = self
            .client
            .get(format!("{}/__control/requests", self.base_url()))
            .send()
            .await
            .expect("control request")
            .json()
            .await
            .expect("control JSON");
        body["requests"]
            .as_array()
            .expect("the request record is an array")
            .clone()
    }

    /// The captured provider diagnostics.
    #[must_use]
    pub fn diagnostics(&self) -> String {
        self.stderr.lock().expect("stderr lock").clone()
    }

    /// Shuts the provider down and asserts the scenario was satisfied:
    /// every declared step consumed, in order, with no unexpected request.
    ///
    /// # Panics
    ///
    /// Panics when the scenario report is not `ok`, quoting the exact
    /// provider-side failures.
    pub async fn finish(mut self) {
        let report: serde_json::Value = self
            .client
            .post(format!("{}/__control/shutdown", self.base_url()))
            .send()
            .await
            .expect("control request")
            .json()
            .await
            .expect("control JSON");
        let child = self.child.take().expect("the child is taken once");
        let status = tokio::time::timeout(AWAIT_TIMEOUT, child.wait_with_output())
            .await
            .expect("the provider emulator exited in time")
            .expect("wait for the provider emulator");
        assert!(
            report["ok"].as_bool().unwrap_or(false),
            "the {} scenario was not satisfied: {}\n{}",
            self.scenario,
            serde_json::to_string_pretty(&report).unwrap_or_default(),
            self.diagnostics()
        );
        assert!(
            status.status.success(),
            "the provider emulator exited with {:?}\n{}",
            status.status,
            self.diagnostics()
        );
    }
}

impl Drop for ProviderEmulator {
    fn drop(&mut self) {
        if let Some(child) = self.child.as_mut() {
            // `kill_on_drop` handles the async runtime path; this covers a
            // drop that happens outside it (a panicking synchronous unwind).
            let _ = child.start_kill();
        }
    }
}

/// The uv project root of the emulator.
fn project_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("test-support/fake-provider")
}

/// The first `name` on `PATH`.
fn which(name: &str) -> Option<PathBuf> {
    std::env::var_os("PATH").and_then(|path| {
        std::env::split_paths(&path)
            .map(|dir| dir.join(name))
            .find(|candidate| candidate.is_file())
    })
}
