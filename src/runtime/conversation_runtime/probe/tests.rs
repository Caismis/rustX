use crate::runtime::conversation_runtime::test_support::*;

/// A wake from the admission task enters Tokio's private LIFO slot.
/// A synchronous probe must hand that core off before parking, otherwise
/// even a ready projection peer cannot run until the test releases it.
#[test]
fn attempt_execution_probe_hands_off_woken_peer_before_parking() {
    let guard = std::time::Duration::from_secs(30);
    for handoff in [false, true] {
        let runtime = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(1)
            .build()
            .unwrap();
        let gate = Arc::new(crate::runtime::conversation_runtime::Gate::default());
        let release = gate.arm_scoped();
        let wake = Arc::new(tokio::sync::Notify::new());
        let (registered, registration) = std::sync::mpsc::channel();
        let (progress, progressed) = std::sync::mpsc::channel();
        let peer_wake = wake.clone();
        let peer = runtime.spawn(async move {
            let notified = peer_wake.notified();
            tokio::pin!(notified);
            notified.as_mut().enable();
            registered.send(()).unwrap();
            notified.await;
            progress.send(()).unwrap();
        });
        registration.recv_timeout(guard).unwrap();
        let parked_gate = gate.clone();
        let admission = runtime.spawn(async move {
            wake.notify_one();
            if handoff {
                parked_gate.enter_attempt_execution();
            } else {
                parked_gate.enter();
            }
        });
        // This controller is outside the one-worker executor. The peer
        // was registered before this task woke it and immediately parked.
        gate.wait_entered();
        let progress_while_held = if handoff {
            progressed.recv_timeout(guard).map(|()| true)
        } else {
            assert!(matches!(
                progressed.try_recv(),
                Err(std::sync::mpsc::TryRecvError::Empty)
            ));
            Ok(false)
        };
        // Always unblock/join the physical worker before asserting the
        // liveness result, including a failed handoff implementation.
        drop(release);
        runtime.block_on(async {
            admission.await.unwrap();
            peer.await.unwrap();
        });
        assert_eq!(progress_while_held.unwrap(), handoff);
    }
}
