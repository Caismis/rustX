//! Residency races at the real manager, native composition, provider and
//! allocation boundaries. Timeouts are liveness guards, never race evidence.
#![allow(clippy::too_many_lines)]
#[path = "../../support/app_server_conformance.rs"]
mod app_server_conformance;
mod compaction_ownership;
mod configuration;
mod inbound_model;
mod model_owner;
mod model_selection;
mod model_selection_noop;
pub(crate) mod protocol;
mod residency_policy;
mod source_repair;
mod startup_measurements;
mod transports;
use super::*;
use crate::events::types::RuntimeEvent;
use crate::local_runtime::configuration::SessionConfigInput;
use crate::local_runtime::launch::{self, HostEnvironment, LaunchRequest};
use crate::local_runtime::session::{SessionPersistentState, SessionSnapshot};
use crate::message::types::UserContentBlock;
use crate::scripted_suites::common::{FixtureReply, FixtureServer, HeaderGate};
use std::sync::atomic::{AtomicUsize, Ordering};
mod deletion;
mod fixture;
mod load;
mod replacement;
mod schedule;
#[cfg(target_os = "linux")]
mod schedule_benchmark;
mod session_claim;
mod session_title;
/// One user text block, from the shared scripted-suite constructors.
use crate::scripted_suites::support::runtime_client_conformance::text as input;
use fixture::{Fixture, FixturePhase, bounded, events, gate_entered};

#[derive(Debug)]
pub(super) struct AsyncGate {
    entered: watch::Sender<bool>,
    /// Callers waiting at the gate now: arrived, and neither released nor
    /// dropped. A count, so a test can wait for every party it sent.
    parked: watch::Sender<usize>,
    release: watch::Sender<bool>,
}

impl Default for AsyncGate {
    fn default() -> Self {
        Self {
            entered: watch::channel(false).0,
            parked: watch::channel(0).0,
            release: watch::channel(true).0,
        }
    }
}

impl AsyncGate {
    fn arm(&self) {
        self.entered.send_replace(false);
        self.release.send_replace(false);
    }
    pub(super) async fn park(&self) {
        struct Leave<'a>(&'a watch::Sender<usize>);
        impl Drop for Leave<'_> {
            fn drop(&mut self) {
                self.0.send_modify(|parked| *parked -= 1);
            }
        }
        self.parked.send_modify(|parked| *parked += 1);
        let _leave = Leave(&self.parked);
        self.entered.send_replace(true);
        self.release.subscribe().wait_for(|v| *v).await.unwrap();
    }
    async fn entered(&self) {
        self.entered.subscribe().wait_for(|v| *v).await.unwrap();
    }
    /// Waits until exactly `count` callers are parked at the gate.
    async fn parked(&self, count: usize) {
        self.parked
            .subscribe()
            .wait_for(|parked| *parked == count)
            .await
            .unwrap();
    }
    fn parked_now(&self) -> usize {
        *self.parked.borrow()
    }
    fn release(&self) {
        self.release.send_replace(true);
    }
}

#[derive(Debug)]
pub(super) struct Probe {
    pub(super) schedule_native_read: AsyncGate,
    pub(super) schedule_writer_busy: watch::Sender<bool>,
    pub(super) schedule_read_return: AsyncGate,
    pub(super) pause_schedule_read_return: std::sync::atomic::AtomicBool,
    pub(super) storage_joined: watch::Sender<usize>,
    pub(super) fail_storage_accept_once: std::sync::atomic::AtomicBool,
    pub(super) fail_storage_close_once: std::sync::atomic::AtomicBool,
    pub(super) panic_storage_once: std::sync::atomic::AtomicBool,
    pub(super) storage_registered: AsyncGate,
    pub(super) storage_before_open: AsyncGate,
    pub(super) storage_before_commit: AsyncGate,
    pub(super) storage_after_commit: AsyncGate,
    pub(super) storage_before_close: AsyncGate,
    pub(super) storage_after_release: AsyncGate,

    pub(super) configuration_preparations: AtomicUsize,
    pub(super) fail_configuration_once: std::sync::atomic::AtomicBool,
    pub(super) before_configuration_prepare: AsyncGate,
    pub(super) before_configuration_publish: AsyncGate,
    pub(super) before_catalog_publication: AsyncGate,
    pub(super) idle_before_claim: Arc<crate::runtime::conversation_runtime::Gate>,
    pub(super) idle_after_claim: Arc<crate::runtime::conversation_runtime::Gate>,
    pub(super) activation: Mutex<Option<Arc<crate::runtime::conversation_runtime::Gate>>>,
    pub(super) compositions: AtomicUsize,
    pub(super) acquisitions: AtomicUsize,
    pub(super) before_allocation: AsyncGate,
    pub(super) after_writer_transfer: AsyncGate,
    pub(super) joined: watch::Sender<usize>,
    pub(super) unloads_joined: watch::Sender<usize>,
    pub(super) fail_compose_once: std::sync::atomic::AtomicBool,
    pub(super) panic_once: std::sync::atomic::AtomicBool,
    pub(super) before_compose: AsyncGate,
    pub(super) after_delete_fence: AsyncGate,
    pub(super) before_shutdown: AsyncGate,
    pub(super) before_operation: AsyncGate,
    pub(super) draining_operations: watch::Sender<bool>,
}

impl Default for Probe {
    fn default() -> Self {
        Self {
            schedule_native_read: AsyncGate::default(),
            schedule_writer_busy: watch::channel(false).0,
            schedule_read_return: AsyncGate::default(),
            pause_schedule_read_return: std::sync::atomic::AtomicBool::new(false),
            storage_joined: watch::channel(0).0,
            fail_storage_accept_once: std::sync::atomic::AtomicBool::new(false),
            fail_storage_close_once: std::sync::atomic::AtomicBool::new(false),
            panic_storage_once: std::sync::atomic::AtomicBool::new(false),
            storage_registered: AsyncGate::default(),
            storage_before_open: AsyncGate::default(),
            storage_before_commit: AsyncGate::default(),
            storage_after_commit: AsyncGate::default(),
            storage_before_close: AsyncGate::default(),
            storage_after_release: AsyncGate::default(),

            configuration_preparations: AtomicUsize::new(0),
            fail_configuration_once: std::sync::atomic::AtomicBool::new(false),
            before_configuration_prepare: AsyncGate::default(),
            before_configuration_publish: AsyncGate::default(),
            before_catalog_publication: AsyncGate::default(),
            idle_before_claim: Arc::default(),
            idle_after_claim: Arc::default(),
            activation: Mutex::new(None),
            compositions: AtomicUsize::new(0),
            acquisitions: AtomicUsize::new(0),
            before_allocation: AsyncGate::default(),
            after_writer_transfer: AsyncGate::default(),
            joined: watch::channel(0).0,
            unloads_joined: watch::channel(0).0,
            fail_compose_once: std::sync::atomic::AtomicBool::new(false),
            panic_once: std::sync::atomic::AtomicBool::new(false),
            before_compose: AsyncGate::default(),
            after_delete_fence: AsyncGate::default(),
            before_shutdown: AsyncGate::default(),
            before_operation: AsyncGate::default(),
            draining_operations: watch::channel(false).0,
        }
    }
}

impl Probe {
    async fn joined(&self, count: usize) {
        self.joined
            .subscribe()
            .wait_for(|n| *n >= count)
            .await
            .unwrap();
    }
}

fn unload_task(
    f: &Fixture,
    id: ConversationId,
) -> tokio::task::JoinHandle<Result<(), RuntimeManagerError>> {
    let manager = f.manager.clone();
    tokio::spawn(async move { manager.unload(&id).await })
}

fn replace_task(
    f: &Fixture,
    index: usize,
) -> tokio::task::JoinHandle<Result<Arc<ManagedRuntime>, RuntimeManagerError>> {
    let manager = f.manager.clone();
    let session = f.sessions[index].id.clone();
    tokio::spawn(async move { manager.replace(&session, None).await })
}
