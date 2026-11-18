//! Skill content preparation must not hold the process-wide application lock.
#![allow(clippy::large_futures)]
use super::*;
use crate::local_runtime::configuration::application::ApplicationRead;
use crate::local_runtime::configuration::settings::{
    EffectiveConfiguration, ResourceTransfer, SourceMutation, SourceTarget, TransferOperation,
};
use crate::runtime::resources::SourceScope;
use std::sync::{Arc, Mutex};

fn same_effective(before: &EffectiveConfiguration, during: &EffectiveConfiguration) -> bool {
    before.adopted_binding == during.adopted_binding
        && before.generation == during.generation
        && before.source_revisions == during.source_revisions
        && before.application == during.application
        && before.process_bindings == during.process_bindings
        && before.session_model == during.session_model
        && before.effective_model == during.effective_model
        && before.document == during.document
        && before.approval_mode == during.approval_mode
        && before.provenance == during.provenance
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn skill_content_preparation_lets_another_loaded_session_read_configuration() {
    bounded(async {
        let fixture = Fixture::with_session_count(None, 1).await;
        // Provider replies are header-gated. Release them before load so
        // session startup cannot park this regression on an unrelated gate.
        fixture.gates[0].release();
        fixture.gates[1].release();
        let loaded = fixture.load(0).await.unwrap().unwrap();
        let session_id = fixture.sessions[0].id.clone();
        assert!(
            fixture.manager.configuration_runtime(&session_id).is_some(),
            "the other session must already be loaded"
        );
        // This fixture loads a runtime without publishing a session application
        // view. The optional view that exists now is the baseline; do not wait
        // for a configuration change this setup never initiates.
        let baseline_application = fixture.manager.configuration_application(&session_id);
        let baseline = fixture
            .manager
            .effective_configuration_observation(&session_id)
            .expect("loaded session has an effective configuration");
        assert_eq!(baseline.application, baseline_application);
        let settings = fixture
            .manager
            .source_settings(&SourceTarget::User)
            .await
            .unwrap();
        let collection = settings.user_resource_root.join("skills").join("original");
        std::fs::create_dir_all(&collection).unwrap();
        std::fs::write(
            collection.join("SKILL.md"),
            "---\nname: original\ndescription: Original skill\n---\nInstructions\n",
        )
        .unwrap();
        std::fs::write(collection.join("body.txt"), "payload").unwrap();
        let other = settings.user_resource_root.join("skills").join("other");
        std::fs::create_dir_all(&other).unwrap();
        std::fs::write(
            other.join("SKILL.md"),
            "---\nname: other\ndescription: Other skill\n---\nOther\n",
        )
        .unwrap();
        let other_revision = fixture
            .manager
            .configuration
            .skill_operation_revision(&SourceTarget::User, "other")
            .expect("other skill revision");
        let revision = fixture
            .manager
            .configuration
            .skill_operation_revision(&SourceTarget::User, "original")
            .expect("prepared skill revision");
        let workspace = SourceTarget::Workspace {
            directory: fixture.workspaces[0].clone(),
        };
        let (entered, waiting) = tokio::sync::oneshot::channel();
        let (release, resume) = std::sync::mpsc::channel();
        let order = Arc::new(Mutex::new(Vec::new()));
        let preparing = order.clone();
        fixture
            .manager
            .configuration
            .test_hooks
            .insert("skill_content_preparing", move || {
                preparing.lock().unwrap().push("preparing");
                entered.send(()).unwrap();
                resume.recv().unwrap();
                preparing.lock().unwrap().push("resumed");
            });
        let manager = fixture.manager.clone();
        let mut transfer = tokio::spawn(async move {
            manager
                .write_source_settings(
                    &workspace,
                    revision,
                    SourceMutation::ResourceTransfer {
                        from: SourceScope::User,
                        operation: TransferOperation::Copy,
                        resource: ResourceTransfer::Skill {
                            source: "original".into(),
                            destination: "copied".into(),
                        },
                    },
                )
                .await
        });
        tokio::select! {
            entered = waiting => entered.expect("content preparation gate dropped before entry"),
            finished = &mut transfer => panic!(
                "skill transfer finished before content preparation gate: {finished:?}"
            ),
        };
        assert!(
            !matches!(
                fixture
                    .manager
                    .try_next_configuration_application(&std::collections::BTreeMap::new()),
                ApplicationRead::Busy
            ),
            "content preparation must not hold the application lock"
        );
        let during_application = fixture.manager.configuration_application(&session_id);
        assert_eq!(
            during_application, baseline_application,
            "the loaded session's application read changed while content was only being prepared"
        );
        let during = fixture
            .manager
            .effective_configuration_observation(&session_id)
            .expect("loaded session config read completes during skill content preparation");
        assert!(
            fixture.manager.configuration_runtime(&session_id).is_some(),
            "content preparation must not drop the loaded session"
        );
        assert!(
            same_effective(&baseline, &during),
            "the loaded session's effective configuration changed during content preparation"
        );
        let removal = fixture.manager.clone();
        let removed = tokio::spawn(async move {
            removal
                .write_source_settings(
                    &SourceTarget::User,
                    other_revision,
                    SourceMutation::SkillDelete {
                        name: "other".into(),
                    },
                )
                .await
        });
        removed
            .await
            .unwrap()
            .expect("deleting another skill must not wait for skill content staging");
        assert!(
            fixture
                .manager
                .configuration
                .skill_operation_revision(&SourceTarget::User, "other")
                .is_none(),
            "the concurrent delete must finish before content preparation resumes"
        );
        order.lock().unwrap().push("read");
        release.send(()).unwrap();
        transfer
            .await
            .unwrap()
            .expect("skill copy commits after the other session read");
        assert_eq!(
            order.lock().unwrap().as_slice(),
            ["preparing", "read", "resumed"],
            "the other session's configuration read completed before content preparation resumed"
        );
        drop(loaded);
        fixture.close().await;
    })
    .await;
}
