//! Configuration over the protocol: source and Session CAS, source authoring
//! independent of Session revisions, MCP authoring owners and bounded
//! source/settings/effective-configuration pages.
#![allow(clippy::large_futures)] // bounded fixture futures; no recursive or unbounded stack growth
use super::*;

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn web08_source_and_session_cas_cross_the_real_protocol_boundary() {
    use crate::local_runtime::configuration::settings::SourceMutation;
    use crate::model::{catalog::ModelRef, session::SessionModelConfig};
    bounded(async {
        let f = Fixture::new().await;
        // Authorized, canonical TOML; only full-runtime Workflow semantics fail.
        std::fs::write(
            f.workspaces[0].join("rustx.toml"),
            "[agent]\nworkflows = ['check', 'check']\n",
        )
        .unwrap();
        let connection = AppServerConnection::new(f.host.clone());
        initialize(&connection).await;
        let _id = f.sessions[0].id.clone();
        let MethodResult::SourceSettings {
            projection,
            ..
        } = call(
            &connection,
            1,
            Method::SourcesRead {
                target: crate::local_runtime::configuration::settings::SourceTarget::User,
            },
        )
        .await
        else {
            panic!()
        };
        let projection=projection.inline.expect("bounded fixture source");
        assert!(projection.user.authored.is_some());
        assert!(projection.workspace.is_none());
        assert!(projection.user.authored.as_ref().unwrap().models.as_ref().unwrap().contains_key("local/b"));
        let expected = projection.user.revision.clone();
        let selection = Some(SessionModelConfig::of(ModelRef::parse("local/b").unwrap()));
        let MethodResult::SourcesWritten { revision: written_revision, .. } = call(
            &connection,
            2,
            Method::SourcesWrite {
                target: crate::local_runtime::configuration::settings::SourceTarget::User,
                expected_revision: expected.clone(),
                mutation: SourceMutation::Config { mutation: crate::local_runtime::configuration::settings::ConfigMutation::RootModel {
                    authored: Some(
                        crate::local_runtime::authoring::ModelLayer {
                            model: Some(ModelRef::parse("local/b").unwrap()),
                            ..Default::default()
                        },
                    ),
                } },
            },
        )
        .await
        else {
            panic!()
        };
        let MethodResult::SourceSettings { projection: saved, .. } = call(
            &connection, 1002, Method::SourcesRead { target: crate::local_runtime::configuration::settings::SourceTarget::User },
        ).await else { panic!("explicit source reread after confirmed write") };
        let saved=saved.inline.expect("bounded fixture source");
        assert_eq!(saved.user.revision, written_revision);
        assert_ne!(saved.user.revision, expected);
        assert_eq!(
            saved.user.authored.as_ref().unwrap().agent.as_ref().unwrap().model.as_ref().unwrap().model,
            selection.as_ref().map(|s| s.model.clone())
        );
        assert!(matches!(
            rejected(
                &connection,
                Method::SourcesWrite {
                    target: crate::local_runtime::configuration::settings::SourceTarget::User,
                    expected_revision: expected,
                    mutation: SourceMutation::Config { mutation: crate::local_runtime::configuration::settings::ConfigMutation::RootModel { authored: None } }
                }
            )
            .await,
            ErrorData::SourceConflict {
                ..
            }
        ));
        let MethodResult::SourceSettings {
            projection: fresh, ..
        } = call(
            &connection,
            3,
            Method::SourcesRead {
                target: crate::local_runtime::configuration::settings::SourceTarget::User,
            },
        )
        .await
        else {
            panic!()
        };
        let fresh=fresh.inline.expect("bounded fixture source");
        assert_eq!(fresh.user, saved.user);
        let error = f
            .manager
            .configuration
            .resolve_session(
                &crate::local_runtime::configuration::SessionConfigInput::new(
                    f.workspaces[0].clone(),
                ),
            )
            .err()
            .unwrap()
            .to_string();
        assert!(error.contains("duplicate"), "{error}");
        assert!(f.provider.request_bodies().is_empty());
        f.close().await;
    })
    .await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn web08_catalog_commit_preserves_admitted_attempt_and_updates_cold_resolution() {
    use crate::local_runtime::configuration::settings::SourceMutation;
    bounded(Box::pin(async {
        let f = Fixture::new().await;
        let connection = AppServerConnection::new(f.host.clone());
        initialize(&connection).await;
        let initial = f
            .manager
            .source_settings(&crate::local_runtime::configuration::settings::SourceTarget::User)
            .await
            .unwrap();
        let mut model = initial
            .user
            .authored
            .as_ref()
            .unwrap()
            .models
            .as_ref()
            .unwrap()["local/a"]
            .clone();
        model.request_params = None;
        model.default_profile = Some(crate::model::catalog::ModelProfileId::new("old"));
        model.profiles = Some(
            ["old", "new"]
                .into_iter()
                .map(|name| {
                    (
                        crate::model::catalog::ModelProfileId::new(name),
                        crate::model::authoring::Profile {
                            reasoning_enabled: Some(false),
                            ..Default::default()
                        },
                    )
                })
                .collect(),
        );
        f.manager
            .write_source_settings(
                &crate::local_runtime::configuration::settings::SourceTarget::User,
                initial.user.revision,
                SourceMutation::Config {
                    mutation:
                        crate::local_runtime::configuration::settings::ConfigMutation::Model {
                            id: "local/a".into(),
                            authored: Some(model),
                        },
                },
            )
            .await
            .unwrap();
        let target = attach(&connection, &f, 0).await;
        if let Some(candidate) = f
            .manager
            .configuration_application(&target.session_id)
            .and_then(|application| application.candidate)
        {
            f.manager
                .adopt_configuration(
                    &target.session_id,
                    &candidate.identity,
                    candidate.expected_binding,
                )
                .unwrap();
        }
        call(
            &connection,
            101,
            Method::TurnStart {
                target: target.clone(),
                content: vec![UserInputBlock::Text(crate::message::content::TextBlock {
                    text: "request-A".into(),
                })],
            },
        )
        .await;
        f.gates[0].wait_entered().await;
        let runtime = f.manager.load(&target.session_id, None).await.unwrap();
        let before = runtime.client().checkpoint().unwrap().snapshot;
        let MethodResult::SourceSettings { projection, .. } = call(
            &connection,
            102,
            Method::SourcesRead {
                target: crate::local_runtime::configuration::settings::SourceTarget::User,
            },
        )
        .await
        else {
            panic!()
        };
        let projection = projection.inline.expect("bounded fixture source");
        let mut model = projection
            .user
            .authored
            .as_ref()
            .unwrap()
            .models
            .as_ref()
            .unwrap()["local/a"]
            .clone();
        model.max_output_tokens = 2048;
        // A Profile edit after admission: a new default whose complete
        // native object carries a value the admitted Attempt never froze.
        model.default_profile = Some(crate::model::catalog::ModelProfileId::new("new"));
        model.profiles.as_mut().unwrap().insert(
            crate::model::catalog::ModelProfileId::new("new"),
            crate::model::authoring::Profile {
                reasoning_enabled: Some(false),
                max_output_tokens: Some(1024),
                request_params: crate::toml_authoring::AuthoredRequestParams(
                    serde_json::from_value(serde_json::json!({"temperature": 0.8, "stop": [null]}))
                        .unwrap(),
                ),
            },
        );
        let MethodResult::SourcesWritten { .. } = call(
            &connection,
            103,
            Method::SourcesWrite {
                target: crate::local_runtime::configuration::settings::SourceTarget::User,
                expected_revision: projection.user.revision,
                mutation: SourceMutation::Config {
                    mutation:
                        crate::local_runtime::configuration::settings::ConfigMutation::Model {
                            id: "local/a".into(),
                            authored: Some(model),
                        },
                },
            },
        )
        .await
        else {
            panic!("confirmed native source-write receipt")
        };
        let after = runtime.client().checkpoint().unwrap().snapshot;
        assert_eq!(before.resources, after.resources);
        assert_eq!(before.capabilities, after.capabilities);
        assert_eq!(
            before.attempt.as_ref().unwrap().execution_settings,
            after.attempt.as_ref().unwrap().execution_settings
        );
        assert_eq!(before.model, after.model);
        assert_eq!(
            before.attempt.as_ref().unwrap().model,
            after.attempt.as_ref().unwrap().model
        );
        let prospective = f
            .manager
            .configuration
            .resolve_session(
                &crate::local_runtime::configuration::SessionConfigInput::new(
                    f.workspaces[0].clone(),
                ),
            )
            .unwrap();
        let next = prospective
            .models
            .model(&crate::model::catalog::ModelRef::parse("local/a").unwrap())
            .unwrap();
        let frozen = after.attempt.as_ref().unwrap().model.as_ref().unwrap();
        assert_eq!(next.id.as_str(), "a");
        assert_eq!(next.max_output_tokens, 2048);
        assert_eq!(frozen.primary.max_output_tokens, 4096);
        assert_eq!(next.default_profile.as_ref().unwrap().as_str(), "new");
        assert_eq!(frozen.primary.profile.as_ref().unwrap().as_str(), "old");
        let new = &next.profiles[&crate::model::catalog::ModelProfileId::new("new")];
        assert_eq!(new.request_params["temperature"], 0.8);
        assert_eq!(new.request_params["stop"][0], serde_json::Value::Null);
        assert_eq!(new.max_output_tokens, Some(1024));
        assert!(frozen.primary.request_params.is_empty());
        // Creation uses the successfully prepared source authority, after
        // native processing acknowledges availability. Existing A stays frozen.
        super::super::configuration::settled(&f, 0).await;
        let created = f
            .manager
            .create_session(crate::local_runtime::session::SessionPersistentState {
                cwd: f.workspaces[0].clone(),
                model: None,
            })
            .await
            .unwrap();
        let cold = attach_session(&connection, &created.session).await;
        let catalog = read_catalog_result(
            call(&connection, 104, Method::ModelCatalog { target: cold }).await,
        );
        assert_eq!(
            catalog
                .models
                .iter()
                .find(|m| m.model.to_string() == "local/a")
                .unwrap()
                .max_output_tokens,
            2048
        );
        f.gates[0].release();
        f.close().await;
    }))
    .await;
}

fn source_gate(
    f: &Fixture,
    point: &'static str,
) -> (
    tokio::sync::oneshot::Receiver<()>,
    std::sync::mpsc::Sender<()>,
) {
    let (entered, waiting) = tokio::sync::oneshot::channel();
    let (release, resume) = std::sync::mpsc::channel();
    f.manager.configuration.test_hooks.insert(point, move || {
        entered.send(()).unwrap();
        resume.recv().unwrap();
    });
    (waiting, release)
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn c12_source_read_is_independent_of_concurrent_session_revision() {
    bounded(async {
        let f = Fixture::new().await;
        let id = f.sessions[0].id.clone();
        let before = f
            .manager
            .source_settings(&crate::local_runtime::configuration::settings::SourceTarget::User)
            .await
            .unwrap();
        let revision = f
            .manager
            .sessions
            .catalog
            .lock()
            .await
            .settings_revision(&id)
            .unwrap();
        let document_lock =
            crate::local_runtime::settings::lock_document(std::path::Path::new(&before.user.path))
                .unwrap();
        let (entered, resume) = source_gate(&f, "before_documents");
        let manager = f.manager.clone();
        let _read_id = id.clone();
        let read = tokio::spawn(async move {
            manager
                .source_settings(&crate::local_runtime::configuration::settings::SourceTarget::User)
                .await
        });
        entered.await.unwrap();
        resume.send(()).unwrap();
        // Source worker is about to wait on this held document lock.
        // Unrelated catalog access and a durable same-Session commit both finish.
        let mut catalog = f.manager.sessions.catalog.lock().await;
        catalog.settings_revision(&f.sessions[1].id).unwrap();
        let (_, mut settings) = catalog.lineage(&id, None).unwrap();
        settings.model = Some(crate::model::session::SessionModelConfig::of(
            crate::model::catalog::ModelRef::parse("local/b").unwrap(),
        ));
        let next = catalog.replace_settings(&id, revision, settings).unwrap();
        drop(catalog);
        assert!(
            !read.is_finished(),
            "held document lock prevents source completion"
        );
        drop(document_lock);
        assert!(read.await.unwrap().is_ok());
        let after = f
            .manager
            .source_settings(&crate::local_runtime::configuration::settings::SourceTarget::User)
            .await
            .unwrap();
        let (current, settings) = f.manager.sessions.read_settings(&id).await.unwrap();
        let selected = settings.model;
        assert_eq!(current, next);
        assert_eq!(selected.unwrap().model.to_string(), "local/b");
        assert_eq!(after.user.revision, before.user.revision);
        f.close().await;
    })
    .await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn c12_source_commit_is_independent_of_concurrent_session_revision() {
    use crate::local_runtime::configuration::settings::SourceMutation;
    bounded(async {
        let f = Fixture::new().await;
        let id = f.sessions[0].id.clone();
        let before = f
            .manager
            .source_settings(&crate::local_runtime::configuration::settings::SourceTarget::User)
            .await
            .unwrap();
        let revision = f
            .manager
            .sessions
            .catalog
            .lock()
            .await
            .settings_revision(&id)
            .unwrap();
        let (entered, resume) = source_gate(&f, "before_publication");
        let mut model = before
            .user
            .authored
            .as_ref()
            .unwrap()
            .models
            .as_ref()
            .unwrap()["local/a"]
            .clone();
        model.max_output_tokens = 2048;
        let manager = f.manager.clone();
        let _write_id = id.clone();
        let write = tokio::spawn(async move {
            manager
                .write_source_settings(
                    &crate::local_runtime::configuration::settings::SourceTarget::User,
                    before.user.revision,
                    SourceMutation::Config {
                        mutation:
                            crate::local_runtime::configuration::settings::ConfigMutation::Model {
                                id: "local/a".into(),
                                authored: Some(model),
                            },
                    },
                )
                .await
        });
        entered.await.unwrap();
        let mut catalog = f.manager.sessions.catalog.lock().await;
        let (_, settings) = catalog.lineage(&id, None).unwrap();
        catalog.replace_settings(&id, revision, settings).unwrap();
        drop(catalog);
        resume.send(()).unwrap();
        assert!(write.await.unwrap().is_ok());
        let fresh = f
            .manager
            .source_settings(&crate::local_runtime::configuration::settings::SourceTarget::User)
            .await
            .unwrap();
        assert_eq!(
            fresh
                .user
                .authored
                .as_ref()
                .unwrap()
                .models
                .as_ref()
                .unwrap()["local/a"]
                .max_output_tokens,
            2048
        );
        f.close().await;
    })
    .await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn c12_mcp_source_commit_is_independent_of_session_revision() {
    use crate::local_runtime::configuration::settings::{McpWrite, SourceMutation};
    bounded(async {
        let f = Fixture::new().await;
        let id = f.sessions[0].id.clone();
        let connection = AppServerConnection::new(f.host.clone());
        initialize(&connection).await;
        let before = f
            .manager
            .source_settings(&crate::local_runtime::configuration::settings::SourceTarget::User)
            .await
            .unwrap();
        let revision = f
            .manager
            .sessions
            .catalog
            .lock()
            .await
            .settings_revision(&id)
            .unwrap();
        let mutation = SourceMutation::Mcp {
            id: crate::runtime::identity::McpServerId::new("fixture"),
            authored: Some(McpWrite {
                definition: serde_json::from_value(
                    serde_json::json!({"command":"never-start-unselected-fixture"}),
                )
                .unwrap(),
                retained_env: vec![],
                retained_headers: vec![],
            }),
        };
        let (entered, resume) = source_gate(&f, "before_publication");
        let manager = f.manager.clone();
        let _write_id = id.clone();
        let expected = before.user_mcp.revision.clone();
        let write = tokio::spawn(async move {
            manager
                .write_source_settings(
                    &crate::local_runtime::configuration::settings::SourceTarget::User,
                    expected,
                    mutation,
                )
                .await
        });
        entered.await.unwrap();
        let mut catalog = f.manager.sessions.catalog.lock().await;
        let (_, settings) = catalog.lineage(&id, None).unwrap();
        catalog.replace_settings(&id, revision, settings).unwrap();
        drop(catalog);
        resume.send(()).unwrap();
        assert!(write.await.unwrap().is_ok());
        let MethodResult::SourceSettings {
            projection: fresh, ..
        } = call(
            &connection,
            1,
            Method::SourcesRead {
                target: crate::local_runtime::configuration::settings::SourceTarget::User,
            },
        )
        .await
        else {
            panic!()
        };
        let fresh = fresh.inline.expect("bounded fixture source");
        assert!(
            fresh
                .user_mcp
                .authored
                .as_ref()
                .unwrap()
                .contains_key(&crate::runtime::identity::McpServerId::new("fixture"))
        );
        assert!(matches!(
            rejected(
                &connection,
                Method::SourcesWrite {
                    target: crate::local_runtime::configuration::settings::SourceTarget::User,
                    expected_revision: before.user_mcp.revision,
                    mutation: SourceMutation::Mcp {
                        id: crate::runtime::identity::McpServerId::new("fixture"),
                        authored: None
                    }
                }
            )
            .await,
            ErrorData::SourceConflict { .. }
        ));
        let MethodResult::SourcesWritten {
            revision: written_revision,
            ..
        } = call(
            &connection,
            2,
            Method::SourcesWrite {
                target: crate::local_runtime::configuration::settings::SourceTarget::User,
                expected_revision: fresh.user_mcp.revision,
                mutation: SourceMutation::Mcp {
                    id: crate::runtime::identity::McpServerId::new("fixture"),
                    authored: None,
                },
            },
        )
        .await
        else {
            panic!()
        };
        let MethodResult::SourceSettings {
            projection: deleted,
            ..
        } = call(
            &connection,
            1002,
            Method::SourcesRead {
                target: crate::local_runtime::configuration::settings::SourceTarget::User,
            },
        )
        .await
        else {
            panic!("explicit source reread after confirmed write")
        };
        let deleted = deleted.inline.expect("bounded fixture source");
        assert_eq!(deleted.user_mcp.revision, written_revision);
        assert!(deleted.user_mcp.authored.as_ref().unwrap().is_empty());
        assert!(f.provider.request_bodies().is_empty());
        f.close().await;
    })
    .await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn cfg332_workspace_mcp_definition_and_policy_have_independent_authoring_owners() {
    use crate::local_runtime::configuration::settings::{ConfigMutation, McpWrite, SourceMutation};
    use crate::runtime::identity::McpServerId;
    bounded(async {
        let f = Fixture::new().await;
        let _id = f.sessions[0].id.clone();
        let connection = AppServerConnection::new(f.host.clone());
        initialize(&connection).await;
        let before = f
            .manager
            .source_settings(
                &crate::local_runtime::configuration::settings::SourceTarget::Workspace {
                    directory: f.workspaces[0].clone(),
                },
            )
            .await
            .unwrap();
        let MethodResult::SourcesWritten {
            revision: written_revision,
            ..
        } = call(
            &connection,
            1,
            Method::SourcesWrite {
                target: crate::local_runtime::configuration::settings::SourceTarget::Workspace {
                    directory: f.workspaces[0].clone(),
                },
                expected_revision: before.workspace_mcp.as_ref().unwrap().revision.clone(),
                mutation: SourceMutation::Mcp {
                    id: McpServerId::new("service"),
                    authored: Some(McpWrite {
                        definition: serde_json::from_value(
                            serde_json::json!({"command":"never-start-unselected-fixture"}),
                        )
                        .unwrap(),
                        retained_env: vec![],
                        retained_headers: vec![],
                    }),
                },
            },
        )
        .await
        else {
            panic!("source projection")
        };
        let MethodResult::SourceSettings {
            projection: definition,
            ..
        } = call(
            &connection,
            1001,
            Method::SourcesRead {
                target: crate::local_runtime::configuration::settings::SourceTarget::Workspace {
                    directory: f.workspaces[0].clone(),
                },
            },
        )
        .await
        else {
            panic!("explicit source reread after confirmed write")
        };
        let definition = definition.inline.expect("bounded fixture source");
        assert_eq!(
            definition.workspace_mcp.as_ref().unwrap().revision,
            written_revision
        );
        assert_eq!(
            definition.workspace.as_ref().unwrap().revision,
            before.workspace.as_ref().unwrap().revision
        );
        let MethodResult::SourcesWritten {
            revision: written_revision,
            ..
        } = call(
            &connection,
            2,
            Method::SourcesWrite {
                target: crate::local_runtime::configuration::settings::SourceTarget::Workspace {
                    directory: f.workspaces[0].clone(),
                },
                expected_revision: definition.workspace.as_ref().unwrap().revision.clone(),
                mutation: SourceMutation::Config {
                    mutation: ConfigMutation::McpPolicy {
                        id: McpServerId::new("service"),
                        authored: Some(crate::local_runtime::config::InvocationPolicyDocument {
                            approval: crate::local_runtime::config::ApprovalPolicyDocument::Always,
                            ..Default::default()
                        }),
                    },
                },
            },
        )
        .await
        else {
            panic!("policy projection")
        };
        let MethodResult::SourceSettings {
            projection: policy, ..
        } = call(
            &connection,
            1002,
            Method::SourcesRead {
                target: crate::local_runtime::configuration::settings::SourceTarget::Workspace {
                    directory: f.workspaces[0].clone(),
                },
            },
        )
        .await
        else {
            panic!("explicit source reread after confirmed write")
        };
        let policy = policy.inline.expect("bounded fixture source");
        assert_eq!(
            policy.workspace.as_ref().unwrap().revision,
            written_revision
        );
        assert_eq!(
            policy.workspace_mcp.as_ref().unwrap().revision,
            definition.workspace_mcp.as_ref().unwrap().revision
        );
        assert_ne!(
            policy.workspace.as_ref().unwrap().revision,
            before.workspace.as_ref().unwrap().revision
        );
        assert_eq!(policy.user.revision, before.user.revision);
        let target = attach(&connection, &f, 0).await;
        let MethodResult::EffectiveConfiguration { projection } =
            call(&connection, 3, Method::ConfigurationGet { target }).await
        else {
            panic!("native effective projection")
        };
        let projection = projection.inline.expect("bounded fixture configuration");
        assert!(projection.root_agent.tools.sources.is_empty());
        assert!(f.provider.request_bodies().is_empty());
        connection.close();
        f.close().await;
    })
    .await;
}

/// T15: the source configuration owner has bounded projections, advancing
/// aggregate pages, and independent exact large-field reads at one source cut.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn t15_source_fields_and_many_small_revisions_page_with_exact_cut_evidence() {
    bounded(async {
        use crate::local_runtime::configuration::{observation as source, settings::SourceTarget};
        use base64::Engine;
        let f = Fixture::new().await;
        let mut native = f
            .manager
            .source_settings(&SourceTarget::User)
            .await
            .unwrap();
        native.prospective_diagnostic = Some("é\n".repeat(140_000));
        for index in 0..3000 {
            native.resource_revisions.insert(
                std::path::PathBuf::from(format!("resource-{index:05}")),
                "revision".repeat(8),
            );
        }
        native.resource_revisions.insert(
            std::path::PathBuf::from("resource-zzzz"),
            "x".repeat(40_000),
        );
        let projection = source::bounded(&native).unwrap();
        assert!(projection.inline.is_none());
        let mut reconstructed = serde_json::Map::new();
        for field in &projection.fields {
            let value = if let Some(value) = &field.value {
                value.clone()
            } else {
                let reference = field.reference.as_ref().unwrap();
                let mut bytes = Vec::new();
                let mut offset = 0;
                loop {
                    let chunk =
                        source::read(&native, reference, offset, source::READ_BYTES).unwrap();
                    bytes.extend(
                        base64::engine::general_purpose::STANDARD
                            .decode(&chunk.data_base64)
                            .unwrap(),
                    );
                    let Some(next) = chunk.next_offset else {
                        break;
                    };
                    assert!(next > offset);
                    offset = next;
                }
                serde_json::from_slice(&bytes).unwrap()
            };
            let serde_json::Value::String(name) = serde_json::to_value(field.name).unwrap() else {
                panic!("field name");
            };
            assert!(reconstructed.insert(name, value).is_none());
        }
        let reconstructed: crate::local_runtime::configuration::settings::SourceSettings =
            crate::runtime_client::structural_json::decode(serde_json::Value::Object(
                reconstructed,
            ))
            .unwrap();
        assert_eq!(reconstructed, native);
        let response = Response::Success(Box::new(Success {
            jsonrpc: JsonRpcVersion::V2,
            id: RequestId::Integer(315),
            result: MethodResult::SourceSettings {
                projection: Box::new(projection.clone()),
            },
        }));
        assert!(serde_json::to_vec(&response).unwrap().len() < 786_432);
        let reference = projection
            .fields
            .iter()
            .find(|field| field.name == source::SourceFieldName::ProspectiveDiagnostic)
            .unwrap()
            .reference
            .as_ref()
            .unwrap();
        let mut offset = 0;
        let mut bytes = vec![];
        loop {
            let chunk = source::read(&native, reference, offset, source::READ_BYTES).unwrap();
            let decoded = base64::engine::general_purpose::STANDARD
                .decode(&chunk.data_base64)
                .unwrap();
            assert!(decoded.len() <= 65_536);
            assert!(chunk.end_offset > offset);
            bytes.extend(decoded);
            let Some(next) = chunk.next_offset else {
                break;
            };
            offset = next;
        }
        assert_eq!(
            serde_json::from_slice::<String>(&bytes).unwrap(),
            native.prospective_diagnostic.as_deref().unwrap()
        );
        assert!(source::read(&native, reference, reference.byte_length + 1, 1).is_err());
        let mut offset = 0;
        let mut seen = 0;
        loop {
            let page = source::page(
                &native,
                &projection.revision,
                source::SourceFieldName::ResourceRevisions,
                offset,
                32,
            )
            .unwrap();
            assert!(page.items.len() <= 32);
            assert!(serde_json::to_vec(&page).unwrap().len() < 786_432);
            for item in &page.items {
                assert_eq!(item.index, seen);
                seen += 1;
                if let Some(reference) = &item.reference {
                    let chunk = source::read(&native, reference, 0, 65_536).unwrap();
                    let entry: serde_json::Value = serde_json::from_slice(
                        &base64::engine::general_purpose::STANDARD
                            .decode(chunk.data_base64)
                            .unwrap(),
                    )
                    .unwrap();
                    assert_eq!(entry[0], "resource-zzzz");
                    assert_eq!(entry[1], "x".repeat(40_000));
                }
            }
            let Some(next) = page.next_offset else {
                break;
            };
            assert!(next > offset);
            offset = next;
        }
        assert_eq!(
            seen,
            u64::try_from(native.resource_revisions.len()).unwrap()
        );
        native.prospective_diagnostic.as_mut().unwrap().push('x');
        assert_eq!(
            source::read(&native, reference, 0, 1).unwrap_err(),
            "source read cut expired"
        );
        native.prospective_diagnostic = None;
        native.resource_revisions.clear();
        for index in 0..33 {
            native
                .resource_revisions
                .insert(std::path::PathBuf::from(format!("r{index}")), "v".into());
        }
        assert!(
            serde_json::to_vec(&native.resource_revisions)
                .unwrap()
                .len()
                < 16_384
        );
        let small_many = source::bounded(&native).unwrap();
        assert!(
            small_many.inline.is_none(),
            "count budget applies below whole-view byte budget"
        );
        let field = small_many
            .fields
            .iter()
            .find(|field| field.name == source::SourceFieldName::ResourceRevisions)
            .unwrap();
        assert!(
            field.value.is_none(),
            "count budget applies below per-field byte budget"
        );
        assert!(field.reference.as_ref().unwrap().byte_length < 16_384);
        let page = source::page(
            &native,
            &small_many.revision,
            source::SourceFieldName::ResourceRevisions,
            0,
            32,
        )
        .unwrap();
        assert_eq!(page.items.len(), 32);
        assert_eq!(page.next_offset, Some(32));
        f.close().await;
    })
    .await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn t15_cold_settings_deferred_value_advances_with_exact_catalog_revision() {
    use crate::local_runtime::configuration::observation as source;
    use base64::Engine;
    bounded(async {
        let f = Fixture::new().await;
        let (revision, mut settings) = f
            .manager
            .sessions
            .read_settings(&f.sessions[0].id)
            .await
            .unwrap();
        settings.model = Some(crate::model::session::SessionModelConfig::of(
            crate::model::catalog::ModelRef::parse("local/a").unwrap(),
        ));
        settings.model.as_mut().unwrap().request_params.insert(
            "large".into(),
            serde_json::Value::String("é\n".repeat(700_000)),
        );
        let projection = source::session_settings(revision, &settings).unwrap();
        assert!(projection.inline.is_none());
        let reference = projection.reference.as_ref().unwrap();
        assert!(serde_json::to_vec(&projection).unwrap().len() < 786_432);
        let mut bytes = Vec::new();
        let mut offset = 0;
        loop {
            let chunk =
                source::session_settings_read(revision, &settings, reference, offset, 65_536)
                    .unwrap();
            let decoded = base64::engine::general_purpose::STANDARD
                .decode(chunk.data_base64)
                .unwrap();
            assert!(decoded.len() <= 65_536);
            assert!(chunk.end_offset > offset);
            bytes.extend(decoded);
            let Some(next) = chunk.next_offset else {
                break;
            };
            offset = next;
        }
        assert_eq!(
            crate::runtime_client::structural_json::decode::<
                crate::local_runtime::session::SessionPersistentState,
            >(serde_json::from_slice(&bytes).unwrap())
            .unwrap(),
            settings
        );
        assert_eq!(
            source::session_settings_read(revision + 1, &settings, reference, 0, 1).unwrap_err(),
            "source read cut expired"
        );
        let mut modified = reference.clone();
        modified.byte_length += 1;
        assert!(source::session_settings_read(revision, &settings, &modified, 0, 1).is_err());
        assert!(
            source::session_settings_read(
                revision,
                &settings,
                reference,
                reference.byte_length + 1,
                1
            )
            .is_err()
        );
        f.close().await;
    })
    .await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn t15_effective_configuration_preserves_all_fields_and_advancing_pages() {
    use crate::local_runtime::configuration::effective_observation as source;
    use base64::Engine;
    bounded(async {
        let mut phases = FixturePhase::new("effective-configuration-pages");
        phases.enter("fixture");
        let f = Fixture::new().await;
        phases.enter("load");
        f.manager.load(&f.sessions[0].id, None).await.unwrap();
        phases.enter("native effective configuration");
        let mut native = f
            .manager
            .effective_configuration_observation(&f.sessions[0].id)
            .unwrap();
        native.effective_model.configured.request_params.insert(
            "large".into(),
            serde_json::Value::String("é\n".repeat(300_000)),
        );
        for index in 0..3000 {
            native.source_revisions.insert(
                std::path::PathBuf::from(format!("source-{index:05}")),
                "revision".repeat(8),
            );
        }
        phases.enter("bounded projection");
        let projection = source::bounded(&native).unwrap();
        assert!(projection.inline.is_none());
        assert_eq!(projection.fields.len(), 18);
        assert!(serde_json::to_vec(&projection).unwrap().len() < 786_432);
        phases.enter("field reconstruction");
        let mut reconstructed = serde_json::Map::new();
        for field in &projection.fields {
            let value = if let Some(value) = &field.value {
                value.clone()
            } else {
                let reference = field.reference.as_ref().unwrap();
                let mut bytes = Vec::new();
                let mut offset = 0;
                loop {
                    let chunk = source::read(&native, reference, offset, 65_536).unwrap();
                    bytes.extend(
                        base64::engine::general_purpose::STANDARD
                            .decode(chunk.data_base64)
                            .unwrap(),
                    );
                    let Some(next) = chunk.next_offset else {
                        break;
                    };
                    assert!(next > offset);
                    offset = next;
                }
                serde_json::from_slice(&bytes).unwrap()
            };
            let serde_json::Value::String(name) = serde_json::to_value(field.name).unwrap() else {
                panic!("field");
            };
            assert!(reconstructed.insert(name, value).is_none());
        }
        let reconstructed: crate::local_runtime::configuration::settings::EffectiveConfiguration =
            crate::runtime_client::structural_json::decode(serde_json::Value::Object(
                reconstructed,
            ))
            .unwrap();
        assert_eq!(reconstructed, native);
        phases.enter("source revisions pages");
        let mut offset = 0;
        let mut seen = 0;
        loop {
            let page = source::page(
                &native,
                &projection.revision,
                source::EffectiveFieldName::SourceRevisions,
                offset,
                32,
            )
            .unwrap();
            assert!(page.items.len() <= 32);
            assert!(serde_json::to_vec(&page).unwrap().len() < 786_432);
            seen += page.items.len();
            let Some(next) = page.next_offset else {
                break;
            };
            assert!(next > offset);
            offset = next;
        }
        assert_eq!(seen, native.source_revisions.len());
        phases.enter("expired and small collection evidence");
        let reference = projection
            .fields
            .iter()
            .find(|field| field.name == source::EffectiveFieldName::EffectiveModel)
            .unwrap()
            .reference
            .as_ref()
            .unwrap();
        native.adopted_binding += 1;
        assert_eq!(
            source::read(&native, reference, 0, 1).unwrap_err(),
            "source read cut expired"
        );
        native.effective_model.configured.request_params.clear();
        native.source_revisions.clear();
        for index in 0..33 {
            native
                .source_revisions
                .insert(std::path::PathBuf::from(format!("r{index}")), "v".into());
        }
        assert!(serde_json::to_vec(&native.source_revisions).unwrap().len() < 16_384);
        let small_many = source::bounded(&native).unwrap();
        assert!(small_many.inline.is_none());
        let field = small_many
            .fields
            .iter()
            .find(|field| field.name == source::EffectiveFieldName::SourceRevisions)
            .unwrap();
        assert!(field.value.is_none());
        assert!(field.reference.as_ref().unwrap().byte_length < 16_384);
        let page = source::page(
            &native,
            &small_many.revision,
            source::EffectiveFieldName::SourceRevisions,
            0,
            32,
        )
        .unwrap();
        assert_eq!(page.items.len(), 32);
        assert_eq!(page.next_offset, Some(32));
        phases.enter("close");
        f.close().await;
        phases.finish();
    })
    .await;
}
