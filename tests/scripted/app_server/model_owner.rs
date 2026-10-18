//! Durable Session selection has one owner across every native client handle.
#![allow(clippy::large_futures)]

use super::*;
use crate::local_runtime::configuration::settings::SourceTarget;
use crate::model::catalog::ModelRef;
use crate::model::session::SessionModelConfig;
use crate::runtime::conversation_runtime::ModelUpdateError;
use crate::runtime_client::{
    RUNTIME_CLIENT_PROTOCOL_VERSION, RuntimeClientError, RuntimeClientRequest,
};

fn selection(model: &str) -> SessionModelConfig {
    SessionModelConfig::of(ModelRef::parse(model).unwrap())
}

async fn assert_authority(f: &Fixture, expected: &SessionModelConfig) {
    let id = &f.sessions[0].id;
    let (_, settings) = f.manager.sessions.read_settings(id).await.unwrap();
    assert_eq!(settings.model.as_ref(), Some(expected));
    let reopened = crate::local_runtime::session::SessionCatalog::open_existing(&f.archive_root)
        .unwrap()
        .unwrap();
    assert_eq!(
        reopened.lineage(id, None).unwrap().1.model.as_ref(),
        Some(expected)
    );
    let retained = f.manager.sessions.configuration_bindings.lock().unwrap()[id].clone();
    let runtime = f.manager.configuration_runtime(id).unwrap();
    assert_eq!(retained.session_model(), expected);
    assert_eq!(runtime.model_view().configured, *expected);
    assert_eq!(
        runtime.configuration_view().unwrap().adopted_binding,
        retained.binding_revision
    );
    assert_eq!(
        f.manager
            .configuration_host(id)
            .unwrap()
            .checkpoint()
            .unwrap()
            .snapshot
            .model
            .unwrap()
            .configured,
        *expected
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn session_owned_model_selection_rejects_runtime_client_bypass_without_divergence() {
    bounded(async {
        let f = Fixture::with_session_count(None, 1).await;
        let id = &f.sessions[0].id;
        f.manager.load(id, None).await.unwrap();
        f.manager.set_model(id, selection("local/a")).await.unwrap();
        f.manager
            .reconcile_configuration(&SourceTarget::Workspace {
                directory: f.workspaces[0].clone(),
            })
            .await
            .unwrap();
        super::configuration::settled(&f, 0).await;
        let runtime = f.manager.configuration_runtime(id).unwrap();
        let host = f.manager.configuration_host(id).unwrap();
        let (attachment, _) = host.attach(RUNTIME_CLIENT_PROTOCOL_VERSION).unwrap();
        let settings = f.manager.sessions.read_settings(id).await.unwrap();
        let catalog = std::fs::read(f.archive_root.join("sessions/catalog.json")).unwrap();
        let retained = || {
            let bindings = f.manager.sessions.configuration_bindings.lock().unwrap();
            let binding = &bindings[id];
            (binding.binding_revision, binding.session_model().clone())
        };
        let binding = retained();
        let resources = runtime.runtime_resources();
        let view = runtime.model_view();
        let configuration = runtime.configuration_view().unwrap();
        let application = f.manager.configuration_application(id);
        let checkpoint = host.checkpoint().unwrap();
        let (projection, cursor) = (checkpoint.snapshot, checkpoint.cursor);

        assert_eq!(
            runtime.model_set(selection("local/b")),
            Err(ModelUpdateError::SessionOwned)
        );
        assert!(matches!(
            host.model_set(selection("local/b")),
            Err(RuntimeClientError::InvalidState { .. })
        ));
        assert!(matches!(
            attachment.model_set(selection("local/b")),
            Err(RuntimeClientError::InvalidState { .. })
        ));
        let response = attachment.handle_request(RuntimeClientRequest::ModelSet {
            id: crate::runtime_client::types::RequestId::new(1),
            config: Box::new(selection("local/b")),
        });
        assert!(matches!(
            response.error,
            Some(RuntimeClientError::InvalidState { .. })
        ));
        assert!(response.result.is_none());
        assert_eq!(
            f.manager.sessions.read_settings(id).await.unwrap(),
            settings
        );
        assert_eq!(
            std::fs::read(f.archive_root.join("sessions/catalog.json")).unwrap(),
            catalog
        );
        assert_eq!(retained(), binding);
        assert!(Arc::ptr_eq(&runtime.runtime_resources(), &resources));
        assert_eq!(runtime.model_view(), view);
        assert_eq!(runtime.configuration_view().unwrap(), configuration);
        assert_eq!(f.manager.configuration_application(id), application);
        let checkpoint = host.checkpoint().unwrap();
        assert_eq!(
            (checkpoint.snapshot, checkpoint.cursor),
            (projection, cursor)
        );
        drop(attachment);

        let connection = crate::app_server::connection::AppServerConnection::new(f.host.clone());
        super::protocol::initialize(&connection).await;
        let target = super::protocol::attach(&connection, &f, 0).await;
        let selected = super::protocol::call(
            &connection,
            20,
            crate::app_server::protocol::Method::ModelSet {
                target: target.clone(),
                config: Box::new(selection("local/b")),
            },
        )
        .await;
        assert!(matches!(
            selected,
            crate::app_server::protocol::MethodResult::ModelSet { .. }
        ));
        super::configuration::settled(&f, 0).await;
        assert_authority(&f, &selection("local/b")).await;

        // A subsequent catalog publication must not meet an untracked runtime
        // revision. Adding an unselected model publishes without context adoption.
        let path = &f.manager.configuration.sources.config_path;
        let mut source: toml::Value =
            toml::from_str(&std::fs::read_to_string(path).unwrap()).unwrap();
        let mut added = source["models"]["local/a"].clone();
        added["id"] = "c".into();
        source["models"]
            .as_table_mut()
            .unwrap()
            .insert("local/c".into(), added);
        std::fs::write(path, toml::to_string(&source).unwrap()).unwrap();
        f.manager
            .reconcile_configuration(&SourceTarget::User)
            .await
            .unwrap();
        let published = super::configuration::settled(&f, 0).await;
        assert!(published.candidate.is_none());
        assert!(
            runtime
                .model_catalog()
                .models
                .iter()
                .any(|model| model.model.to_string() == "local/c")
        );
        assert_authority(&f, &selection("local/b")).await;
        super::protocol::call(
            &connection,
            21,
            crate::app_server::protocol::Method::ModelSet {
                target,
                config: Box::new(selection("local/c")),
            },
        )
        .await;
        super::configuration::settled(&f, 0).await;
        assert_authority(&f, &selection("local/c")).await;
        assert!(f.provider.request_bodies().is_empty());
        drop(connection);
        f.close().await;
    })
    .await;
}
