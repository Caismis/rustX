//! User-default document authority. Independent from every live Session owner.
//!
//! Cooperating writers hold a persistent sibling file lock. Publication is one
//! same-directory rename, after validation and a final source fingerprint check.
//! Noncooperating editors can still race the final check and rename; this is not CAS.
use std::fs::{File, OpenOptions};
use std::io::Write;
use std::os::unix::fs::OpenOptionsExt;
use std::path::{Path, PathBuf};

use jsonc_parser::cst::{CstInputValue, CstObject, CstRootNode};
use sha2::{Digest, Sha256};

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
        Ok(_) => crate::config_format::read_bounded(path)
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

// Reject duplicate names (including escaped equivalents) rather than selecting an
// arbitrary occurrence. Recurse only through CST objects/arrays, never parse JSONC ourselves.
fn unique(node: jsonc_parser::cst::CstNode) -> Result<(), RuntimeClientError> {
    use jsonc_parser::cst::CstContainerNode;
    if let jsonc_parser::cst::CstNode::Container(container) = node {
        if let CstContainerNode::Object(object) = &container {
            let mut names = std::collections::BTreeSet::new();
            for prop in object.properties() {
                let name = prop
                    .name()
                    .ok_or_else(|| invalid(()))?
                    .decoded_value()
                    .map_err(invalid)?;
                if !names.insert(name) {
                    return Err(failure(
                        "duplicate JSONC properties prevent a non-destructive default save; remove duplicate keys first",
                    ));
                }
            }
        }
        for child in container.children() {
            unique(child)?;
        }
    }
    Ok(())
}
fn tree(bytes: &[u8]) -> Result<CstRootNode, RuntimeClientError> {
    let text = std::str::from_utf8(bytes).map_err(invalid)?;
    let root = CstRootNode::parse(text, &crate::config_format::OPTIONS).map_err(invalid)?;
    unique(root.clone().into())?;
    root.object_value().ok_or_else(|| invalid(()))?;
    Ok(root)
}
fn set(object: &CstObject, key: &str, value: CstInputValue) {
    if let Some(prop) = object.get(key) {
        prop.set_value(value);
    } else {
        object.append(key, value);
    }
}
fn update(bytes: &[u8], value: &DefaultValue) -> Result<Vec<u8>, RuntimeClientError> {
    let root = tree(bytes)?;
    let object = root.object_value().ok_or_else(|| invalid(()))?;
    match value {
        DefaultValue::ModelSelection { selection } => {
            let model = object
                .object_value_or_create("model")
                .ok_or_else(|| invalid(()))?;
            set(
                &model,
                "model",
                CstInputValue::String(selection.model.to_string()),
            );
            set(
                &model,
                "reasoningProfile",
                selection
                    .reasoning_profile
                    .as_ref()
                    .map_or(CstInputValue::Null, |profile| {
                        CstInputValue::String(profile.to_string())
                    }),
            );
        }
        DefaultValue::ApprovalMode { mode } => {
            let value = serde_json::to_value(mode).map_err(invalid)?;
            set(
                &object,
                "approvalMode",
                CstInputValue::String(value.as_str().ok_or_else(|| invalid(()))?.into()),
            );
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
        self.directory.join("settings.jsonc")
    }
    fn read_sync(&self, scope: DefaultScope) -> Result<DefaultDocument, RuntimeClientError> {
        let path = self.target();
        let bytes = read_document(&path)?;
        let model = if let Some(bytes) = &bytes {
            tree(bytes)?;
            crate::config_format::parse::<serde_json::Value>(bytes).map_err(invalid)?
        } else {
            serde_json::json!({})
        };
        let selection = model
            .get("model")
            .and_then(|m| m.get("model"))
            .map(|selected| -> Result<ModelDefault, RuntimeClientError> {
                Ok(ModelDefault {
                    model: serde_json::from_value(selected.clone()).map_err(invalid)?,
                    reasoning_profile: model
                        .get("model")
                        .and_then(|m| m.get("reasoningProfile"))
                        .map(|v| serde_json::from_value(v.clone()))
                        .transpose()
                        .map_err(invalid)?
                        .flatten(),
                })
            })
            .transpose()?;
        Ok(DefaultDocument {
            scope,
            document: path.display().to_string(),
            revision: revision(bytes.as_deref()),
            model: selection,
            approval_mode: model
                .get("approvalMode")
                .map(|v| serde_json::from_value(v.clone()))
                .transpose()
                .map_err(invalid)?,
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
        let candidate = update(original.as_deref().unwrap_or(b"{\n}\n"), &value)?;
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
        let bytes = crate::config_format::read_bounded(staged.path()).map_err(io_failure)?;
        // Reuse the canonical user-layer schema/ownership parser. This validates
        // the document, not readiness of any mutable project or resource files.
        super::launch::parse_layer(&target, &bytes, false).map_err(invalid)?;
        if let DefaultValue::ModelSelection { selection } = &value {
            let mut config = crate::model::session::SessionModelConfig::of(selection.model.clone());
            config
                .reasoning_profile
                .clone_from(&selection.reasoning_profile);
            crate::model::invocation::analyze_selection(
                self.models.model(&config.model).map_err(invalid)?,
                &config.selection(),
                crate::model::invocation::RequestParamsLayer::SessionOverrides,
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
        let models = include_str!("../../examples/local-runtime/minimal/models.jsonc");
        std::fs::write(host.config_directory.join("models.jsonc"), models).unwrap();
        std::fs::write(host.config_directory.join("settings.jsonc"), b"{\n // preserve my reason\n \"model\": {\"model\":\"example/demo-model\", /* keep profile comment */ \"reasoningProfile\":null},\n \"environment\": {\"PRIVATE\": \"SECRET_SENTINEL\"},\n \"defaultTools\": [\"read\"],\n}\n").unwrap();
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
        let project = workspace.join("project.jsonc");
        std::fs::write(&project, br#"{"model":{"model":"example/demo-model"}}"#).unwrap();
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
            std::fs::remove_file(owner.directory.join("models.jsonc")).unwrap();
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
            br#"{"unknown":true}"#.as_slice(),
            br#"{"workspace":"/forbidden"}"#,
            br#"{"context":{"unexpected":true}}"#,
            br#"{"model":{"model":"example/demo-model","maxOutputTokens":"wrong-type"}}"#,
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
        assert!(after.contains("// preserve my reason"));
        assert!(after.contains("/* keep profile comment */"));
        assert!(after.contains("\"PRIVATE\": \"SECRET_SENTINEL\""));
        let mut expected: serde_json::Value = crate::config_format::parse(&before).unwrap();
        expected["approvalMode"] = serde_json::json!("full_access");
        assert_eq!(
            expected,
            crate::config_format::parse::<serde_json::Value>(after.as_bytes()).unwrap()
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
        let external = b"{\"model\":{\"model\":\"example/demo-model\"},\"environment\":{\"PRIVATE\":\"EXTERNAL_SECRET\"}}";
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
        let duplicate = b"{\"model\":{\"model\":\"example/demo-model\"},\"environment\":{\"x\":\"SECRET_SENTINEL\",\"x\":\"second\"}}";
        std::fs::write(owner.target(), duplicate).unwrap();
        let error = owner
            .save_sync(DefaultScope::User, &revision(Some(duplicate)), approval())
            .unwrap_err();
        assert!(format!("{error:?}").contains("duplicate"));
        assert!(!format!("{error:?}").contains("SECRET_SENTINEL"));
        assert_eq!(std::fs::read(owner.target()).unwrap(), duplicate);
        let other = owner.target().with_file_name("other.jsonc");
        std::fs::rename(owner.target(), &other).unwrap();
        std::os::unix::fs::symlink(&other, owner.target()).unwrap();
        assert!(owner.read_sync(DefaultScope::User).is_err());
        assert_eq!(std::fs::read(other).unwrap(), duplicate);
    }
}
