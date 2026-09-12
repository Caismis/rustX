//! User-default document authority. Independent from every live Session owner.
//!
//! Cooperating writers hold a persistent sibling file lock. Publication is one
//! same-directory rename, after validation and a final source fingerprint check.
//! Noncooperating editors can still race the final check and rename; this is not CAS.
use std::fs::{File, OpenOptions};
use std::io::Write;
use std::os::unix::fs::OpenOptionsExt;
use std::path::{Path, PathBuf};

use sha2::{Digest, Sha256};
use toml_edit::{DocumentMut, Item, TableLike};

use super::launch::ResolvedLaunch;
use crate::runtime_client::settings::{
    DefaultDocument, DefaultScope, DefaultSettingsStore, DefaultValue, ModelDefault,
    SaveDefaultResult, SettingsBoundary, SettingsFuture,
};
use crate::runtime_client::types::RuntimeClientError;

#[derive(Clone)]
pub(crate) struct UserDefaults {
    directory: PathBuf,
    models: crate::model::catalog::ModelCatalog,
}

fn failure(message: &str) -> RuntimeClientError {
    RuntimeClientError::InvalidRequest {
        message: message.into(),
    }
}
fn io_failure(_: impl std::fmt::Debug) -> RuntimeClientError {
    failure(
        "default document I/O failed before publication; inspect document permissions and retry after reading its revision",
    )
}
fn save_worker_failure(_: impl std::fmt::Debug) -> RuntimeClientError {
    failure(
        "default save worker stopped; publication outcome is unknown; read the default document and review its revision before any retry",
    )
}
fn invalid(_: impl std::fmt::Debug) -> RuntimeClientError {
    failure(
        "invalid user default document or model/profile; correct the target document or selection before saving",
    )
}

/// Shared with create-only initialization. Never unlink the lock inode.
pub(super) fn lock_document(target: &Path) -> std::io::Result<File> {
    let name = target
        .file_name()
        .ok_or_else(|| std::io::Error::other("invalid document"))?;
    let lock = target.with_file_name(format!(".{}.lock", name.to_string_lossy()));
    let file = OpenOptions::new()
        .create(true)
        .truncate(false)
        .read(true)
        .write(true)
        .mode(0o600)
        .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
        .open(lock)?;
    if !file.metadata()?.is_file() {
        return Err(std::io::Error::other("lock is not a regular file"));
    }
    file.lock()?;
    Ok(file)
}

fn read_document(path: &Path) -> Result<Option<Vec<u8>>, RuntimeClientError> {
    match std::fs::symlink_metadata(path) {
        Ok(meta) if meta.file_type().is_symlink() || !meta.is_file() => Err(failure(
            "default document must be a regular file, not a symlink",
        )),
        Ok(_) => crate::bounded_file::read_bounded(path)
            .map(Some)
            .map_err(io_failure),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(io_failure(error)),
    }
}
fn revision(bytes: Option<&[u8]>) -> String {
    bytes.map_or_else(
        || "missing".into(),
        |bytes| format!("sha256:{:x}", Sha256::digest(bytes)),
    )
}
fn check_revision(path: &Path, expected: &str) -> Result<Option<Vec<u8>>, RuntimeClientError> {
    let bytes = read_document(path)?;
    if revision(bytes.as_deref()) != expected {
        return Err(failure(
            "stale default document revision; nothing published by this save; read defaults again and review the external change",
        ));
    }
    Ok(bytes)
}

// TOML's parser rejects duplicate keys/tables before any mutation.
fn tree(bytes: &[u8]) -> Result<DocumentMut, RuntimeClientError> {
    std::str::from_utf8(bytes)
        .map_err(invalid)?
        .parse()
        .map_err(invalid)
}
fn set(table: &mut dyn TableLike, key: &str, mut value: Item) {
    if let Some(old) = table.get(key).and_then(Item::as_value)
        && let Some(new) = value.as_value_mut()
    {
        *new.decor_mut() = old.decor().clone();
    }
    if table.get(key).and_then(Item::as_str).is_some()
        && table.get(key).and_then(Item::as_str) == value.as_str()
    {
        return;
    }
    table.insert(key, value);
}
fn update(bytes: &[u8], value: &DefaultValue) -> Result<Vec<u8>, RuntimeClientError> {
    let mut root = tree(bytes)?;
    match value {
        DefaultValue::ModelSelection { selection } => {
            if !root.contains_key("agent") {
                root["agent"] = Item::Table(toml_edit::Table::new());
            }
            let agent = root["agent"]
                .as_table_like_mut()
                .ok_or_else(|| invalid(()))?;
            if !agent.contains_key("model") {
                agent.insert("model", Item::Table(toml_edit::Table::new()));
            }
            let model = agent
                .get_mut("model")
                .expect("model table inserted")
                .as_table_like_mut()
                .ok_or_else(|| invalid(()))?;
            set(
                model,
                "model",
                toml_edit::value(selection.model.to_string()),
            );
            let mut profile = toml_edit::InlineTable::new();
            if let Some(name) = &selection.reasoning_profile {
                profile.insert("mode", "profile".into());
                profile.insert("name", name.to_string().into());
            } else {
                profile.insert("mode", "catalog_default".into());
            }
            let unchanged = model
                .get("reasoning_profile")
                .and_then(Item::as_table_like)
                .is_some_and(|old| {
                    old.get("mode").and_then(Item::as_str)
                        == profile.get("mode").and_then(toml_edit::Value::as_str)
                        && old.get("name").and_then(Item::as_str)
                            == profile.get("name").and_then(toml_edit::Value::as_str)
                });
            if !unchanged {
                set(model, "reasoning_profile", Item::Value(profile.into()));
            }
        }
        DefaultValue::ApprovalMode { mode } => {
            let mode = match mode {
                crate::runtime::ApprovalMode::Policy => "policy",
                crate::runtime::ApprovalMode::FullAccess => "full_access",
            };
            set(root.as_table_mut(), "approval_mode", toml_edit::value(mode));
        }
    }
    Ok(root.to_string().into_bytes())
}

/// Deterministic tests force boundaries without timing assumptions.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Frontier {
    BeforeLock,
    Locked,
    Staged,
    Validated,
    FinalChecked,
}

impl UserDefaults {
    pub(crate) fn new(paths: &ResolvedLaunch) -> Self {
        Self {
            directory: paths.host.config_directory.clone(),
            models: paths.models.clone(),
        }
    }
    fn target(&self) -> PathBuf {
        self.directory.join("settings.toml")
    }
    fn read_sync(&self, scope: DefaultScope) -> Result<DefaultDocument, RuntimeClientError> {
        let path = self.target();
        let bytes = read_document(&path)?;
        let layer: super::authoring::RuntimeLayer = if let Some(bytes) = &bytes {
            super::launch::parse_layer(&path, bytes, false).map_err(invalid)?
        } else {
            super::authoring::RuntimeLayer::default()
        };
        let selection = layer.agent.and_then(|agent| agent.model).and_then(|model| {
            model.model.map(|selected| ModelDefault {
                model: selected,
                reasoning_profile: model
                    .reasoning_profile
                    .and_then(super::authoring::ReasoningSelection::resolve),
            })
        });
        Ok(DefaultDocument {
            scope,
            document: path.display().to_string(),
            revision: revision(bytes.as_deref()),
            model: selection,
            approval_mode: layer.approval_mode,
        })
    }
    fn save_sync(
        &self,
        scope: DefaultScope,
        expected: &str,
        value: DefaultValue,
    ) -> Result<SaveDefaultResult, RuntimeClientError> {
        self.save_at(scope, expected, value, |_| Ok(()))
    }
    fn save_at(
        &self,
        scope: DefaultScope,
        expected: &str,
        value: DefaultValue,
        mut frontier: impl FnMut(Frontier) -> Result<(), RuntimeClientError>,
    ) -> Result<SaveDefaultResult, RuntimeClientError> {
        let target = self.target();
        frontier(Frontier::BeforeLock)?;
        // Parent must already exist (init owns directory creation). Canonicalize
        // it so aliases share a lock; target symlinks are explicitly refused.
        let parent = std::fs::canonicalize(&self.directory).map_err(io_failure)?;
        let target = parent.join(target.file_name().ok_or_else(|| invalid(()))?);
        let _lock = lock_document(&target).map_err(io_failure)?;
        frontier(Frontier::Locked)?;
        let original = check_revision(&target, expected)?;
        let candidate = update(original.as_deref().unwrap_or(b""), &value)?;
        let mut staged = tempfile::NamedTempFile::new_in(&parent).map_err(io_failure)?;
        if original.is_some() {
            staged
                .as_file()
                .set_permissions(
                    std::fs::metadata(&target)
                        .map_err(io_failure)?
                        .permissions(),
                )
                .map_err(io_failure)?;
        }
        staged.write_all(&candidate).map_err(io_failure)?;
        staged.as_file().sync_all().map_err(io_failure)?;
        frontier(Frontier::Staged)?;
        let bytes = crate::bounded_file::read_bounded(staged.path()).map_err(io_failure)?;
        // Reuse the canonical user-layer schema/ownership parser. This validates
        // the document, not readiness of any mutable project or resource files.
        let layer = super::launch::parse_layer(&target, &bytes, false).map_err(invalid)?;
        if matches!(value, DefaultValue::ModelSelection { .. }) {
            let (config, context) = super::launch::user_model_sections(layer).map_err(invalid)?;
            let (primary, summary) =
                crate::model::session::analyze_session_model_config(&self.models, &config)
                    .map_err(invalid)?;
            let summary = summary.as_ref().unwrap_or(&primary);
            context
                .to_policy()
                .validate_budgets(
                    (primary.context_window, primary.max_output_tokens),
                    (summary.context_window, summary.max_output_tokens),
                )
                .map_err(invalid)?;
        }
        frontier(Frontier::Validated)?;
        check_revision(&target, expected)?;
        frontier(Frontier::FinalChecked)?;
        // PUBLICATION COMMIT. No fallible durability operation follows this rename.
        // Power-loss directory durability is not promised. External editors ignoring
        // the lock can race after FinalChecked; the API deliberately makes no CAS claim.
        staged.persist(&target).map_err(io_failure)?;
        Ok(SaveDefaultResult {
            scope,
            document: target.display().to_string(),
            revision: revision(Some(&bytes)),
            changed: value,
            live_unchanged: true,
            applies_at: SettingsBoundary::NextLaunch,
        })
    }
}
impl DefaultSettingsStore for UserDefaults {
    fn read(&self, scope: DefaultScope) -> SettingsFuture<DefaultDocument> {
        let owner = self.clone();
        Box::pin(async move {
            tokio::task::spawn_blocking(move || owner.read_sync(scope))
                .await
                .map_err(io_failure)?
        })
    }
    fn save(
        &self,
        scope: DefaultScope,
        expected: String,
        value: DefaultValue,
    ) -> SettingsFuture<SaveDefaultResult> {
        let owner = self.clone();
        Box::pin(async move {
            tokio::task::spawn_blocking(move || owner.save_sync(scope, &expected, value))
                .await
                .map_err(save_worker_failure)?
        })
    }
}

#[cfg(test)]
mod tests {
    use super::super::launch::{HostEnvironment, LaunchRequest};
    use super::*;
    use std::sync::{Arc, Barrier, mpsc};

    fn fixture() -> (tempfile::TempDir, UserDefaults) {
        let root = tempfile::tempdir().unwrap();
        let workspace = root.path().join("workspace");
        std::fs::create_dir(&workspace).unwrap();
        let host =
            HostEnvironment::from_paths(workspace.clone(), root.path().join("home"), None, None)
                .unwrap();
        std::fs::create_dir_all(&host.config_directory).unwrap();
        let models = include_str!("../../examples/local-runtime/minimal/models.toml");
        std::fs::write(host.config_directory.join("models.toml"), models).unwrap();
        std::fs::write(
            host.config_directory.join("settings.toml"),
            br#"# preserve my reason
[environment]
PRIVATE = "SECRET_SENTINEL"


[agent]
[agent.model]
model = "example/demo-model"

[agent.model.reasoning_profile] # keep profile comment
mode = "catalog_default"


[agent.tools]
builtin = ["read"]
"#,
        )
        .unwrap();
        let request = LaunchRequest {
            workspace: Some(workspace),
            ..LaunchRequest::default()
        };
        super::super::launch::change_trust(
            &request,
            &host,
            super::super::launch::TrustAction::Grant,
        )
        .unwrap();
        let launch = super::super::launch::analyze(&request, &host)
            .unwrap()
            .admit(|| crate::credentials::CredentialSnapshot::new([]))
            .unwrap();
        (root, UserDefaults::new(&launch))
    }
    fn approval() -> DefaultValue {
        DefaultValue::ApprovalMode {
            mode: crate::runtime::ApprovalMode::FullAccess,
        }
    }
    fn selected() -> DefaultValue {
        DefaultValue::ModelSelection {
            selection: ModelDefault {
                model: crate::model::catalog::ModelRef::parse("example/demo-model").unwrap(),
                reasoning_profile: None,
            },
        }
    }
    fn current(owner: &UserDefaults) -> String {
        owner.read_sync(DefaultScope::User).unwrap().revision
    }

    fn project_fixture() -> (tempfile::TempDir, UserDefaults, PathBuf) {
        let (root, _) = fixture();
        let workspace = root.path().join("workspace");
        let project = workspace.join("project.toml");
        std::fs::write(
            &project,
            br#"[agent]
[agent.model]
model = "example/demo-model"
"#,
        )
        .unwrap();
        let host =
            HostEnvironment::from_paths(workspace.clone(), root.path().join("home"), None, None)
                .unwrap();
        let request = LaunchRequest {
            workspace: Some(workspace),
            config: Some(project.clone()),
            ..LaunchRequest::default()
        };
        let launch = super::super::launch::analyze(&request, &host)
            .unwrap()
            .admit(|| crate::credentials::CredentialSnapshot::new([]))
            .unwrap();
        (root, UserDefaults::new(&launch), project)
    }

    fn model_change_fixture(preserved: &str) -> (tempfile::TempDir, UserDefaults) {
        let (root, owner) = fixture();
        let mut catalog: serde_json::Value = crate::toml_authoring::parse(include_bytes!(
            "../../examples/local-runtime/minimal/models.toml"
        ))
        .unwrap();
        let mut a = catalog["providers"]["example"]["models"][0].clone();
        a["id"] = "a".into();
        a["protocol"] = "openai_responses".into();
        a.as_object_mut().unwrap().remove("compat");
        let mut b = a.clone();
        b["id"] = "b".into();
        b["protocol"] = "openai_chat_completions".into();
        b["compat"] = serde_json::json!({"chat_reasoning_replay": "omit"});
        b["max_output_tokens"] = 2048.into();
        b["context_window"] = 8192.into();
        catalog["providers"]["example"]["models"] = serde_json::json!([a, b]);
        std::fs::write(
            owner.directory.join("models.toml"),
            toml::to_string_pretty(&catalog).unwrap(),
        )
        .unwrap();
        std::fs::write(
            owner.target(),
            format!(
                r#"# preserved settings
[agent.model]
model = "example/a"
{preserved}
"#
            ),
        )
        .unwrap();
        let workspace = root.path().join("workspace");
        let host =
            HostEnvironment::from_paths(workspace.clone(), root.path().join("home"), None, None)
                .unwrap();
        let launch = super::super::launch::analyze(
            &LaunchRequest {
                workspace: Some(workspace),
                ..LaunchRequest::default()
            },
            &host,
        )
        .unwrap()
        .admit(|| crate::credentials::CredentialSnapshot::new([]))
        .unwrap();
        let owner = UserDefaults::new(&launch);
        std::fs::remove_file(owner.directory.join("models.toml")).unwrap();
        (root, owner)
    }

    fn assert_preserved_model_setting_rejected(preserved: &str) {
        let (_root, owner) = model_change_fixture(preserved);
        let before = std::fs::read(owner.target()).unwrap();
        let expected = current(&owner);
        let value = DefaultValue::ModelSelection {
            selection: ModelDefault {
                model: crate::model::catalog::ModelRef::parse("example/b").unwrap(),
                reasoning_profile: None,
            },
        };
        let mut staged = false;
        let mut validated = false;
        let result = owner.save_at(DefaultScope::User, &expected, value, |point| {
            staged |= point == Frontier::Staged;
            validated |= point == Frontier::Validated;
            Ok(())
        });
        let error = result.expect_err("the actual staged model must be rejected");
        assert!(
            !serde_json::to_string(&error)
                .unwrap()
                .contains("SECRET_SENTINEL")
        );
        assert!(
            staged && !validated,
            "rejection precedes the publication frontier"
        );
        assert_eq!(std::fs::read(owner.target()).unwrap(), before);
        assert_eq!(current(&owner), expected);
    }

    #[test]
    fn cfg238_staged_model_preserved_output_budget_is_validated() {
        assert_preserved_model_setting_rejected(
            r#"max_output_tokens = { mode = "limit", tokens = 4096 }"#,
        );
    }

    #[test]
    fn cfg238_staged_model_preserved_request_params_are_validated() {
        // `messages` is opaque to Responses but runtime-owned by Chat Completions.
        assert_preserved_model_setting_rejected(
            r#"request_params_json = '{"messages": ["SECRET_SENTINEL"]}'"#,
        );
    }

    #[test]
    fn cfg238_staged_model_validates_user_context_with_builtin_defaults() {
        let (_root, owner) = model_change_fixture(r"request_params_json = '{}'");
        // Valid for A/128000; B/8192 cannot fit this reserve plus its 2048
        // output budget. Other context fields use the canonical built-in defaults.
        let bytes = br#"[context]
reserve_tokens = 6144


[agent]
[agent.model]
model = "example/a"
"#;
        std::fs::write(owner.target(), bytes).unwrap();
        let expected = current(&owner);
        let value = DefaultValue::ModelSelection {
            selection: ModelDefault {
                model: crate::model::catalog::ModelRef::parse("example/b").unwrap(),
                reasoning_profile: None,
            },
        };
        let mut validated = false;
        assert!(
            owner
                .save_at(DefaultScope::User, &expected, value, |point| {
                    validated |= point == Frontier::Validated;
                    Ok(())
                })
                .is_err()
        );
        assert!(!validated);
        assert_eq!(std::fs::read(owner.target()).unwrap(), bytes);
        assert_eq!(current(&owner), expected);
    }

    #[test]
    fn cfg238_approval_save_does_not_validate_unrelated_model_semantics() {
        let (_root, owner) = fixture();
        let bytes = br#"[context]
[context.summary_output_cap]
mode = "limit"
tokens = 0


[agent]
[agent.model]
model = "example/missing"

[agent.model.max_output_tokens]
mode = "limit"
tokens = 0
"#;
        std::fs::write(owner.target(), bytes).unwrap();
        let result = owner
            .save_sync(DefaultScope::User, &current(&owner), approval())
            .unwrap();
        let after: serde_json::Value =
            crate::toml_authoring::parse(&std::fs::read(owner.target()).unwrap()).unwrap();
        let before: serde_json::Value = crate::toml_authoring::parse(bytes).unwrap();
        assert_eq!(after["agent"]["model"], before["agent"]["model"]);
        assert_eq!(after["context"], before["context"]);
        assert_eq!(after["approval_mode"], "full_access");
        assert_eq!(current(&owner), result.revision);
    }

    #[test]
    fn cfg238_user_write_does_not_reopen_mutable_project_or_catalog() {
        for delete in [false, true] {
            let (_root, owner, project) = project_fixture();
            let expected = current(&owner);
            if delete {
                std::fs::remove_file(&project).unwrap();
            } else {
                std::fs::write(&project, b"broken project SECRET_SENTINEL").unwrap();
            }
            // Domain authority is the captured catalog, not a newly loaded file.
            std::fs::remove_file(owner.directory.join("models.toml")).unwrap();
            let result = owner
                .save_sync(DefaultScope::User, &expected, selected())
                .unwrap();
            assert_eq!(current(&owner), result.revision);
            if delete {
                assert!(!project.exists());
            } else {
                assert_eq!(
                    std::fs::read(&project).unwrap(),
                    b"broken project SECRET_SENTINEL"
                );
            }
        }
    }

    #[test]
    fn cfg238_project_override_cannot_mask_invalid_saved_selection() {
        let (_root, owner, project) = project_fixture();
        let before = std::fs::read(owner.target()).unwrap();
        let project_before = std::fs::read(&project).unwrap();
        for (model, profile) in [
            ("example/missing", None),
            ("example/demo-model", Some("missing-profile")),
        ] {
            let value = DefaultValue::ModelSelection {
                selection: ModelDefault {
                    model: crate::model::catalog::ModelRef::parse(model).unwrap(),
                    reasoning_profile: profile
                        .map(|p| crate::model::catalog::ReasoningProfileId::parse(p).unwrap()),
                },
            };
            let mut validated = false;
            let result = owner.save_at(DefaultScope::User, &current(&owner), value, |point| {
                if point == Frontier::Validated {
                    validated = true;
                }
                Ok(())
            });
            assert!(result.is_err());
            assert!(
                !validated,
                "native target analysis must fail before validation frontier"
            );
            assert_eq!(std::fs::read(owner.target()).unwrap(), before);
            assert_eq!(std::fs::read(&project).unwrap(), project_before);
        }
    }

    #[test]
    fn cfg238_user_layer_schema_and_ownership_still_gate_publication() {
        let (_root, owner) = fixture();
        for invalid in [
            br"unknown = true
"
            .as_slice(),
            br#"workspace = "/forbidden"
"#,
            br"[context]
unexpected = true
",
            br#"[agent]
[agent.model]
model = "example/demo-model"

[agent.model.max_output_tokens]
mode = "limit"
tokens = "wrong-type"
"#,
        ] {
            std::fs::write(owner.target(), invalid).unwrap();
            let mut validated = false;
            assert!(
                owner
                    .save_at(
                        DefaultScope::User,
                        &revision(Some(invalid)),
                        approval(),
                        |point| {
                            if point == Frontier::Validated {
                                validated = true;
                            }
                            Ok(())
                        }
                    )
                    .is_err()
            );
            assert!(!validated);
            assert_eq!(std::fs::read(owner.target()).unwrap(), invalid);
        }
    }

    #[test]
    fn cfg238_save_preserves_comments_unrelated_content_and_redacts_outputs() {
        let (_root, owner) = fixture();
        let before = std::fs::read(owner.target()).unwrap();
        let read = owner.read_sync(DefaultScope::User).unwrap();
        let result = owner
            .save_sync(DefaultScope::User, &read.revision, selected())
            .unwrap();
        let after = std::fs::read(owner.target()).unwrap();
        assert_eq!(before, after, "same values preserve exact existing bytes");
        let result2 = owner
            .save_sync(DefaultScope::User, &result.revision, approval())
            .unwrap();
        let after = std::fs::read_to_string(owner.target()).unwrap();
        assert!(after.contains("# preserve my reason"));
        assert!(after.contains("# keep profile comment"));
        assert!(after.contains("PRIVATE = \"SECRET_SENTINEL\""));
        let mut expected: serde_json::Value = crate::toml_authoring::parse(&before).unwrap();
        expected["approval_mode"] = serde_json::json!("full_access");
        assert_eq!(
            expected,
            crate::toml_authoring::parse::<serde_json::Value>(after.as_bytes()).unwrap()
        );
        for output in [
            serde_json::to_string(&read).unwrap(),
            serde_json::to_string(&result2).unwrap(),
            format!("{result2:?}"),
        ] {
            assert!(!output.contains("SECRET_SENTINEL"));
            assert!(!output.contains("PRIVATE"));
        }
        assert!(result2.live_unchanged);
        assert_eq!(result2.applies_at, SettingsBoundary::NextLaunch);
    }

    #[tokio::test]
    async fn cfg238_worker_failure_reports_unknown_publication_without_claiming_rollback() {
        let (_root, owner) = fixture();
        let expected = current(&owner);
        let worker = owner.clone();
        let error = tokio::task::spawn_blocking(move || {
            worker
                .save_sync(DefaultScope::User, &expected, approval())
                .unwrap();
            panic!("injected post-publication worker failure");
        })
        .await
        .map_err(save_worker_failure)
        .unwrap_err();
        assert_eq!(
            owner.read_sync(DefaultScope::User).unwrap().approval_mode,
            Some(crate::runtime::ApprovalMode::FullAccess)
        );
        let wire = serde_json::to_string(&error).unwrap();
        assert!(wire.contains("outcome is unknown"));
        assert!(!wire.contains("before publication"));
        assert!(!wire.contains("SECRET_SENTINEL"));
    }

    #[test]
    fn cfg238_stale_revision_rejected_after_publication() {
        let (_root, owner) = fixture();
        let a = current(&owner);
        let b = owner.save_sync(DefaultScope::User, &a, approval()).unwrap();
        assert_ne!(a, b.revision);
        let error = owner
            .save_sync(DefaultScope::User, &a, selected())
            .unwrap_err();
        assert!(format!("{error:?}").contains("stale"));
        assert_eq!(current(&owner), b.revision);
    }

    #[test]
    fn cfg238_competing_writers_serialize_then_recheck_expected_revision() {
        let (_root, owner) = fixture();
        let a = current(&owner);
        let (held_tx, held_rx) = mpsc::channel();
        let gate = Arc::new(Barrier::new(2));
        let first_owner = owner.clone();
        let first_expected = a.clone();
        let first_gate = gate.clone();
        let first = std::thread::spawn(move || {
            first_owner.save_at(DefaultScope::User, &first_expected, approval(), |point| {
                if point == Frontier::Locked {
                    held_tx.send(()).unwrap();
                    first_gate.wait();
                }
                Ok(())
            })
        });
        held_rx.recv().unwrap();
        let (ready_tx, ready_rx) = mpsc::channel();
        let second_owner = owner.clone();
        let second = std::thread::spawn(move || {
            second_owner.save_at(DefaultScope::User, &a, selected(), |point| {
                if point == Frontier::BeforeLock {
                    ready_tx.send(()).unwrap();
                }
                if point == Frontier::Locked {
                    // Acquiring the same lock is proof that the first publication has
                    // completed; the first writer holds it until after its result exists.
                    assert_eq!(
                        second_owner
                            .read_sync(DefaultScope::User)
                            .unwrap()
                            .approval_mode,
                        Some(crate::runtime::ApprovalMode::FullAccess)
                    );
                }
                Ok(())
            })
        });
        ready_rx.recv().unwrap();
        gate.wait();
        let published = first.join().unwrap().unwrap();
        assert!(format!("{:?}", second.join().unwrap().unwrap_err()).contains("stale"));
        assert_eq!(current(&owner), published.revision);
    }

    #[test]
    fn cfg238_external_edit_before_final_check_is_rejected() {
        let (_root, owner) = fixture();
        let a = current(&owner);
        let external = br#"[environment]
PRIVATE = "EXTERNAL_SECRET"


[agent]
[agent.model]
model = "example/demo-model"
"#;
        let error = owner
            .save_at(DefaultScope::User, &a, approval(), |point| {
                if point == Frontier::Validated {
                    std::fs::write(owner.target(), external).unwrap();
                }
                Ok(())
            })
            .unwrap_err();
        assert_eq!(std::fs::read(owner.target()).unwrap(), external);
        let wire = serde_json::to_string(&error).unwrap();
        assert!(wire.contains("stale"));
        assert!(!wire.contains("EXTERNAL_SECRET"));
    }

    #[test]
    fn cfg238_external_edit_after_final_check_demonstrates_non_cas_limit() {
        let (_root, owner) = fixture();
        let a = current(&owner);
        let result = owner
            .save_at(DefaultScope::User, &a, approval(), |point| {
                if point == Frontier::FinalChecked {
                    std::fs::write(owner.target(), b"external after frontier").unwrap();
                }
                Ok(())
            })
            .unwrap();
        // Deliberately document the actual limitation: rename wins this race.
        assert_eq!(current(&owner), result.revision);
        assert_eq!(
            owner.read_sync(DefaultScope::User).unwrap().approval_mode,
            Some(crate::runtime::ApprovalMode::FullAccess)
        );
    }

    #[test]
    fn cfg238_invalid_candidate_and_staged_failure_preserve_old_document() {
        let (_root, owner) = fixture();
        let a = current(&owner);
        let before = std::fs::read(owner.target()).unwrap();
        let invalid_profile = DefaultValue::ModelSelection {
            selection: ModelDefault {
                model: crate::model::catalog::ModelRef::parse("example/demo-model").unwrap(),
                reasoning_profile: Some(
                    crate::model::catalog::ReasoningProfileId::parse("SECRET_SENTINEL").unwrap(),
                ),
            },
        };
        let error = owner
            .save_sync(DefaultScope::User, &a, invalid_profile)
            .unwrap_err();
        assert!(
            !serde_json::to_string(&error)
                .unwrap()
                .contains("SECRET_SENTINEL")
        );
        assert_eq!(std::fs::read(owner.target()).unwrap(), before);
        for at in [
            Frontier::Staged,
            Frontier::Validated,
            Frontier::FinalChecked,
        ] {
            owner
                .save_at(DefaultScope::User, &a, approval(), |point| {
                    if point == at {
                        Err(failure("injected pre-publication failure"))
                    } else {
                        Ok(())
                    }
                })
                .unwrap_err();
            assert_eq!(std::fs::read(owner.target()).unwrap(), before);
        }
    }

    #[test]
    fn cfg238_duplicate_keys_and_symlinks_are_refused() {
        let (_root, owner) = fixture();
        let duplicate = br#"
[agent.model]
model = "example/demo-model"

[environment]
x = "SECRET_SENTINEL"
x = "second"
"#;
        std::fs::write(owner.target(), duplicate).unwrap();
        let error = owner
            .save_sync(DefaultScope::User, &revision(Some(duplicate)), approval())
            .unwrap_err();
        assert!(format!("{error:?}").contains("invalid user default document"));
        assert!(!format!("{error:?}").contains("SECRET_SENTINEL"));
        assert_eq!(std::fs::read(owner.target()).unwrap(), duplicate);
        let other = owner.target().with_file_name("other.toml");
        std::fs::rename(owner.target(), &other).unwrap();
        std::os::unix::fs::symlink(&other, owner.target()).unwrap();
        assert!(owner.read_sync(DefaultScope::User).is_err());
        assert_eq!(std::fs::read(other).unwrap(), duplicate);
    }
}
