//! Collect one already-spawned Git child. An interrupted wait/read never
//! returns to command construction or discards bytes already collected.
use std::{io, process::Output};
use tokio::{io::AsyncReadExt, process::Child};

pub(super) async fn collect(mut child: Child, #[cfg(test)] faults: &Faults) -> io::Result<Output> {
    let stdout = child.stdout.take();
    let stderr = child.stderr.take();
    drop(child.stdin.take());
    let wait = async {
        loop {
            #[cfg(test)]
            let result = if faults.wait.swap(false, std::sync::atomic::Ordering::SeqCst) {
                Err(io::ErrorKind::Interrupted.into())
            } else {
                child.wait().await
            };
            #[cfg(not(test))]
            let result = child.wait().await;
            match result {
                Err(error) if error.kind() == io::ErrorKind::Interrupted => {}
                result => break result,
            }
        }
    };
    let (status, stdout, stderr) = tokio::try_join!(
        wait,
        read_pipe(
            stdout,
            #[cfg(test)]
            &faults.stdout
        ),
        read_pipe(
            stderr,
            #[cfg(test)]
            &faults.stderr
        ),
    )?;
    Ok(Output {
        status,
        stdout,
        stderr,
    })
}

async fn read_pipe(
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

#[cfg(test)]
#[derive(Default)]
pub(super) struct Faults {
    wait: std::sync::atomic::AtomicBool,
    stdout: std::sync::atomic::AtomicBool,
    stderr: std::sync::atomic::AtomicBool,
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use std::{
        process::Stdio,
        sync::atomic::{AtomicBool, Ordering},
    };

    #[tokio::test]
    async fn eintr_collection_never_replays_started_child() {
        let directory = tempfile::tempdir().unwrap();
        let effect = directory.path().join("effect");
        let mut command = tokio::process::Command::new("sh");
        command
            .args([
                "-c",
                "printf x >> \"$1\"; printf stdout; printf stderr >&2",
                "fixture",
            ])
            .arg(&effect)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        let mut spawns = 0;
        let child = {
            spawns += 1;
            command.spawn().unwrap()
        };
        let faults = Faults {
            wait: AtomicBool::new(true),
            stdout: AtomicBool::new(true),
            stderr: AtomicBool::new(true),
        };
        // Borrow the faults through the whole collector so consumption is checked.
        let mut terminal_results = 0;
        let result = collect(child, &faults).await.unwrap();
        terminal_results += 1;
        assert!(!faults.wait.load(Ordering::SeqCst));
        assert!(!faults.stdout.load(Ordering::SeqCst));
        assert!(!faults.stderr.load(Ordering::SeqCst));
        assert_eq!(terminal_results, 1);
        assert!(result.status.success());
        assert_eq!(result.stdout, b"stdout");
        assert_eq!(result.stderr, b"stderr");
        assert_eq!(spawns, 1);
        assert_eq!(std::fs::read(effect).unwrap(), b"x");
    }
}
