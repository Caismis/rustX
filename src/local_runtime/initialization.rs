//! Minimal configuration publication. Every destination is create-only.

use std::collections::BTreeMap;
use std::io::Write;
use std::path::{Path, PathBuf};

use crate::model::authoring::{Capabilities, Catalog, Compat, Model, Provider};
use serde::Serialize;

use super::launch::HostEnvironment;
use crate::model::catalog::ModelCatalog;

/// Native initialization declarations, independent of command-line parsing.
#[derive(Debug)]
pub struct InitializationRequest {
    pub template: Template,
    pub provider: String,
    pub endpoint: String,
    pub credential_env: String,
    pub model_id: Option<String>,
    pub context_window: Option<u64>,
    pub max_output: Option<u32>,
    pub tool_calls: Option<bool>,
    pub reasoning: Option<bool>,
    pub compat: Option<String>,
    pub model_document: Option<PathBuf>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Template {
    OpenaiChat,
    OpenaiResponses,
    Anthropic,
    Custom,
}

/// Exact publication outcome, including files published before a later failure.
#[derive(Debug, Serialize)]
pub struct InitializationResult {
    pub written: Vec<PathBuf>,
    pub conflicts: Vec<PathBuf>,
    pub failed: Option<PathBuf>,
    pub reason: Option<String>,
}

/// Build the minimal CFG3 document from explicit declarations.
/// No model capability is inferred from identity or endpoint.
#[allow(clippy::too_many_lines)] // finite explicit template declarations, not an extensible wizard
pub(super) fn documents(request: &InitializationRequest) -> Result<[Vec<u8>; 1], String> {
    let provider = request.provider.as_str();
    let credential = request.credential_env.as_str();
    if !crate::credentials::valid_environment_name(credential) {
        return Err(
            "--credential-env requires an environment variable name, never a key value".into(),
        );
    }
    let model: Model = if request.template == Template::Custom {
        if request.model_id.is_some()
            || request.context_window.is_some()
            || request.max_output.is_some()
            || request.tool_calls.is_some()
            || request.reasoning.is_some()
            || request.compat.is_some()
        {
            return Err("custom uses --model-document for the complete model declaration".into());
        }
        let path = request
            .model_document
            .as_ref()
            .ok_or("custom requires --model-document")?;
        crate::toml_authoring::parse(&crate::bounded_file::read_bounded(path)?)
            .map_err(|_| "invalid custom model document".to_owned())?
    } else {
        if request.model_document.is_some() {
            return Err("--model-document requires --template custom".into());
        }
        let protocol = match request.template {
            Template::OpenaiChat => crate::model::ModelProtocol::OpenAiChatCompletions,
            Template::OpenaiResponses => crate::model::ModelProtocol::OpenAiResponses,
            Template::Anthropic => crate::model::ModelProtocol::AnthropicMessages,
            Template::Custom => unreachable!(),
        };
        let compat = if request.template != Template::Anthropic || request.compat.is_some() {
            crate::toml_authoring::parse::<Compat>(
                request
                    .compat
                    .as_ref()
                    .ok_or("init requires --compat")?
                    .as_bytes(),
            )
            .map_err(|_| "--compat must be a TOML compatibility document".to_owned())?
        } else {
            Compat::default()
        };
        Model {
            provider: provider.into(),
            id: request.model_id.clone().ok_or("init requires --model-id")?,
            protocol,
            context_window: request
                .context_window
                .ok_or("init requires --context-window")?,
            max_output_tokens: request.max_output.ok_or("init requires --max-output")?,
            capabilities: Capabilities {
                input_modalities: [crate::model::catalog::Modality::Text].into(),
                output_modalities: [crate::model::catalog::Modality::Text].into(),
                tool_calls: request.tool_calls.ok_or("init requires --tool-calls")?,
                reasoning: request.reasoning.ok_or("init requires --reasoning")?,
            },
            request_params: crate::toml_authoring::RequestParamsToml::default(),
            reasoning: None,
            compat,
        }
    };
    let selected = crate::model::catalog::ModelRef::parse(&format!("{provider}/{}", model.id))
        .map_err(|_| "invalid model reference")?;
    let catalog = Catalog {
        schema_version: crate::model::catalog::MODEL_CATALOG_SCHEMA_VERSION,
        models: BTreeMap::from([(selected.to_string(), model)]),
        providers: BTreeMap::from([(
            provider.into(),
            Provider {
                base_url: request.endpoint.clone(),
                api_key: crate::model::catalog::CredentialSource::parse(
                    &format!("${credential}"),
                    &crate::model::catalog::ProviderId::new(provider),
                )
                .map_err(|_| "invalid credential reference")?,
            },
        )]),
    };
    let bytes = toml::to_string_pretty(&catalog)
        .map_err(|_| "cannot encode catalog")?
        .into_bytes();
    let parsed = ModelCatalog::from_toml_slice(&bytes).map_err(|_| "invalid model declaration; check protocol, limits, capabilities, and compatibility fields")?;
    let settings = super::authoring::RuntimeLayer::<_, super::authoring::AuthoredEnvironment> {
        providers: Some(catalog.providers),
        models: Some(catalog.models),
        agent: Some(super::authoring::AgentProfileLayer {
            model: Some(super::authoring::ModelLayer {
                model: Some(selected),
                ..Default::default()
            }),
            ..Default::default()
        }),
        ..Default::default()
    };
    let settings_bytes = toml::to_string_pretty(&settings)
        .map_err(|_| "cannot encode settings")?
        .into_bytes();
    let config = super::config::CurrentRuntimeConfig::from_toml_slice(&settings_bytes)
        .map_err(|_| "invalid model selection")?;
    let view = crate::model::invocation::analyze_selection(
        parsed
            .model(&config.initial_model().model)
            .map_err(|_| "invalid model reference")?,
        &config.initial_model().selection(),
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
    Ok([settings_bytes])
}

pub(super) fn initialize(host: &HostEnvironment, documents: &[Vec<u8>; 1]) -> InitializationResult {
    let mut result = publish(&host.config_directory, documents, |_, _| Ok(()));
    if result.failed.is_some() || !result.conflicts.is_empty() {
        return result;
    }
    let agents = host.home_directory.join("rustx/.agents");
    for name in ["skills", "tools", "agents", "workflows"] {
        let path = agents.join(name);
        if let Err(error) = std::fs::create_dir_all(&path) {
            result.failed = Some(path);
            result.reason = Some(error.to_string());
            return result;
        }
    }
    let mcp = agents.join("mcp.toml");
    match std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&mcp)
    {
        Ok(mut file) => {
            if let Err(error) = file
                .write_all(b"[mcp_servers]\n")
                .and_then(|()| file.sync_all())
            {
                result.failed = Some(mcp);
                result.reason = Some(error.to_string());
            } else {
                result.written.push(mcp);
            }
        }
        Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {}
        Err(error) => {
            result.failed = Some(mcp);
            result.reason = Some(error.to_string());
        }
    }
    result
}

/// The hook runs after complete preflight and before each publication. Tests
/// control races/failures here with channels rather than timing assumptions.
fn publish(
    directory: &Path,
    documents: &[Vec<u8>; 1],
    before_publish: impl FnMut(usize, &Path) -> std::io::Result<()>,
) -> InitializationResult {
    publish_with_writer(directory, documents, before_publish, |_, writer, bytes| {
        writer.write_all(bytes)?;
        writer.write_all(b"\n")
    })
}

fn publish_with_writer(
    directory: &Path,
    documents: &[Vec<u8>; 1],
    mut before_publish: impl FnMut(usize, &Path) -> std::io::Result<()>,
    mut write_staged: impl FnMut(usize, &mut std::fs::File, &[u8]) -> std::io::Result<()>,
) -> InitializationResult {
    let targets = [directory.join("rustx.toml")];
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
            let _lock = super::settings::lock_document(target)?;
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
        let existing = root.path().join("unrelated.toml");
        std::fs::write(&existing, b"existing user content").unwrap();
        let result = publish_with_writer(
            root.path(),
            &[b"configuration".to_vec()],
            |_, _| Ok(()),
            |index, file, bytes| {
                if index == 0 {
                    file.write_all(&bytes[..2])?;
                    return Err(std::io::Error::other("injected disk write failure"));
                }
                file.write_all(bytes)
            },
        );
        assert!(result.written.is_empty());
        assert_eq!(result.failed, Some(root.path().join("rustx.toml")));
        assert!(!root.path().join("rustx.toml").exists());
        assert_eq!(std::fs::read(existing).unwrap(), b"existing user content");
        assert_eq!(std::fs::read_dir(root.path()).unwrap().count(), 2); // unrelated content and persistent writer lock
    }

    #[tokio::test]
    async fn cfg235_generated_init_launches_native_session_without_four_paths() {
        let root = tempfile::tempdir().unwrap();
        let workspace = root.path().join("workspace");
        std::fs::create_dir(&workspace).unwrap();
        let host = HostEnvironment::from_paths(workspace, root.path().join("home")).unwrap();
        let documents = documents(&declarations()).unwrap();
        assert_eq!(initialize(&host, &documents).written.len(), 2);
        let request = super::super::launch::LaunchRequest::default();
        let launch = super::super::launch::analyze(&request, &host)
            .unwrap()
            .admit(|| {
                crate::credentials::CredentialSnapshot::new([(
                    "RUSTX_TEST_KEY".into(),
                    "RUSTX_SECRET_SENTINEL_DO_NOT_LEAK".into(),
                )])
            })
            .unwrap();
        let product = super::super::LocalSessionClient::compose(
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
        assert!(!launch.workspace.join("rustx.toml").exists());
        product.runtime().shutdown().await.unwrap();
    }

    #[test]
    fn cfg235_templates_are_explicit_and_deterministic() {
        for template in [
            Template::OpenaiChat,
            Template::OpenaiResponses,
            Template::Anthropic,
        ] {
            let mut flags = declarations();
            flags.template = template;
            if template == Template::OpenaiResponses {
                flags.compat = Some(String::new());
            }
            if template == Template::Anthropic {
                flags.compat = None;
            }
            let first = documents(&flags).unwrap();
            assert_eq!(first, documents(&flags).unwrap());
            assert!(
                crate::toml_authoring::parse::<super::super::authoring::RuntimeLayer>(&first[0])
                    .is_ok()
            );
            let output = String::from_utf8(first[0].clone()).unwrap();
            assert!(output.contains("$RUSTX_TEST_KEY"));
            assert!(!output.contains("RUSTX_SECRET_SENTINEL_DO_NOT_LEAK"));
        }
        let mut flags = declarations();
        flags.compat = None;
        assert!(
            documents(&flags).is_err(),
            "OpenAI compatibility is required explicitly"
        );
    }

    fn declarations() -> InitializationRequest {
        InitializationRequest {
            template: Template::OpenaiChat,
            provider: "local".into(),
            endpoint: "http://127.0.0.1:9/v1".into(),
            credential_env: "RUSTX_TEST_KEY".into(),
            model_id: Some("declared".into()),
            context_window: Some(128_000),
            max_output: Some(4096),
            tool_calls: Some(true),
            reasoning: Some(false),
            compat: Some("chat_reasoning_replay = \"omit\"".into()),
            model_document: None,
        }
    }

    #[test]
    fn cli05_native_custom_model_and_declaration_policy() {
        let root = tempfile::tempdir().unwrap();
        let original = declarations();
        let bytes = documents(&original).unwrap();
        let authored: super::super::authoring::RuntimeLayer =
            crate::toml_authoring::parse(&bytes[0]).unwrap();
        let model = authored.models.unwrap().into_values().next().unwrap();
        let path = root.path().join("model.toml");
        std::fs::write(&path, toml::to_string(&model).unwrap()).unwrap();
        let mut custom = InitializationRequest {
            template: Template::Custom,
            model_id: None,
            context_window: None,
            max_output: None,
            tool_calls: None,
            reasoning: None,
            compat: None,
            model_document: Some(path.clone()),
            ..declarations()
        };
        assert_eq!(documents(&custom).unwrap(), bytes);
        custom.tool_calls = Some(false);
        assert!(
            documents(&custom)
                .unwrap_err()
                .contains("complete model declaration")
        );
        custom.tool_calls = None;
        custom.model_document = None;
        assert!(documents(&custom).unwrap_err().contains("--model-document"));
        for mutation in 0..6 {
            let mut request = declarations();
            match mutation {
                0 => request.credential_env = "literal-key-value".into(),
                1 => request.endpoint = "not a URL".into(),
                2 => request.context_window = Some(0),
                3 => request.max_output = Some(0),
                4 => request.compat = None,
                _ => request.model_document = Some(path.clone()),
            }
            assert!(documents(&request).is_err(), "mutation {mutation}");
        }
    }

    #[test]
    fn cfg235_minimal_init_validates_with_real_analysis_without_trust_or_sources() {
        let root = tempfile::tempdir().unwrap();
        let workspace = root.path().join("workspace");
        std::fs::create_dir(&workspace).unwrap();
        let host =
            HostEnvironment::from_paths(workspace.clone(), root.path().join("home")).unwrap();
        let documents = documents(&declarations()).unwrap();
        let catalog = std::str::from_utf8(&documents[0]).unwrap();
        assert!(catalog.contains("request_params"));
        assert!(!catalog.contains("request_params_json"));
        let authored: super::super::authoring::RuntimeLayer =
            crate::toml_authoring::parse(&documents[0]).unwrap();
        assert!(
            authored
                .models
                .as_ref()
                .unwrap()
                .values()
                .next()
                .unwrap()
                .request_params
                .0
                .is_empty()
        );

        let result = initialize(&host, &documents);
        assert_eq!(
            result.written,
            [
                host.config_directory.join("rustx.toml"),
                host.home_directory.join("rustx/.agents/mcp.toml")
            ]
        );
        assert!(result.failed.is_none());
        let launch =
            super::super::launch::analyze(&super::super::launch::LaunchRequest::default(), &host)
                .unwrap();
        assert!(launch.config.mcp_servers.is_empty());
        assert!(launch.managed_python.packages().is_empty());
        assert_eq!(
            launch.config.initial_model().model.to_string(),
            "local/declared"
        );
        assert!(!workspace.join("rustx.toml").exists());
        assert!(!host.state_directory.exists());
        let repeated = initialize(&host, &documents);
        assert!(repeated.written.is_empty());
        assert_eq!(
            repeated.conflicts,
            [host.config_directory.join("rustx.toml")]
        );
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
        std::fs::write(root.path().join("rustx.toml"), "existing").unwrap();
        let result = publish(root.path(), &[b"configuration".to_vec()], |_, _| {
            panic!("preflight must stop publication")
        });
        assert!(result.written.is_empty());
        assert!(!root.path().join("models.toml").exists());
        assert_eq!(
            std::fs::read_to_string(root.path().join("rustx.toml")).unwrap(),
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
                publish(directory, &[b"configuration".to_vec()], |index, path| {
                    if index == 0 {
                        entered.send(path.to_path_buf()).unwrap();
                        resume.recv().unwrap();
                    }
                    Ok(())
                })
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
            assert!(result.written.is_empty());
            assert_eq!(result.failed, Some(target.clone()));
            assert_eq!(result.conflicts.as_slice(), std::slice::from_ref(&target));
            assert_eq!(std::fs::read(target).unwrap(), b"racing creator");
        });
    }

    #[test]
    fn cfg235_failed_publication_preserves_prior_content_and_staged_bytes_are_not_published() {
        let root = tempfile::tempdir().unwrap();
        let result = publish(root.path(), &[b"configuration".to_vec()], |index, _| {
            if index == 0 {
                Err(std::io::Error::other("injected publication failure"))
            } else {
                Ok(())
            }
        });
        assert!(result.written.is_empty());
        assert!(!root.path().join("rustx.toml").exists());
        assert!(result.failed.is_some());
        assert_eq!(std::fs::read_dir(root.path()).unwrap().count(), 1);
    }
}
