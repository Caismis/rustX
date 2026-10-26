//! Counted standard Resource operations shared by real stdio and HTTP tests.

use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use base64::Engine as _;
use rmcp::RoleServer;
use rmcp::model::{
    ListResourceTemplatesResult, ListResourcesResult, PaginatedRequestParams,
    ReadResourceRequestParams, ReadResourceResult, Resource, ResourceContents, ResourceTemplate,
};
use rmcp::service::RequestContext;

/// JSONL file recording every actual Resource request in self-spawned tests.
pub const RESOURCE_OBSERVATION_FILE_ENV: &str = "RUSTX_FIXTURE_RESOURCE_OBSERVATIONS";

/// Independent request counters and a channel-based withheld read gate.
#[derive(Clone, Default)]
pub struct ResourceFixtureState {
    /// Count resources/list requests reaching the peer.
    pub lists: Arc<AtomicUsize>,
    /// Count resources/templates/list requests reaching the peer.
    pub templates: Arc<AtomicUsize>,
    /// Count resources/read requests reaching the peer.
    pub reads: Arc<AtomicUsize>,
    /// Signals entry into a withheld read.
    pub read_entered: Arc<tokio::sync::Notify>,
    /// Releases a withheld read to produce a correlated response.
    pub read_release: Arc<tokio::sync::Notify>,
    /// Signals cancellation of the server-side request context.
    pub read_cancelled: Arc<tokio::sync::Notify>,
    /// Optional cross-process request observation file.
    pub observation_file: Option<PathBuf>,
    /// Optional correlated error for redaction and aggregation tests.
    pub error: Option<rmcp::ErrorData>,
}

impl ResourceFixtureState {
    /// Builds the fixture's cross-process observation seam.
    #[must_use]
    pub fn from_env() -> Self {
        Self {
            observation_file: std::env::var_os(RESOURCE_OBSERVATION_FILE_ENV).map(PathBuf::from),
            ..Self::default()
        }
    }

    pub(super) fn record(&self, method: &str, value: Option<&str>) -> Result<(), rmcp::ErrorData> {
        use std::io::Write as _;
        if let Some(path) = &self.observation_file {
            let mut file = std::fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open(path)
                .map_err(|error| rmcp::ErrorData::internal_error(error.to_string(), None))?;
            writeln!(
                file,
                "{}",
                serde_json::json!({"method": method, "value": value})
            )
            .map_err(|error| rmcp::ErrorData::internal_error(error.to_string(), None))?;
        }
        Ok(())
    }

    pub(super) fn list(
        &self,
        request: Option<PaginatedRequestParams>,
        empty: bool,
    ) -> Result<ListResourcesResult, rmcp::ErrorData> {
        self.lists.fetch_add(1, Ordering::SeqCst);
        let cursor = request.and_then(|request| request.cursor);
        self.record("resources/list", cursor.as_deref())?;
        if let Some(error) = &self.error {
            return Err(error.clone());
        }
        let mut result = ListResourcesResult::default();
        if empty {
            return Ok(result);
        }
        match cursor.as_deref() {
            None => {
                result.resources =
                    vec![Resource::new("fixture://text", "text").with_mime_type("text/plain")];
                result.next_cursor = Some("page-2".into());
            }
            Some("page-2") => result.resources = vec![Resource::new("fixture://blob", "blob")],
            Some("cycle") => result.next_cursor = Some("cycle".into()),
            Some("oversized") => {
                result.resources = vec![
                    Resource::new("fixture://large", "large")
                        .with_description("x".repeat(128 * 1024)),
                ];
            }
            _ => {
                return Err(rmcp::ErrorData::invalid_params(
                    "unknown fixture cursor",
                    None,
                ));
            }
        }
        Ok(result)
    }

    pub(super) fn templates(
        &self,
        request: Option<PaginatedRequestParams>,
    ) -> Result<ListResourceTemplatesResult, rmcp::ErrorData> {
        self.templates.fetch_add(1, Ordering::SeqCst);
        let cursor = request.and_then(|request| request.cursor);
        self.record("resources/templates/list", cursor.as_deref())?;
        if let Some(error) = &self.error {
            return Err(error.clone());
        }
        let mut result = ListResourceTemplatesResult::default();
        match cursor.as_deref() {
            None => {
                result.resource_templates =
                    vec![ResourceTemplate::new("fixture://{name}", "fixture")];
                result.next_cursor = Some("templates-2".into());
            }
            Some("templates-2") => {}
            _ => {
                return Err(rmcp::ErrorData::invalid_params(
                    "unknown fixture template cursor",
                    None,
                ));
            }
        }
        Ok(result)
    }

    pub(super) async fn read(
        &self,
        request: ReadResourceRequestParams,
        context: RequestContext<RoleServer>,
    ) -> Result<ReadResourceResult, rmcp::ErrorData> {
        self.reads.fetch_add(1, Ordering::SeqCst);
        self.record("resources/read", Some(&request.uri))?;
        if let Some(error) = &self.error {
            return Err(error.clone());
        }
        if request.uri == "fixture://progress"
            && let Some(token) = context.meta.get_progress_token()
        {
            context
                .peer
                .notify_progress(
                    rmcp::model::ProgressNotificationParam::new(token, 1.0)
                        .with_message("resource read complete"),
                )
                .await
                .map_err(|error| rmcp::ErrorData::internal_error(error.to_string(), None))?;
        }
        if request.uri == "fixture://withhold" {
            self.read_entered.notify_one();
            tokio::select! {
                () = self.read_release.notified() => {},
                () = context.ct.cancelled() => {
                    self.read_cancelled.notify_one();
                    std::future::pending::<()>().await;
                }
            }
        }
        Self::contents(request)
    }

    pub(crate) fn contents(
        request: ReadResourceRequestParams,
    ) -> Result<ReadResourceResult, rmcp::ErrorData> {
        let content = match request.uri.as_str() {
            "fixture://ten-blobs" | "fixture://ten-blobs-spilled" => (0..10)
                .map(|index| {
                    ResourceContents::blob(
                        base64::engine::general_purpose::STANDARD
                            .encode(format!("resource-blob-{index}")),
                        format!("fixture://blob-{index}"),
                    )
                    .with_mime_type("application/octet-stream")
                    .with_meta(rmcp::model::MetaObject(
                        serde_json::json!({
                            "artifactId": format!("remote-artifact-{index}"),
                            "padding": if request.uri.ends_with("-spilled") { "p".repeat(4096) } else { String::new() }
                        })
                            .as_object()
                            .unwrap()
                            .clone(),
                    ))
                })
                .collect(),
            "fixture://blob" => vec![
                ResourceContents::blob(
                    base64::engine::general_purpose::STANDARD.encode(b"fixture binary"),
                    request.uri,
                )
                .with_mime_type("application/octet-stream"),
            ],
            "fixture://large" => vec![ResourceContents::text("x".repeat(128 * 1024), request.uri)],
            "fixture://invalid-blob" => vec![ResourceContents::blob("%%%", request.uri)],
            "fixture://error" => {
                return Err(rmcp::ErrorData::resource_not_found(
                    "fixture resource missing",
                    Some(serde_json::json!({"uri":request.uri})),
                ));
            }
            _ => vec![
                ResourceContents::text(
                    format!("peer content for {}", request.uri),
                    request.uri.clone(),
                ),
                ResourceContents::text("second block", request.uri).with_mime_type("text/markdown"),
            ],
        };
        Ok(ReadResourceResult::new(content))
    }
}
