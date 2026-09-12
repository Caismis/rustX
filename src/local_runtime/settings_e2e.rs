//! One CFG dogfooding path through existing initialization, resolver, composition,
//! native admission gates, scripted provider, and Runtime Client fixtures.
use crate::model::catalog::{MapCredentialEnvironment, ModelRef};
use crate::model::invocation::ModelBindingRegistry;
use crate::model::session::SessionModelConfig;
use crate::model::{ModelAdapter, ModelEvent, ModelFinishReason};
use crate::runtime::conversation_runtime::Gate;
use crate::runtime_client::settings::{DefaultScope, DefaultTarget, SettingsBoundary};
use crate::runtime_client::types::{RequestId, RuntimeClientRequest, RuntimeClientResult};
use crate::runtime_client::{RUNTIME_CLIENT_PROTOCOL_VERSION, RuntimeClientEvent};
use crate::scripted_suites::support;
use serde_json::json;
use std::sync::Arc;

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[allow(clippy::too_many_lines)]
async fn cfg238_dogfood_distinct_owners_admission_requests_reload_save_and_reconnect() {
    let root = tempfile::tempdir().unwrap();
    let workspace = root.path().join("workspace");
    std::fs::create_dir(&workspace).unwrap();
    let host = super::launch::HostEnvironment::from_paths(
        workspace.clone(),
        root.path().join("home"),
        None,
        None,
    )
    .unwrap();
    let args = [
        "--template",
        "openai-chat",
        "--provider",
        "local",
        "--model-id",
        "a",
        "--endpoint",
        "http://127.0.0.1:9/v1",
        "--credential-env",
        "TEST_KEY",
        "--context-window",
        "128000",
        "--max-output",
        "4096",
        "--tool-calls",
        "true",
        "--reasoning",
        "false",
        "--compat",
        "chat_reasoning_replay = \"omit\"",
    ]
    .map(str::to_owned);
    let documents = super::initialization::documents(&args).unwrap();
    assert_eq!(
        super::initialization::initialize(&host, &documents)
            .written
            .len(),
        2
    );
    // Three explicit fixture models, sharing one scripted adapter. No provider heuristics.
    let mut catalog: serde_json::Value = crate::toml_authoring::parse(&documents[0]).unwrap();
    let mut model = catalog["providers"]["local"]["models"][0].clone();
    model["capabilities"]["reasoning"] = json!(true);
    model["reasoning"] = json!({"default_profile":"on","profiles": {
        "on":{"enabled":true,"request_params_json":r#"{"thinking":{"type":"enabled"}}"#},
        "off":{"enabled":false,"request_params_json":r#"{"thinking":{"type":"disabled"}}"#}
    }});
    catalog["providers"]["local"]["models"] = json!(["a", "b", "c"].map(|id| {
        let mut m = model.clone();
        m["id"] = json!(id);
        m
    }));
    std::fs::write(
        host.config_directory.join("models.toml"),
        toml::to_string_pretty(&catalog).unwrap(),
    )
    .unwrap();
    let user_path = host.config_directory.join("settings.toml");
    // The launch also authors a distinctive native Agent Extension
    // composition (Issue #256): every contributor field is non-default, so a
    // projection that substituted built-in defaults could not pass below.
    let settings = r#"# future default A; retain this comment
[model]
model = "local/a"
[extensions.agent_status.time]
timezone = "Asia/Shanghai"
[extensions.agent_status.background]
enabled = false
[environment]
PRIVATE = "SECRET_SENTINEL"
"#;
    std::fs::write(&user_path, settings).unwrap();
    let request = super::launch::LaunchRequest::default();
    super::launch::change_trust(&request, &host, super::launch::TrustAction::Grant).unwrap();
    let (report, _) = super::diagnostics::inspect("config_show", &request, &host);
    assert_eq!(report.validity, super::diagnostics::Validity::Valid);
    assert!(!report.render(true).contains("SECRET_SENTINEL"));
    let launch = super::launch::analyze(&request, &host)
        .unwrap()
        .admit(|| {
            crate::credentials::CredentialSnapshot::new([(
                "TEST_KEY".into(),
                "SECRET_SENTINEL".into(),
            )])
        })
        .unwrap();
    let (release, release_rx) = support::fake::model_release();
    let stop = || {
        vec![
            support::fake::FakeStep::Emit(ModelEvent::Started),
            support::fake::FakeStep::Emit(ModelEvent::Completed {
                finish_reason: ModelFinishReason::Stop,
                usage: None,
            }),
        ]
    };
    let mut first = vec![
        support::fake::FakeStep::Emit(ModelEvent::Started),
        support::fake::FakeStep::ParkUntilReleased(release_rx),
    ];
    first.extend(support::fake::tool_call_events(0, &support::fake::ScriptedCall {
        id: "cfg238-write", tool_id: "tool-write", name: "write",
        arguments: json!({"path":"must-not-be-created.txt", "content":"approval is required"}),
    }).into_iter().map(support::fake::FakeStep::Emit));
    first.push(support::fake::FakeStep::Emit(ModelEvent::Completed {
        finish_reason: ModelFinishReason::ToolCalls,
        usage: None,
    }));
    let fake = Arc::new(support::fake::FakeModel::new(vec![first, stop(), stop()]));
    let environment =
        MapCredentialEnvironment::new([("TEST_KEY".into(), "SECRET_SENTINEL".into())]);
    let registry = ModelBindingRegistry::new_with_scripted_adapters(
        launch.models.resolve(&environment).unwrap(),
        &support::model::ScriptedAdapterFactory::new(fake.clone() as Arc<dyn ModelAdapter>),
    )
    .unwrap();
    let controller = Arc::new(
        crate::runtime::local_storage::ProductController::acquire(&launch.runtime_root).unwrap(),
    );
    let core = super::composition::LocalConversationCore::compose_from_config(
        &launch,
        &super::LocalRuntimeDependencies::default(),
        registry,
        launch.config().clone(),
        super::session::SessionPersistentState {
            model: launch.config().initial_model().clone().clone(),
        },
        crate::runtime::identity::ConversationId::new("cfg238"),
        controller.root().join("artifacts"),
        controller,
    )
    .await
    .unwrap();
    let gate = Arc::new(Gate::default());
    core.runtime().install_admission_gate(gate.clone());
    let local = core.into_interactive().unwrap();
    let runtime = local.runtime();
    let (attachment, initialized) = local
        .host()
        .attach(RUNTIME_CLIENT_PROTOCOL_VERSION)
        .unwrap();
    let RuntimeClientResult::Initialized {
        snapshot: client_projection,
        ..
    } = initialized
    else {
        panic!("initialized")
    };
    assert_eq!(
        client_projection
            .model
            .as_ref()
            .unwrap()
            .configured
            .model
            .to_string(),
        "local/a"
    );
    // Issue #256: the frozen effective extension composition of this exact
    // launch, projected through the ordinary Runtime Client snapshot.
    let frozen_extensions = crate::runtime_client::settings::EffectiveNativeAgentExtensions {
        goal: None,
        agent_status: Some(
            crate::runtime_client::settings::EffectiveAgentStatusExtension {
                time: crate::runtime_client::settings::EffectiveTimeStatus {
                    enabled: true,
                    timezone: Some(chrono_tz::Asia::Shanghai),
                },
                background: crate::runtime_client::settings::EffectiveBackgroundStatus {
                    enabled: false,
                },
            },
        ),
        // Unauthored, and therefore composed: the Todo extension's default
        // lives in the closed extension document (Issue #259).
        todo: Some(crate::runtime_client::settings::EffectiveTodoExtension {}),
    };
    assert_eq!(
        client_projection.effective_extensions,
        Some(frozen_extensions.clone())
    );
    assert_eq!(
        client_projection.settings_lifetimes.extensions,
        SettingsBoundary::LaunchCapture
    );
    // Regression 3: the extension is composed and *no* Agent Status has been
    // composed for any step yet. Enablement never comes from observations.
    assert!(client_projection.statuses.is_empty());
    let subscription = attachment
        .subscribe_events(crate::runtime_client::RuntimeClientCursor::new(0))
        .unwrap();
    gate.arm();
    runtime
        .submit_inbound(vec![crate::message::types::UserContentBlock::Text(
            crate::message::content::TextBlock {
                text: "first".into(),
            },
        )])
        .unwrap();
    let wait_gate = gate.clone();
    tokio::task::spawn_blocking(move || wait_gate.wait_entered())
        .await
        .unwrap();
    let mut c = SessionModelConfig::of(ModelRef::parse("local/c").unwrap());
    c.reasoning_profile = Some(crate::model::catalog::ReasoningProfileId::parse("on").unwrap());
    runtime.model_set(c).unwrap(); // BEFORE the admission linearization
    gate.release();
    support::runtime_client_conformance::await_model_parked(&fake).await;
    assert_eq!(fake.requests()[0].model(), "c");
    assert_eq!(
        fake.requests()[0].request_params()["thinking"]["type"],
        "enabled"
    );
    let frozen_request = fake.requests()[0].clone();
    let mut b = SessionModelConfig::of(ModelRef::parse("local/b").unwrap());
    b.reasoning_profile = Some(crate::model::catalog::ReasoningProfileId::parse("off").unwrap());
    // Drain older observations, then prevent B's observation from being published.
    local.host().snapshot().unwrap();
    let probe = crate::runtime_client::test_sync::ProjectionProbe::default();
    local.host().install_projection_probe(probe.clone());
    probe.arm_publish();
    let response = attachment.handle_request(RuntimeClientRequest::ModelSet {
        id: RequestId::new(238),
        config: Box::new(b.clone()),
    });
    let Some(RuntimeClientResult::ModelSet { model }) = response.result else {
        panic!("model response")
    };
    assert_eq!(model.configured, b);
    let waiting = probe.clone();
    tokio::task::spawn_blocking(move || waiting.wait_publish_entered())
        .await
        .unwrap();
    assert_eq!(runtime.model_view().configured, b);
    // No event can reach/fold into the client's A projection at this cut.
    assert_eq!(
        client_projection
            .model
            .as_ref()
            .unwrap()
            .configured
            .model
            .to_string(),
        "local/a"
    );
    runtime
        .approval_mode_set(crate::runtime::ApprovalMode::FullAccess)
        .unwrap();
    // Disk read is separate, and doesn't replace the captured launch or live view.
    let read = attachment
        .handle_request_async(RuntimeClientRequest::DefaultsRead {
            id: RequestId::new(1),
            scope: DefaultScope::User,
        })
        .await;
    let Some(RuntimeClientResult::Defaults { document }) = read.result else {
        panic!("{read:?}")
    };
    assert_eq!(document.model.unwrap().model.to_string(), "local/a");
    let saved = attachment
        .handle_request_async(RuntimeClientRequest::DefaultSave {
            id: RequestId::new(2),
            scope: DefaultScope::User,
            expected_revision: document.revision,
            target: DefaultTarget::ModelSelection,
        })
        .await;
    let Some(RuntimeClientResult::DefaultSaved { result }) = saved.result else {
        panic!("{saved:?}")
    };
    assert!(result.live_unchanged);
    assert_eq!(runtime.model_view().configured, b);
    assert_eq!(
        std::fs::read_to_string(&user_path)
            .unwrap()
            .matches("retain this comment")
            .count(),
        1
    );
    assert!(
        !serde_json::to_string(&result)
            .unwrap()
            .contains("SECRET_SENTINEL")
    );
    probe.release_publish();
    // Saving approval captures desired FullAccess while this attempt retains Policy.
    let approval_saved = attachment
        .handle_request_async(RuntimeClientRequest::DefaultSave {
            id: RequestId::new(239),
            scope: DefaultScope::User,
            expected_revision: result.revision.clone(),
            target: DefaultTarget::ApprovalMode,
        })
        .await;
    let Some(RuntimeClientResult::DefaultSaved { result }) = approval_saved.result else {
        panic!("approval save")
    };
    let disk: serde_json::Value =
        crate::toml_authoring::parse(&std::fs::read(&user_path).unwrap()).unwrap();
    assert_eq!(disk["model"]["model"], "local/b");
    assert_eq!(disk["approval_mode"], "full_access");
    assert_eq!(
        runtime.approval_mode_state().effective,
        crate::runtime::ApprovalMode::Policy
    );
    assert_eq!(
        runtime.approval_mode_state().desired,
        crate::runtime::ApprovalMode::FullAccess
    );
    let (before, _) = local.host().snapshot().unwrap();
    assert_eq!(
        before
            .launch_settings
            .as_ref()
            .unwrap()
            .model
            .model
            .to_string(),
        "local/a"
    );
    assert_eq!(before.model.as_ref().unwrap().configured, b);
    let attempt = before.attempt.as_ref().unwrap();
    assert_eq!(
        attempt.model.as_ref().unwrap().primary.model.to_string(),
        "local/c"
    );
    let r1 = before.resources.revision;
    assert_eq!(
        attempt
            .execution_settings
            .as_ref()
            .unwrap()
            .resource_revision,
        r1
    );
    assert_eq!(
        before.pending_approval_mode,
        Some(crate::runtime::ApprovalMode::FullAccess)
    );
    assert_eq!(
        before.effective_approval_mode,
        crate::runtime::ApprovalMode::Policy
    );
    assert_eq!(
        before.settings_lifetimes.model,
        SettingsBoundary::NextAdmission
    );
    // Regression 3, the other direction: real Agent Status observations now
    // exist, and the effective-extension projection is bit-identical to the
    // one taken before any of them did.
    assert!(
        !before.statuses.is_empty(),
        "the composed Agent Status window really did fill"
    );
    assert_eq!(
        before.effective_extensions,
        Some(frozen_extensions.clone()),
        "composing statuses neither installs nor reconfigures an extension"
    );
    assert!(matches!(
        runtime.reload_resources().await,
        Err(crate::runtime::conversation_runtime::RuntimeResourceReloadError::Busy { .. })
    ));
    let (before_reconnect, _) = local.host().snapshot().unwrap();
    assert_eq!(before_reconnect.resources.revision, r1);
    let (observer, _) = local
        .host()
        .attach_read_only(RUNTIME_CLIENT_PROTOCOL_VERSION)
        .unwrap();
    let refused = observer
        .handle_request_async(RuntimeClientRequest::DefaultSave {
            id: RequestId::new(240),
            scope: DefaultScope::User,
            expected_revision: result.revision.clone(),
            target: DefaultTarget::ApprovalMode,
        })
        .await;
    assert!(matches!(
        refused.error,
        Some(crate::runtime_client::RuntimeClientError::InvalidState { .. })
    ));
    attachment.detach();
    let refused = attachment
        .handle_request_async(RuntimeClientRequest::DefaultSave {
            id: RequestId::new(241),
            scope: DefaultScope::User,
            expected_revision: result.revision.clone(),
            target: DefaultTarget::ModelSelection,
        })
        .await;
    assert!(matches!(
        refused.error,
        Some(crate::runtime_client::RuntimeClientError::NotAttached)
    ));
    let (reconnected, initialized) = local
        .host()
        .attach(RUNTIME_CLIENT_PROTOCOL_VERSION)
        .unwrap();
    let RuntimeClientResult::Initialized { snapshot, .. } = initialized else {
        panic!("initialized")
    };
    assert_eq!(snapshot, before_reconnect);
    // Regression 12: reconnect reconstructs the identical authoritative
    // effective-extension view. It is read from the same runtime-owned
    // projection, so nothing was rereplayed and no document was reopened.
    assert_eq!(
        snapshot.effective_extensions,
        Some(frozen_extensions.clone())
    );
    assert_eq!(fake.requests(), vec![frozen_request.clone()]);
    assert_eq!(
        result.revision,
        match reconnected
            .handle_request_async(RuntimeClientRequest::DefaultsRead {
                id: RequestId::new(3),
                scope: DefaultScope::User
            })
            .await
            .result
            .unwrap()
        {
            RuntimeClientResult::Defaults { document } => document.revision,
            _ => panic!("defaults"),
        }
    );
    assert!(
        !serde_json::to_string(&snapshot)
            .unwrap()
            .contains("SECRET_SENTINEL")
    );
    let sub = reconnected
        .subscribe_events(local.host().snapshot().unwrap().1)
        .unwrap();
    release.send_replace(true);
    // The already-admitted attempt still requires approval, despite desired
    // FullAccess. No executor may start while this native interaction is pending.
    let interaction = tokio::time::timeout(std::time::Duration::from_secs(30), async {
        loop {
            match sub.next().await {
                crate::runtime_client::EventDelivery::Event(event) => {
                    if let RuntimeClientEvent::InteractionPending { interaction } = event.event {
                        break interaction;
                    }
                }
                other => panic!("expected approval event: {other:?}"),
            }
        }
    })
    .await
    .expect("approval publication");
    assert!(!workspace.join("must-not-be-created.txt").exists());
    let (awaiting, _) = local.host().snapshot().unwrap();
    assert_eq!(
        awaiting.effective_approval_mode,
        crate::runtime::ApprovalMode::Policy
    );
    assert_eq!(
        awaiting.pending_approval_mode,
        Some(crate::runtime::ApprovalMode::FullAccess)
    );
    let response = reconnected
        .handle_request_async(RuntimeClientRequest::InteractionRespond {
            id: RequestId::new(4),
            interaction: interaction.interaction,
            response: crate::runtime::interaction::InteractionResponse::Approval {
                decision: crate::runtime::interaction::ApprovalDecision::Deny {
                    reason: "proof of frozen approval".into(),
                },
            },
        })
        .await;
    assert!(response.error.is_none(), "{response:?}");

    settled(&sub).await;
    let published = runtime.reload_resources().await.unwrap();
    assert_eq!(published.resource_revision, r1.next());
    let (after_reload, _) = local.host().snapshot().unwrap();
    assert_eq!(after_reload.resources.revision, published.resource_revision);
    // Regression 4: a *successful* resource publication advanced the
    // revision and left the effective extension composition untouched.
    assert_eq!(
        after_reload.effective_extensions,
        Some(frozen_extensions.clone()),
        "resource publication never recomposes a launch-scoped extension set"
    );
    assert_eq!(
        after_reload.attempt.as_ref().unwrap().model,
        before.attempt.as_ref().unwrap().model
    );
    assert_eq!(
        after_reload.attempt.as_ref().unwrap().execution_settings,
        before.attempt.as_ref().unwrap().execution_settings
    );
    assert_eq!(fake.requests()[0], frozen_request);
    std::fs::write(
        workspace.join("rustx.toml"),
        "[workflows]\ndefinitions = [\"missing\"]\n",
    )
    .unwrap();
    assert!(runtime.reload_resources().await.is_err());
    let (after_failure, _) = local.host().snapshot().unwrap();
    assert_eq!(
        after_failure.resources.revision,
        published.resource_revision
    );
    // A project document that disables the extension entirely, deliberately
    // left in place for the failed reload path and the fresh resolution at
    // the end of this test.
    std::fs::write(
        workspace.join("rustx.toml"),
        "[extensions.agent_status]\nenabled = false\n",
    )
    .unwrap();
    assert_eq!(
        local.host().snapshot().unwrap().0.effective_extensions,
        Some(frozen_extensions.clone()),
        "an edit to the document on disk cannot reach the running composition"
    );
    runtime
        .submit_inbound(vec![crate::message::types::UserContentBlock::Text(
            crate::message::content::TextBlock {
                text: "second".into(),
            },
        )])
        .unwrap();
    settled(&sub).await;
    assert_eq!(
        fake.requests()[1].model(),
        "c",
        "tool continuation keeps the admitted model"
    );
    assert_eq!(
        fake.requests()[1].request_params()["thinking"]["type"],
        "enabled"
    );
    assert!(!workspace.join("must-not-be-created.txt").exists());
    assert_eq!(fake.requests().len(), 3);
    assert_eq!(fake.requests()[2].model(), "b");
    assert_eq!(
        fake.requests()[2].request_params()["thinking"]["type"],
        "disabled"
    );
    let (next, _) = local.host().snapshot().unwrap();
    assert_eq!(
        next.attempt
            .unwrap()
            .execution_settings
            .unwrap()
            .resource_revision,
        published.resource_revision
    );
    assert_eq!(
        next.effective_approval_mode,
        crate::runtime::ApprovalMode::FullAccess
    );
    assert!(next.pending_approval_mode.is_none());
    drop(subscription);
    runtime.shutdown().await.unwrap();
    drop((attachment, observer, reconnected, sub));
    drop(local);
    // A fresh resolver and new native Session, not a restarted old execution.
    let next_launch = super::launch::analyze(&request, &host)
        .unwrap()
        .admit(|| launch.credentials.clone())
        .unwrap();
    // The prospective next launch is the other configuration surface, and it
    // legitimately disagrees: it reads the edited document and would compose
    // no Agent Status at all, while the runtime above kept projecting the
    // composition it was actually running.
    assert!(
        next_launch
            .config()
            .extension_composition()
            .agent_status()
            .is_none(),
        "the prospective next launch reads the edited extension configuration"
    );
    assert_eq!(next_launch.config().initial_model().clone().model, b.model);
    assert_eq!(
        next_launch
            .config()
            .initial_model()
            .clone()
            .reasoning_profile,
        b.reasoning_profile
    );
    let fresh = super::LocalSessionProduct::compose(
        &next_launch,
        &super::LocalRuntimeDependencies::default(),
    )
    .await
    .unwrap();
    assert_eq!(fresh.runtime().model_view().configured.model, b.model);
    assert_eq!(fake.requests().len(), 3);
    fresh.runtime().shutdown().await.unwrap();
}

async fn settled(subscription: &crate::runtime_client::EventSubscription) {
    tokio::time::timeout(std::time::Duration::from_secs(30), async {
        loop {
            match subscription.next().await {
                crate::runtime_client::EventDelivery::Event(event)
                    if matches!(event.event, RuntimeClientEvent::AttemptSettled { .. }) =>
                {
                    return;
                }
                crate::runtime_client::EventDelivery::Event(_) => {}
                other => panic!("expected live event: {other:?}"),
            }
        }
    })
    .await
    .expect("attempt settlement liveness");
}
