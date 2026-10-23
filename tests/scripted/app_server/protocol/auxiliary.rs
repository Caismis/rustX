//! Native handoff and exact negative evidence through actual App Server admission.
use super::*;
use crate::runtime::auxiliary::submission::{BtwSubmission, CorrelationId};
use crate::runtime::conversation_runtime::auxiliary::AuxiliarySubmissionLookup;
fn submission(correlation: &str) -> BtwSubmission {
    BtwSubmission {
        correlation: CorrelationId::new(correlation.into()).unwrap(),
        question: "A temporary question".into(),
    }
}
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn auxiliary_original_scope_closes_while_manager_handoff_is_parked() {
    bounded(async {
        let f = Fixture::with_session_count(None, 1).await;
        let connection = std::sync::Arc::new(AppServerConnection::with_delivery_access(
            f.host.clone(),
            tokio_util::sync::CancellationToken::new(),
        ));
        initialize(&connection).await;
        let target = attach(&connection, &f, 0).await;
        let managed = f.load(0).await.unwrap().unwrap();
        let runtime = managed.inspect_runtime().unwrap();
        let probe = f.manager.probe(&target.conversation_id);
        probe.before_operation.arm();
        let input = submission("park-before-native");
        let correlation = input.correlation.clone();
        let forwarded = connection.clone();
        let task = tokio::spawn(async move {
            forwarded
                .handle_request(Request {
                    jsonrpc: JsonRpcVersion::V2,
                    id: RequestId::Integer(476),
                    call: Method::AuxiliaryBtw {
                        target,
                        submission: input,
                    },
                })
                .await
        });
        probe.before_operation.entered().await;
        let AuxiliarySubmissionLookup::Pending { scope, .. } =
            runtime.auxiliary_lookup(&correlation, None)
        else {
            panic!("original scope exists before native handoff")
        };
        connection.close();
        assert!(matches!(
            runtime.auxiliary_lookup(&correlation, Some(&scope)),
            AuxiliarySubmissionLookup::ConfirmedNotAdmitted { .. }
        ));
        probe.before_operation.release();
        assert!(matches!(task.await.unwrap(), Response::Failure(_)));
        assert!(f.provider.request_bodies().is_empty());
        assert!(matches!(
            runtime.auxiliary_lookup(&correlation, Some(&scope)),
            AuxiliarySubmissionLookup::ConfirmedNotAdmitted { .. }
        ));
        drop(managed);
        f.close().await;
    })
    .await;
}
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn auxiliary_receipt_returns_before_provider_and_result_is_repeatable_after_detach() {
    bounded(async {
        let f = Fixture::with_session_count(None, 1).await;
        let connection = AppServerConnection::with_delivery_access(
            f.host.clone(),
            tokio_util::sync::CancellationToken::new(),
        );
        initialize(&connection).await;
        let target = attach(&connection, &f, 0).await;
        let input = submission("receipt-before-result");
        let MethodResult::AuxiliarySubmitted { receipt } = call(
            &connection,
            476,
            Method::AuxiliaryBtw {
                target: target.clone(),
                submission: input.clone(),
            },
        )
        .await
        else {
            panic!("finite receipt")
        };
        f.gates[0].wait_entered().await;
        assert_eq!(
            call(
                &connection,
                477,
                Method::AuxiliaryBtw {
                    target: target.clone(),
                    submission: input
                }
            )
            .await,
            MethodResult::AuxiliarySubmitted {
                receipt: receipt.clone()
            }
        );
        call(&connection, 478, Method::SessionDetach { target }).await;
        let successor = attach(&connection, &f, 0).await;
        assert!(matches!(
            call(
                &connection,
                479,
                Method::AuxiliarySubmissionLookup {
                    target: successor.clone(),
                    correlation: receipt.correlation.clone(),
                    scope: Some(receipt.scope.clone())
                }
            )
            .await,
            MethodResult::AuxiliarySubmissionLookup {
                lookup: AuxiliarySubmissionLookup::Known { .. }
            }
        ));
        f.gates[0].release();
        let read = Method::AuxiliaryResult {
            target: successor.clone(),
            call_id: receipt.call_id.clone(),
        };
        let result = loop {
            let result = call(&connection, 480, read.clone()).await;
            if matches!(result, MethodResult::AuxiliaryResult { result: Some(_) }) {
                break result;
            }
            tokio::task::yield_now().await;
        };
        assert_eq!(result, call(&connection, 481, read).await);
        assert_eq!(f.provider.request_bodies().len(), 1);
        connection.revoke_delivery_access();
        assert!(matches!(
            connection
                .handle_request(Request {
                    jsonrpc: JsonRpcVersion::V2,
                    id: RequestId::Integer(482),
                    call: Method::AuxiliaryStatus {
                        target: successor,
                        call_id: receipt.call_id
                    }
                })
                .await,
            Response::Failure(_)
        ));
        connection.close();
        f.close().await;
    })
    .await;
}

#[tokio::test]
async fn auxiliary_cancel_runs_while_a_real_store_read_holds_the_connection_on_one_async_worker() {
    bounded(async {
        let f = Fixture::with_session_count(None, 1).await;
        let connection = std::sync::Arc::new(AppServerConnection::with_delivery_access(
            f.host.clone(),
            tokio_util::sync::CancellationToken::new(),
        ));
        initialize(&connection).await;
        let target = attach(&connection, &f, 0).await;
        let MethodResult::AuxiliarySubmitted { receipt } = call(
            &connection,
            600,
            Method::AuxiliaryBtw {
                target: target.clone(),
                submission: submission("cancel-with-blocked-source"),
            },
        )
        .await
        else {
            panic!("receipt")
        };
        f.gates[0].wait_entered().await;
        let (entered, observing) = tokio::sync::oneshot::channel();
        let (release, parked) = std::sync::mpsc::channel();
        crate::durable::sqlite::install_auxiliary_lookup_hook(
            "blocked-auxiliary-read".into(),
            Box::new(move || {
                entered.send(()).unwrap();
                parked.recv().unwrap();
            }),
        );
        let reader = connection.clone();
        let read_target = target.clone();
        let pending = tokio::spawn(async move {
            call(
                &reader,
                601,
                Method::AuxiliarySubmissionLookup {
                    target: read_target,
                    correlation: CorrelationId::new("blocked-auxiliary-read".into()).unwrap(),
                    scope: None,
                },
            )
            .await
        });
        observing.await.unwrap();
        assert!(matches!(
            call(
                &connection,
                602,
                Method::AuxiliaryCancel {
                    target,
                    call_id: receipt.call_id
                }
            )
            .await,
            MethodResult::AuxiliaryCancelled { accepted: true }
        ));
        assert!(
            !pending.is_finished(),
            "cancellation did not wait for or terminate the source read"
        );
        release.send(()).unwrap();
        pending.await.unwrap();
        f.gates[0].release();
        connection.close();
        f.close().await;
    })
    .await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn admitted_auxiliary_receipt_revocation_before_writer_acceptance_is_unknown() {
    bounded(async {
        for (revocation, uncertain) in [
            ("detach", None),
            ("grant", None),
            ("delivery", None),
            ("delivery", Some(ErrorData::OutcomeUnknown)),
            ("delivery", Some(ErrorData::CommittedDurabilityUncertain)),
        ] {
            for writer in [false, true] {
                let f = Fixture::with_session_count(None, 1).await;
                let connection = AppServerConnection::with_delivery_access(
                    f.host.clone(),
                    tokio_util::sync::CancellationToken::new(),
                );
                initialize(&connection).await;
                let target = attach(&connection, &f, 0).await;
                let input = submission("confirmed-before-publication");
                let mut reply = connection
                    .reply(Request {
                        jsonrpc: JsonRpcVersion::V2,
                        id: RequestId::Integer(700),
                        call: Method::AuxiliaryBtw {
                            target: target.clone(),
                            submission: input.clone(),
                        },
                    })
                    .await;
                let Response::Success(success) = reply.produced() else {
                    panic!("confirmed receipt")
                };
                let MethodResult::AuxiliarySubmitted { receipt } = &success.result else {
                    panic!("receipt")
                };
                let receipt = receipt.clone();
                f.gates[0].wait_entered().await;
                if let Some(data) = &uncertain {
                    reply.force_uncertain_failure_for_test(data.clone());
                }
                // Both native admission and inference have occurred. The real
                // publication owner has offered no accepted byte yet.
                let (mut outbound, direct) = if writer {
                    let mut outbound =
                        crate::app_server::transport::Outbound::reply(reply).unwrap();
                    assert!(
                        outbound
                            .poll_hand_off::<()>(|_| std::task::Poll::Pending)
                            .is_pending()
                    );
                    (Some(outbound), None)
                } else {
                    (None, Some(reply))
                };
                match revocation {
                    "detach" => {
                        call(&connection, 701, Method::SessionDetach { target }).await;
                    }
                    "grant" => connection.revoke_delivery_access(),
                    "delivery" => {
                        call(
                            &connection,
                            701,
                            Method::DeliveryCancel {
                                request_id: RequestId::Integer(700),
                            },
                        )
                        .await;
                    }
                    _ => unreachable!(),
                }
                let response = if let Some(outbound) = &mut outbound {
                    let std::task::Poll::Ready(Ok((record, Some(())))) =
                        outbound.poll_hand_off(|_| std::task::Poll::Ready(Ok(())))
                    else {
                        panic!("writer acceptance")
                    };
                    serde_json::from_str(&record).unwrap()
                } else {
                    direct.unwrap().commit()
                };
                let Response::Failure(failure) = response else {
                    panic!("revoked receipt")
                };
                assert_eq!(failure.error.data, Some(ErrorData::OutcomeUnknown));
                connection.close();
                let successor = AppServerConnection::with_delivery_access(
                    f.host.clone(),
                    tokio_util::sync::CancellationToken::new(),
                );
                initialize(&successor).await;
                let target = attach(&successor, &f, 0).await;
                assert_eq!(
                    call(
                        &successor,
                        702,
                        Method::AuxiliarySubmissionLookup {
                            target,
                            correlation: input.correlation,
                            scope: None,
                        }
                    )
                    .await,
                    MethodResult::AuxiliarySubmissionLookup {
                        lookup: AuxiliarySubmissionLookup::Known { receipt },
                    }
                );
                assert_eq!(f.provider.request_bodies().len(), 1);
                f.gates[0].release();
                successor.close();
                f.close().await;
            }
        }
    })
    .await;
}
