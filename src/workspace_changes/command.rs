//! Git uses the same physical process owner as Bash, Python and Skill setup.
//! Dropping a snapshot waiter cancels its process tree without abandoning reaping.
use crate::runtime::{
    cancellation::CancellationSignal,
    process_runner::{ProcessOutcomeIntent, SupervisedCommandRunner, SupervisedCommandSpec},
};
use tokio::io::{AsyncRead, AsyncReadExt};

struct CancelOnDrop(CancellationSignal);
impl Drop for CancelOnDrop {
    fn drop(&mut self) {
        self.0.cancel();
    }
}

async fn limited(reader: impl AsyncRead + Unpin, limit: usize) -> Result<Vec<u8>, String> {
    let mut bytes = Vec::new();
    reader
        .take(limit as u64 + 1)
        .read_to_end(&mut bytes)
        .await
        .map_err(|error| error.to_string())?;
    if bytes.len() > limit {
        return Err("Git output exceeded its limit".into());
    }
    Ok(bytes)
}

pub(super) async fn run(
    spec: SupervisedCommandSpec,
    limit: usize,
    keepalive: impl Send + 'static,
) -> Result<(i32, Vec<u8>, Vec<u8>), String> {
    let _lifetime = CancelOnDrop(spec.cancellation.clone());
    let (sender, receiver) = tokio::sync::oneshot::channel();
    // The detached driver, not the caller's abortable baseline task, owns
    // physical settlement. Even an output error must keep driving settle().
    std::mem::drop(tokio::spawn(async move {
        let result = drive(spec, limit).await;
        // Input files and the private Git directory outlive physical reaping,
        // including when the runtime abandons its observation waiter.
        drop(keepalive);
        let _ = sender.send(result);
    }));
    receiver.await.map_err(|error| error.to_string())?
}

async fn drive(
    spec: SupervisedCommandSpec,
    limit: usize,
) -> Result<(i32, Vec<u8>, Vec<u8>), String> {
    let (mut runner, stdout, stderr) =
        SupervisedCommandRunner::spawn(&spec, None).map_err(|error| error.to_string())?;
    let capture = async {
        let result = match (stdout, stderr) {
            (Some(stdout), Some(stderr)) => {
                tokio::try_join!(limited(stdout, limit), limited(stderr, 16 * 1024))
            }
            _ => Err("missing Git output pipes".into()),
        };
        if result.is_err() {
            spec.cancellation.cancel();
        }
        result
    };
    let (captured, terminal) = tokio::join!(capture, runner.settle());
    match terminal.intent {
        ProcessOutcomeIntent::Completed => {
            let (stdout, stderr) = captured?;
            let code = terminal
                .exit_status
                .and_then(|status| status.code())
                .ok_or("Git was terminated")?;
            Ok((code, stdout, stderr))
        }
        ProcessOutcomeIntent::Cancelled => Err(captured
            .err()
            .unwrap_or_else(|| "Git snapshot was cancelled".into())),
        ProcessOutcomeIntent::TimedOut => Err("Git snapshot timed out".into()),
        ProcessOutcomeIntent::ProcessControlFailed(error) => Err(error),
    }
}
