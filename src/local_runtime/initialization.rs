//! Minimal configuration publication. Every destination is create-only.

use std::collections::BTreeMap;
use std::io::Write;
use std::path::{Path, PathBuf};

use serde::Serialize;
use serde_json::{Value, json};

use super::launch::HostEnvironment;
use crate::model::catalog::ModelCatalog;

pub(super) const VALUE_FLAGS: &[&str] = &[
    "--template",
    "--provider",
    "--model-id",
    "--endpoint",
    "--credential-env",
    "--context-window",
    "--max-output",
    "--tool-calls",
    "--reasoning",
    "--compat",
    "--model-document",
];

/// Exact publication outcome, including files published before a later failure.
#[derive(Debug, Serialize)]
pub struct InitializationResult {
    pub written: Vec<PathBuf>,
    pub conflicts: Vec<PathBuf>,
    pub failed: Option<PathBuf>,
    pub reason: Option<String>,
}

/// Build the two minimal documents from explicit declarations.
/// No model capability is inferred from identity or endpoint.
#[allow(clippy::too_many_lines)] // finite explicit template declarations, not an extensible wizard
pub(super) fn documents(arguments: &[String]) -> Result<[Vec<u8>; 2], String> {
    let mut options = BTreeMap::new();
    let mut arguments = arguments.iter();
    while let Some(flag) = arguments.next() {
        if !VALUE_FLAGS.contains(&flag.as_str()) {
            return Err("unknown initialization option; see rustx --help".into());
        }
        let value = arguments
            .next()
            .ok_or("initialization option requires a value")?;
        if options.insert(flag.as_str(), value.as_str()).is_some() {
            return Err("initialization options must not repeat".into());
        }
    }
    let required = |flag| {
        options
            .get(flag)
            .copied()
            .filter(|s| !s.is_empty())
            .ok_or_else(|| format!("init requires {flag}; see rustx --help"))
    };
    let provider = required("--provider")?;
    let credential = required("--credential-env")?;
    if !crate::credentials::valid_environment_name(credential) {
        return Err(
            "--credential-env requires an environment variable name, never a key value".into(),
        );
    }
    let template = required("--template")?;
    let model: Value = if template == "custom" {
        if options.keys().any(|key| {
            matches!(
                *key,
                "--model-id"
                    | "--context-window"
                    | "--max-output"
                    | "--tool-calls"
                    | "--reasoning"
                    | "--compat"
            )
        }) {
            return Err("custom uses --model-document for the complete model declaration".into());
        }
        crate::config_format::parse(&crate::config_format::read_bounded(Path::new(required(
            "--model-document",
        )?))?)
        .map_err(|_| "invalid custom model document".to_owned())?
    } else {
        if options.contains_key("--model-document") {
            return Err("--model-document requires --template custom".into());
        }
        let protocol = match template {
            "openai-chat" => "openai_chat_completions",
            "openai-responses" => "openai_responses",
            "anthropic" => "anthropic_messages",
            _ => {
                return Err(
                    "template must be openai-chat, openai-responses, anthropic, or custom".into(),
                );
            }
        };
        let number = |flag| {
            required(flag)?
                .parse::<u64>()
                .map_err(|_| format!("{flag} requires a positive integer"))
        };
        let boolean = |flag| {
            required(flag)?
                .parse::<bool>()
                .map_err(|_| format!("{flag} requires true or false"))
        };
        let mut model = json!({
            "id": required("--model-id")?, "protocol": protocol,
            "contextWindow": number("--context-window")?, "maxOutputTokens": number("--max-output")?,
            "capabilities": {"inputModalities":["text"], "outputModalities":["text"], "toolCalls":boolean("--tool-calls")?, "reasoning":boolean("--reasoning")?}
        });
        if template != "anthropic" || options.contains_key("--compat") {
            model["compat"] = crate::config_format::parse(required("--compat")?.as_bytes())
                .map_err(|_| "--compat must be a JSONC compatibility object".to_owned())?;
        }
        model
    };
    let id = model
        .get("id")
        .and_then(Value::as_str)
        .ok_or("model document requires id")?;
    let catalog = json!({"providers": {provider: {"baseUrl": required("--endpoint")?, "apiKey": format!("${credential}"), "models": [model]}}});
    let bytes = serde_json::to_vec_pretty(&catalog).map_err(|_| "cannot encode catalog")?;
    let parsed = ModelCatalog::from_jsonc_slice(&bytes).map_err(|_| "invalid model declaration; check protocol, limits, capabilities, and compatibility fields")?;
    let settings = json!({"model": {"model": format!("{provider}/{id}")}});
    let settings_bytes =
        serde_json::to_vec_pretty(&settings).map_err(|_| "cannot encode settings")?;
    let config = super::config::CurrentRuntimeConfig::from_jsonc_slice(&settings_bytes)
        .map_err(|_| "invalid model selection")?;
    let view = crate::model::invocation::analyze_selection(
        parsed
            .model(&config.model.model)
            .map_err(|_| "invalid model reference")?,
        &config.model.selection(),
        crate::model::invocation::RequestParamsLayer::SessionOverrides,
    )
    .map_err(|_| "invalid model selection")?;
    config
        .context_policy()
        .validate_budgets(
            (view.context_window, view.max_output_tokens),
            (view.context_window, view.max_output_tokens),
        )
        .map_err(|_| "declared model limits cannot fit native context budgets")?;
    Ok([bytes, settings_bytes])
}

pub(super) fn initialize(host: &HostEnvironment, documents: &[Vec<u8>; 2]) -> InitializationResult {
    publish(&host.config_directory, documents, |_, _| Ok(()))
}

/// The hook runs after complete preflight and before each publication. Tests
/// control races/failures here with channels rather than timing assumptions.
fn publish(
    directory: &Path,
    documents: &[Vec<u8>; 2],
    before_publish: impl FnMut(usize, &Path) -> std::io::Result<()>,
) -> InitializationResult {
    publish_with_writer(directory, documents, before_publish, |_, writer, bytes| {
        writer.write_all(bytes)?;
        writer.write_all(b"\n")
    })
}

fn publish_with_writer(
    directory: &Path,
    documents: &[Vec<u8>; 2],
    mut before_publish: impl FnMut(usize, &Path) -> std::io::Result<()>,
    mut write_staged: impl FnMut(usize, &mut std::fs::File, &[u8]) -> std::io::Result<()>,
) -> InitializationResult {
    let targets = [
        directory.join("models.jsonc"),
        directory.join("settings.jsonc"),
    ];
    let mut result = InitializationResult {
        written: Vec::new(),
        conflicts: Vec::new(),
        failed: None,
        reason: None,
    };
    for target in &targets {
        match std::fs::symlink_metadata(target) {
            Ok(_) => result.conflicts.push(target.clone()),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(_) => {
                result.failed = Some(target.clone());
                result.reason = Some("cannot inspect target".into());
                return result;
            }
        }
    }
    if !result.conflicts.is_empty() {
        result.reason = Some("existing configuration is never overwritten".into());
        return result;
    }
    if std::fs::create_dir_all(directory).is_err() {
        result.failed = Some(directory.into());
        result.reason = Some("cannot create user configuration directory".into());
        return result;
    }
    for (index, (target, bytes)) in targets.iter().zip(documents).enumerate() {
        let mut write = || -> std::io::Result<()> {
            let mut staged = tempfile::NamedTempFile::new_in(directory)?;
            write_staged(index, staged.as_file_mut(), bytes)?;
            staged.as_file().sync_all()?;
            before_publish(index, target)?;
            staged
                .persist_noclobber(target)
                .map_err(|error| error.error)?;
            Ok(())
        };
        if let Err(error) = (write)() {
            if error.kind() == std::io::ErrorKind::AlreadyExists {
                result.conflicts.push(target.clone());
            }
            result.failed = Some(target.clone());
            result.reason = Some("publication failed; previously written files remain; no existing file was overwritten".into());
            break;
        }
        result.written.push(target.clone());
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cfg235_partial_staging_write_failure_never_publishes_truncated_configuration() {
        let root = tempfile::tempdir().unwrap();
        let existing = root.path().join("unrelated.jsonc");
        std::fs::write(&existing, b"existing user content").unwrap();
        let result = publish_with_writer(
            root.path(),
            &[b"models".to_vec(), b"settings".to_vec()],
            |_, _| Ok(()),
            |index, file, bytes| {
                if index == 1 {
                    file.write_all(&bytes[..2])?;
                    return Err(std::io::Error::other("injected disk write failure"));
                }
                file.write_all(bytes)
            },
        );
        assert_eq!(result.written, [root.path().join("models.jsonc")]);
        assert_eq!(result.failed, Some(root.path().join("settings.jsonc")));
        assert!(!root.path().join("settings.jsonc").exists());
        assert_eq!(std::fs::read(existing).unwrap(), b"existing user content");
        assert_eq!(
            std::fs::read(root.path().join("models.jsonc")).unwrap(),
            b"models"
        );
        assert_eq!(std::fs::read_dir(root.path()).unwrap().count(), 2);
    }

    #[tokio::test]
    async fn cfg235_generated_init_launches_native_session_without_four_paths() {
        let root = tempfile::tempdir().unwrap();
        let workspace = root.path().join("workspace");
        std::fs::create_dir(&workspace).unwrap();
        let host =
            HostEnvironment::from_paths(workspace, root.path().join("home"), None, None).unwrap();
        let documents = documents(&declarations()).unwrap();
        assert_eq!(initialize(&host, &documents).written.len(), 2);
        let request = super::super::launch::LaunchRequest::default();
        super::super::launch::change_trust(
            &request,
            &host,
            super::super::launch::TrustAction::Grant,
        )
        .unwrap();
        let launch = super::super::launch::analyze(&request, &host)
            .unwrap()
            .admit(|| {
                crate::credentials::CredentialSnapshot::new([(
                    "RUSTX_TEST_KEY".into(),
                    "RUSTX_SECRET_SENTINEL_DO_NOT_LEAK".into(),
                )])
            })
            .unwrap();
        let product = super::super::LocalSessionProduct::compose(
            &launch,
            &super::super::LocalRuntimeDependencies::default(),
        )
        .await
        .unwrap();
        assert!(
            !launch
                .environment_store_root_for(product.runtime().conversation_id())
                .join("python-tools")
                .exists()
        );
        assert!(!launch.workspace.join("rustx.jsonc").exists());
        product.runtime().shutdown().await.unwrap();
    }

    #[test]
    fn cfg235_templates_are_explicit_and_deterministic() {
        for template in ["openai-chat", "openai-responses", "anthropic"] {
            let mut flags = declarations();
            flags[1] = template.into();
            if template == "openai-responses" {
                *flags.last_mut().unwrap() = "{}".into();
            }
            if template == "anthropic" {
                flags.truncate(flags.len() - 2);
            }
            let first = documents(&flags).unwrap();
            assert_eq!(first, documents(&flags).unwrap());
            assert!(ModelCatalog::from_jsonc_slice(&first[0]).is_ok());
            let output = String::from_utf8(first[0].clone()).unwrap();
            assert!(output.contains("$RUSTX_TEST_KEY"));
            assert!(!output.contains("RUSTX_SECRET_SENTINEL_DO_NOT_LEAK"));
        }
        let mut flags = declarations();
        flags.truncate(flags.len() - 2);
        assert!(
            documents(&flags).is_err(),
            "OpenAI compatibility is required explicitly"
        );
    }

    fn declarations() -> Vec<String> {
        [
            "--template",
            "openai-chat",
            "--provider",
            "local",
            "--model-id",
            "declared",
            "--endpoint",
            "http://127.0.0.1:9/v1",
            "--credential-env",
            "RUSTX_TEST_KEY",
            "--context-window",
            "128000",
            "--max-output",
            "4096",
            "--tool-calls",
            "true",
            "--reasoning",
            "false",
            "--compat",
            "{\"chatReasoningReplay\":\"omit\"}",
        ]
        .map(str::to_owned)
        .to_vec()
    }

    #[test]
    fn cfg235_minimal_init_validates_with_real_analysis_without_trust_or_sources() {
        let root = tempfile::tempdir().unwrap();
        let workspace = root.path().join("workspace");
        std::fs::create_dir(&workspace).unwrap();
        let host =
            HostEnvironment::from_paths(workspace.clone(), root.path().join("home"), None, None)
                .unwrap();
        let documents = documents(&declarations()).unwrap();
        let result = initialize(&host, &documents);
        assert_eq!(
            result.written,
            [
                host.config_directory.join("models.jsonc"),
                host.config_directory.join("settings.jsonc")
            ]
        );
        assert!(result.failed.is_none());
        let launch =
            super::super::launch::analyze(&super::super::launch::LaunchRequest::default(), &host)
                .unwrap();
        assert!(!launch.trusted);
        assert!(launch.config.mcp_servers.is_empty());
        assert!(launch.config.python_sources.is_empty());
        assert_eq!(launch.config.model.model.to_string(), "local/declared");
        assert!(!workspace.join("rustx.jsonc").exists());
        assert!(!host.state_directory.exists());
        let repeated = initialize(&host, &documents);
        assert!(repeated.written.is_empty());
        assert_eq!(repeated.conflicts, result.written);
        for (path, expected) in result.written.iter().zip(documents) {
            assert_eq!(
                std::fs::read(path).unwrap(),
                [expected, b"\n".to_vec()].concat()
            );
        }
    }

    #[test]
    fn cfg235_preflight_conflict_writes_nothing_and_never_enters_publication() {
        let root = tempfile::tempdir().unwrap();
        std::fs::write(root.path().join("settings.jsonc"), "existing").unwrap();
        let result = publish(root.path(), &[b"one".to_vec(), b"two".to_vec()], |_, _| {
            panic!("preflight must stop publication")
        });
        assert!(result.written.is_empty());
        assert!(!root.path().join("models.jsonc").exists());
        assert_eq!(
            std::fs::read_to_string(root.path().join("settings.jsonc")).unwrap(),
            "existing"
        );
    }

    #[test]
    fn cfg235_racing_creator_is_not_overwritten_and_partial_result_is_honest() {
        let root = tempfile::tempdir().unwrap();
        let (entered, reached) = std::sync::mpsc::channel();
        let (release, resume) = std::sync::mpsc::channel();
        std::thread::scope(|scope| {
            let directory = root.path();
            let writer = scope.spawn(move || {
                publish(
                    directory,
                    &[b"models".to_vec(), b"settings".to_vec()],
                    |index, path| {
                        if index == 1 {
                            entered.send(path.to_path_buf()).unwrap();
                            resume.recv().unwrap();
                        }
                        Ok(())
                    },
                )
            });
            let target = reached.recv().unwrap();
            std::fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&target)
                .unwrap()
                .write_all(b"racing creator")
                .unwrap();
            release.send(()).unwrap();
            let result = writer.join().unwrap();
            assert_eq!(result.written, [directory.join("models.jsonc")]);
            assert_eq!(result.failed, Some(target.clone()));
            assert_eq!(result.conflicts.as_slice(), std::slice::from_ref(&target));
            assert_eq!(std::fs::read(target).unwrap(), b"racing creator");
        });
    }

    #[test]
    fn cfg235_failed_publication_preserves_prior_content_and_staged_bytes_are_not_published() {
        let root = tempfile::tempdir().unwrap();
        let result = publish(
            root.path(),
            &[b"models".to_vec(), b"settings".to_vec()],
            |index, _| {
                if index == 1 {
                    Err(std::io::Error::other("injected publication failure"))
                } else {
                    Ok(())
                }
            },
        );
        assert_eq!(result.written, [root.path().join("models.jsonc")]);
        assert_eq!(
            std::fs::read(root.path().join("models.jsonc")).unwrap(),
            b"models\n"
        );
        assert!(!root.path().join("settings.jsonc").exists());
        assert!(result.failed.is_some());
        assert_eq!(std::fs::read_dir(root.path()).unwrap().count(), 1);
    }
}
