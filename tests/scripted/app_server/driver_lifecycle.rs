//! The shared test driver settles every request the transport can no longer
//! complete, whether it waits for outbound admission or for its response.
use super::bounded;
use super::driver::{self, Driver, TransportEnded};
use crate::app_server::protocol::{
    Failure, JsonRpcVersion, Method, Request, RequestId, Response, RpcError,
};
use futures_util::{FutureExt, Sink, future::BoxFuture};
use tokio::sync::mpsc;

fn request(id: i64) -> Request {
    Request {
        jsonrpc: JsonRpcVersion::V2,
        id: RequestId::Integer(id),
        call: Method::ServerDiagnostics {},
    }
}
fn response(id: i64) -> Response {
    Response::Failure(Failure {
        jsonrpc: JsonRpcVersion::V2,
        id: Some(RequestId::Integer(id)),
        error: RpcError {
            code: -32_000,
            message: "answered".into(),
            data: None,
        },
    })
}
fn written_id(record: &str) -> i64 {
    serde_json::from_str::<serde_json::Value>(record).unwrap()["id"]
        .as_i64()
        .unwrap()
}

/// A peer whose input the test feeds record by record and whose acceptance
/// of each outbound record the test decides.
struct Peer {
    input: mpsc::UnboundedSender<Result<String, String>>,
    offered: mpsc::UnboundedReceiver<String>,
    decide: mpsc::UnboundedSender<Result<(), String>>,
}
impl Peer {
    fn connect() -> (Driver, Self) {
        let (input, incoming) = mpsc::unbounded_channel();
        let (offer, offered) = mpsc::unbounded_channel();
        let (decide, decisions) = mpsc::unbounded_channel();
        let incoming = futures_util::stream::unfold(incoming, |mut incoming| async move {
            incoming.recv().await.map(|record| (record, incoming))
        });
        let driver = Driver::new(incoming, gated(offer, decisions));
        (
            driver,
            Self {
                input,
                offered,
                decide,
            },
        )
    }
    /// The next record the writer offers, still unaccepted.
    async fn offered(&mut self) -> i64 {
        written_id(&self.offered.recv().await.unwrap())
    }
    fn accept(&self) {
        self.decide.send(Ok(())).unwrap();
    }
    fn answer(&self, id: i64) {
        let record = serde_json::to_string(&response(id)).unwrap();
        self.input.send(Ok(record)).unwrap();
    }
}

/// A sink that offers each record to the test and completes the send only
/// with the test's decision; without one, the record is never accepted.
fn gated(
    offer: mpsc::UnboundedSender<String>,
    decisions: mpsc::UnboundedReceiver<Result<(), String>>,
) -> impl Sink<String, Error = String> + Send + 'static {
    futures_util::sink::unfold(
        (offer, decisions),
        |(offer, mut decisions), record: String| async move {
            offer.send(record).unwrap();
            match decisions.recv().await {
                Some(decision) => decision.map(|()| (offer, decisions)),
                None => std::future::pending().await,
            }
        },
    )
}

type Waiting<'a> = BoxFuture<'a, Result<Response, TransportEnded>>;
/// Start `ids` in order, each polled once so it is registered and either
/// admitted or blocked on admission.
fn start(client: &Driver, ids: impl IntoIterator<Item = i64>) -> Vec<Waiting<'_>> {
    ids.into_iter()
        .map(|id| {
            let mut waiting = client.try_request(request(id)).boxed();
            assert!(waiting.as_mut().now_or_never().is_none(), "{id} waits");
            waiting
        })
        .collect()
}
#[tokio::test]
async fn transport_end_settles_requests_blocked_on_a_saturated_outbound_queue() {
    bounded(async {
        for cause in [Ok("end of input"), Err("connection reset")] {
            let (client, mut peer) = Peer::connect();
            let mut held = start(&client, [0]);
            assert_eq!(peer.offered().await, 0, "the writer holds the first record");
            let mut waiting = start(&client, 1..=65);
            match cause {
                Ok(_) => drop(peer.input),
                Err(reset) => peer.input.send(Err(reset.into())).unwrap(),
            }
            let expected = TransportEnded(cause.unwrap_or_else(|reset| reset).into());
            // The 65th waits for admission; the others wait queued or offered.
            waiting.append(&mut held);
            for outcome in futures_util::future::join_all(waiting).await {
                assert_eq!(outcome, Err(expected.clone()));
            }
            assert_eq!(client.try_request(request(66)).await, Err(expected));
        }
    })
    .await;
}

#[tokio::test]
async fn write_failure_settles_queued_and_blocked_requests_while_input_stays_open() {
    bounded(async {
        let (client, mut peer) = Peer::connect();
        let mut waiting = start(&client, [0]);
        assert_eq!(peer.offered().await, 0);
        waiting.extend(start(&client, 1..=65));
        peer.decide.send(Err("broken pipe".into())).unwrap();
        let expected = TransportEnded("write failed: broken pipe".into());
        for outcome in futures_util::future::join_all(waiting).await {
            assert_eq!(outcome, Err(expected.clone()));
        }
        assert_eq!(client.try_request(request(66)).await, Err(expected));
        // Input remained open; nothing it delivers revives a failed request.
        assert!(!peer.input.is_closed());
    })
    .await;
}

#[tokio::test]
async fn output_close_fails_only_unsent_requests_and_keeps_sent_ones_answerable() {
    bounded(async {
        let (client, mut peer) = Peer::connect();
        let mut sent = start(&client, [1]);
        assert_eq!(peer.offered().await, 1);
        peer.accept();
        let mut offered = start(&client, [2]);
        assert_eq!(peer.offered().await, 2, "2 is offered but not yet accepted");
        // Output closes after every record admitted before the close.
        client.close_output().await;
        let unsent = start(&client, [3]).remove(0);
        peer.accept();
        let closed = TransportEnded("end of output".into());
        assert_eq!(unsent.await, Err(closed.clone()), "3 was never accepted");
        assert_eq!(client.try_request(request(4)).await, Err(closed.clone()));
        // Records accepted before the close still receive their responses.
        peer.answer(2);
        peer.answer(1);
        assert_eq!(offered.remove(0).await, Ok(response(2)));
        assert_eq!(sent.remove(0).await, Ok(response(1)));
        // The later end of input does not replace the first frontier's cause.
        drop(peer.input);
        assert_eq!(client.try_request(request(5)).await, Err(closed));
    })
    .await;
}

#[tokio::test]
async fn end_of_input_after_output_close_fails_sent_requests_with_the_input_cause() {
    bounded(async {
        let (client, mut peer) = Peer::connect();
        let sent = start(&client, [1]).remove(0);
        assert_eq!(peer.offered().await, 1);
        peer.accept();
        client.close_output().await;
        assert_eq!(
            client.try_request(request(2)).await,
            Err(TransportEnded("end of output".into()))
        );
        peer.input.send(Err("connection reset".into())).unwrap();
        assert_eq!(sent.await, Err(TransportEnded("connection reset".into())));
    })
    .await;
}

#[tokio::test]
async fn websocket_input_distinguishes_a_reset_from_a_completed_close() {
    bounded(async {
        use futures_util::StreamExt;
        use tokio_tungstenite::tungstenite::{Error, Message, error::ProtocolError};
        let ended = |last: Error| {
            driver::frames(futures_util::stream::iter([
                Ok(Message::Text("record".into())),
                Ok(Message::Ping(Vec::new().into())),
                Err(last),
            ]))
            .collect::<Vec<_>>()
        };
        assert_eq!(
            ended(Error::ConnectionClosed).await,
            [Ok("record".to_owned())],
            "a completed close handshake is an orderly end"
        );
        let reset = ended(Error::Protocol(ProtocolError::ResetWithoutClosingHandshake)).await;
        assert_eq!(reset.len(), 2);
        assert_eq!(
            reset[1],
            Err(Error::Protocol(ProtocolError::ResetWithoutClosingHandshake).to_string()),
            "a peer that vanished without the handshake is the recorded cause"
        );
    })
    .await;
}
