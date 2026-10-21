//! Manager lease transfer is independent of the native compaction reply.
use super::*;

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn native_compaction_transfer_releases_its_lease_before_reply_or_waiter_loss() {
    bounded(async {
        for lose_reply in [false, true] {
            let f = Fixture::new().await;
            let loaded = f.load(0).await.unwrap().unwrap();
            let id = loaded.conversation_id().clone();
            let incarnation = loaded.incarnation_id();
            let client = loaded.client();
            let (attachment, _pin) = client.attach().unwrap();
            let authority = attachment.operation_authority().unwrap();
            let (entered_tx, entered_rx) = tokio::sync::oneshot::channel();
            let (release_tx, release_rx) = tokio::sync::oneshot::channel();
            let (finished_tx, finished_rx) = tokio::sync::oneshot::channel();
            let mut reply = Some(
                client
                    .start_compaction_operation(move |_| async move {
                        // Real native admission owns the business task before the
                        // manager releases its lease. Only reply consumption is gated.
                        let native = authority.admit_compaction_transfer(None)?;
                        let tail: CompactionTail = Box::pin(async move {
                            let _ = entered_tx.send(());
                            release_rx.await.unwrap();
                            let result = native.await;
                            let _ = finished_tx.send(());
                            result
                        });
                        Ok(tail)
                    })
                    .unwrap(),
            );
            entered_rx.await.unwrap();
            assert!(matches!(
                reply.as_mut().unwrap().try_recv(),
                Err(tokio::sync::oneshot::error::TryRecvError::Empty)
            ));
            let diagnostic = f
                .manager
                .diagnostics()
                .sessions
                .into_iter()
                .find(|session| session.conversation_id == id)
                .unwrap();
            assert_eq!(diagnostic.operations, 0, "reply wait owns no manager lease");
            if lose_reply {
                drop(reply.take());
            }

            // An explicit unload must reach native shutdown and release the
            // actual writer allocation while the response is still parked.
            f.manager.unload(&id).await.unwrap();
            assert_eq!(f.manager.residency(&id), ResidencyState::Unloaded);
            let successor = f.load(0).await.unwrap().unwrap();
            assert_ne!(successor.incarnation_id(), incarnation);
            release_tx.send(()).unwrap();
            finished_rx.await.unwrap();
            if let Some(reply) = reply {
                assert!(
                    reply.await.unwrap().is_err(),
                    "empty history cannot compact"
                );
            }
            assert!(f.provider.request_bodies().is_empty());
            f.close().await;
        }
    })
    .await;
}
