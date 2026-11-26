//! Paired #612/#613 release proof: a published synced Skill is readable by
//! the real native tool and remains read-only to file-tool mutations.

use super::{Fixture, bounded};
use std::os::unix::fs::{MetadataExt as _, PermissionsExt as _};
use std::path::Path;

use crate::runtime::cancellation::{CancellationSignal, ExecutionCancellation};
use crate::runtime::identity::{ConversationId, ToolCallId, ToolId};
use crate::runtime::types::CancellationReason;
use crate::tools::artifacts::ArtifactStore;
use crate::tools::environment::ToolEnvironment;
use crate::tools::executor::{ProgressReporter, ToolExecutionContext, ToolExecutor};
use crate::tools::managed_output::ManagedToolOutput;
use crate::tools::native::{ReadTool, support::bind_invocation_mutation_target};
use crate::tools::types::{
    ToolExecutionStatus, ToolInvocation, ToolInvocationId, ToolInvocationMode, ToolOrigin,
    ToolProgress, ToolResultContent,
};
use crate::tools::workspace::Workspace;

const INSTRUCTIONS: &str =
    "---\nname: paired-global\ndescription: Paired release proof\n---\nRead reference.md.\n";
const REFERENCE: &str = "Client-only attachment.\n";

struct NoProgress;

impl ProgressReporter for NoProgress {
    fn report(&self, _progress: ToolProgress) {}
}

struct NativeTools {
    conversation: ConversationId,
    workspace: Workspace,
    artifacts: ArtifactStore,
    output: ManagedToolOutput,
    environment: ToolEnvironment,
    cancellation: ExecutionCancellation,
    progress: NoProgress,
}

impl NativeTools {
    async fn new(fixture: &Fixture, storage: &Path) -> Self {
        let conversation = fixture.id(0).await;
        let workspace = Workspace::for_session(
            &fixture.workspaces[0],
            fixture.manager.execution_scope(&fixture.sessions[0].id),
        )
        .unwrap();
        let artifacts =
            ArtifactStore::new(conversation.clone(), storage.join("artifacts")).unwrap();
        let output = ManagedToolOutput::new(conversation.clone(), storage.join("output")).unwrap();
        let cancellation = ExecutionCancellation::detached(
            CancellationSignal::new(),
            CancellationReason::UserRequested,
        );
        Self {
            conversation,
            workspace,
            artifacts,
            output,
            environment: ToolEnvironment::new(),
            cancellation,
            progress: NoProgress,
        }
    }

    fn context(&self) -> ToolExecutionContext<'_> {
        ToolExecutionContext::new(
            &self.conversation,
            None,
            self.cancellation.clone(),
            &self.workspace,
            &self.progress,
            &self.artifacts,
            &self.output,
            &self.environment,
        )
    }

    async fn read(&self, path: &Path) -> String {
        let result = ReadTool
            .start(
                ToolInvocation {
                    id: ToolInvocationId::Agent {
                        call_id: ToolCallId::new("paired-skill-read"),
                    },
                    tool_id: ToolId::new("tool-read"),
                    tool_name: "read".into(),
                    mode: ToolInvocationMode::Foreground,
                    arguments: serde_json::json!({ "path": path }),
                },
                self.context(),
            )
            .completion
            .await;
        assert_eq!(result.status, ToolExecutionStatus::Success, "{result:?}");
        let [ToolResultContent::Text(text)] = result.content.as_slice() else {
            panic!("Read did not return text: {result:?}");
        };
        text.text.clone()
    }

    async fn assert_mutation_denied(&self, name: &str, arguments: serde_json::Value) {
        let error = bind_invocation_mutation_target(
            &ToolOrigin::Builtin,
            name,
            &arguments,
            &self.workspace,
            &self.output,
            &self.cancellation,
        )
        .await
        .expect_err("the Client must refuse a protected Skill mutation");
        assert!(
            error.contains("physical access denied"),
            "the Client must issue the denial, not an unavailable route: {error}"
        );
    }
}

/// Runs only against the combined #612/#613 release: the Client's separate
/// HOME overlays the Server's home, and its `rustx` entry is a symlink.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "paired #612/#613 release proof; run test-support/prove_paired_skill_read.sh"]
async fn synced_catalog_location_and_attachment_use_real_read() {
    assert!(std::env::var_os("RUSTX_MOUNT_VIEW_PROOF").is_some());
    Box::pin(bounded(async {
        let fixture = Fixture::with_session_count(None, 1).await;
        let home = fixture
            .manager
            .configuration
            .source_bindings()
            .home_directory
            .clone();
        let source = home.join("rustx/.agents/skills/paired-global");
        std::fs::create_dir_all(&source).unwrap();
        std::fs::write(source.join("SKILL.md"), INSTRUCTIONS).unwrap();
        std::fs::write(source.join("reference.md"), REFERENCE).unwrap();

        let physical = tempfile::Builder::new()
            .prefix("rustx-paired-client-")
            .tempdir_in("/dev/shm")
            .unwrap();
        assert_ne!(
            std::fs::metadata(&home).unwrap().dev(),
            std::fs::metadata(physical.path()).unwrap().dev()
        );
        std::fs::create_dir(physical.path().join("client-rustx")).unwrap();
        std::os::unix::fs::symlink("client-rustx", physical.path().join("rustx")).unwrap();
        let mount = tempfile::tempdir().unwrap();
        std::os::unix::fs::symlink(physical.path(), mount.path().join("client-home")).unwrap();
        let binary = std::env::current_exe().unwrap().with_file_name("rustx-execution-client");
        std::os::unix::fs::symlink(binary, mount.path().join("client-real")).unwrap();
        let launcher = mount.path().join("client");
        std::fs::write(
            &launcher,
            r#"#!/bin/sh
set -eu
bin=${0%/*}
unset RUSTX_CLIENT_DOMAIN_STAGING
exec /usr/bin/bwrap --die-with-parent --ro-bind / / --bind "$bin/client-home" "$HOME" --dev /dev -- "$bin/client-real" "$@"
"#,
        )
        .unwrap();
        std::fs::set_permissions(&launcher, std::fs::Permissions::from_mode(0o700)).unwrap();

        let mut client = fixture.start_execution_via(0, Some(&launcher)).await;
        fixture.load(0).await.unwrap().unwrap();
        let runtime = fixture.manager.configuration_runtime(&fixture.sessions[0].id).unwrap();
        let resources = runtime.runtime_resources();
        let capability = resources.capability();
        let location = capability
            .model_skill_entries()
            .iter()
            .find(|entry| entry.name == "paired-global")
            .expect("the bound Skill is published to the model")
            .location
            .clone();
        let rendered = capability.skill_catalog().expect("Skill guidance is rendered");
        assert!(rendered.contains("Use the read tool"));
        assert!(rendered.contains(&location));
        let location = std::path::PathBuf::from(location);
        let reference = location.parent().unwrap().join("reference.md");
        assert!(location.starts_with(home.join("client-rustx/.agents/skills/.rustx-sync")));
        assert!(!location.starts_with(&fixture.workspaces[0]));
        assert!(!location.exists() && !reference.exists(), "Server view must not contain the Client copy");
        let client_location = physical.path().join(location.strip_prefix(&home).unwrap());
        assert!(client_location.is_file(), "the copied Skill is in the separate Client filesystem");

        let storage = tempfile::tempdir().unwrap();
        let tools = NativeTools::new(&fixture, storage.path()).await;
        assert_eq!(tools.read(&location).await, INSTRUCTIONS);
        assert_eq!(tools.read(&reference).await, REFERENCE);
        tools.assert_mutation_denied(
            "write",
            serde_json::json!({ "path": &location, "content": "changed" }),
        ).await;
        tools.assert_mutation_denied(
            "edit",
            serde_json::json!({
                "path": &location,
                "edits": [{ "oldText": "Read reference.md.", "newText": "changed" }]
            }),
        ).await;
        assert_eq!(tools.read(&location).await, INSTRUCTIONS);
        assert_eq!(tools.read(&reference).await, REFERENCE);
        assert_eq!(std::fs::read_to_string(source.join("SKILL.md")).unwrap(), INSTRUCTIONS);
        assert_eq!(std::fs::read_to_string(client_location).unwrap(), INSTRUCTIONS);
        drop(tools);
        drop(resources);
        drop(runtime);
        fixture.close().await;
        let _ = client.kill().await;
        let _ = client.wait().await;
    }))
    .await;
}
