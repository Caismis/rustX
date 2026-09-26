//! Exact Git streams; interruptions retain accumulated bytes. The shared
//! supervised runner exclusively owns process wait, containment, and reap.
use std::io;
use tokio::io::AsyncReadExt;

pub(super) async fn read_pipe(
    pipe: Option<impl tokio::io::AsyncRead + Unpin>,
    #[cfg(test)] interrupt: &std::sync::atomic::AtomicBool,
) -> io::Result<Vec<u8>> {
    let mut output = Vec::new();
    if let Some(mut pipe) = pipe {
        let mut buffer = vec![0; 8192];
        loop {
            // Inject after a successful read to prove already-collected bytes
            // survive the interruption, using this collector's own fault.
            #[cfg(test)]
            let result = if !output.is_empty()
                && interrupt.swap(false, std::sync::atomic::Ordering::SeqCst)
            {
                Err(io::ErrorKind::Interrupted.into())
            } else {
                pipe.read(&mut buffer).await
            };
            #[cfg(not(test))]
            let result = pipe.read(&mut buffer).await;
            match result {
                Ok(0) => break,
                Ok(count) => output.extend_from_slice(&buffer[..count]),
                Err(error) if error.kind() == io::ErrorKind::Interrupted => {}
                Err(error) => return Err(error),
            }
        }
    }
    Ok(output)
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use crate::runtime::process_runner::{
        ProcessOutcomeIntent, RunnerTestControl, SupervisedCommandRunner, SupervisedCommandSpec,
    };
    use std::sync::atomic::{AtomicBool, Ordering};

    #[tokio::test]
    async fn eintr_collection_never_replays_started_child() {
        let directory = tempfile::tempdir().unwrap();
        let effect = directory.path().join("effect");
        let spec = SupervisedCommandSpec {
            command: "printf x >> effect; printf stdout; printf stderr >&2".into(),
            cwd: directory.path().into(),
            environment: std::env::vars().collect(),
            timeout: None,
            cancellation: crate::runtime::cancellation::CancellationSignal::new(),
        };
        let control = RunnerTestControl::new();
        control.interrupt_direct_wait.store(true, Ordering::SeqCst);
        let (mut runner, stdout, stderr) =
            SupervisedCommandRunner::spawn(&spec, Some(control.clone())).unwrap();
        let stdout_interrupt = AtomicBool::new(true);
        let stderr_interrupt = AtomicBool::new(true);
        let (terminal, stdout, stderr) = tokio::join!(
            runner.settle(),
            read_pipe(stdout, &stdout_interrupt),
            read_pipe(stderr, &stderr_interrupt)
        );
        assert!(!control.interrupt_direct_wait.load(Ordering::SeqCst));
        assert!(!stdout_interrupt.load(Ordering::SeqCst));
        assert!(!stderr_interrupt.load(Ordering::SeqCst));
        assert_eq!(terminal.intent, ProcessOutcomeIntent::Completed);
        assert!(terminal.exit_status.unwrap().success());
        assert_eq!(stdout.unwrap(), b"stdout");
        assert_eq!(stderr.unwrap(), b"stderr");
        assert_eq!(
            std::fs::read(effect).unwrap(),
            b"x",
            "the native command ran exactly once despite all three interruptions"
        );
    }
}
