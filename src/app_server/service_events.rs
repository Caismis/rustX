//! Bounded connection-owned environment observations and physical output fence.
use super::protocol::{JsonRpcVersion, Notification, NotificationMethod};
use tokio::sync::{mpsc, oneshot};
use tokio_util::sync::CancellationToken;

pub(super) struct Event {
    pub notification: Notification,
    pub emitted: oneshot::Sender<()>,
    pub fence: Option<CancellationToken>,
}
pub(super) struct Events {
    sender: mpsc::Sender<Event>,
    receiver: tokio::sync::Mutex<mpsc::Receiver<Event>>,
    pub closed: CancellationToken,
    pub overloaded: CancellationToken,
}
impl Default for Events {
    fn default() -> Self {
        Self::new(CancellationToken::new())
    }
}
impl Events {
    pub fn new(closed: CancellationToken) -> Self {
        let (sender, receiver) = mpsc::channel(32);
        Self {
            sender,
            receiver: tokio::sync::Mutex::new(receiver),
            closed,
            overloaded: CancellationToken::new(),
        }
    }
    pub fn emit(&self, notification: NotificationMethod) -> Result<oneshot::Receiver<()>, ()> {
        self.emit_fenced(notification, None)
    }
    pub fn emit_fenced(
        &self,
        notification: NotificationMethod,
        fence: Option<CancellationToken>,
    ) -> Result<oneshot::Receiver<()>, ()> {
        let (emitted, receipt) = oneshot::channel();
        if self.closed.is_cancelled() {
            return Err(());
        }
        self.sender
            .try_send(Event {
                notification: Notification {
                    jsonrpc: JsonRpcVersion::V2,
                    notification,
                },
                emitted,
                fence,
            })
            .map_err(|_| {
                self.overloaded.cancel();
            })?;
        Ok(receipt)
    }
    #[cfg(all(test, target_os = "linux"))]
    pub fn idle(&self) -> bool {
        self.sender.capacity() == 32 && !self.overloaded.is_cancelled()
    }
    pub async fn next(&self) -> Event {
        self.receiver
            .lock()
            .await
            .recv()
            .await
            .expect("connection event sender retained")
    }
    pub async fn emitted(&self, receipt: oneshot::Receiver<()>) -> Result<(), ()> {
        tokio::select! {
            result = receipt => result.map_err(|_| ()),
            () = self.closed.cancelled() => Err(()),
            () = self.overloaded.cancelled() => Err(()),
        }
    }
}
