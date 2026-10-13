//! Drive the actual Darwin supervisor beyond an expired absence observation.
//! The fixture withholds one proof; it cannot manufacture the later ESRCH.

use super::*;
use std::os::fd::OwnedFd;
use std::time::Duration;
use tokio::io::{AsyncReadExt, AsyncWriteExt};

async fn next_frame(
    stream: &mut tokio::net::UnixStream,
    reader: &mut FrameReader,
    trace_path: &std::path::Path,
) -> (u8, Vec<u8>) {
    tokio::time::timeout(Duration::from_secs(20), async {
        loop {
            if let Some(frame) = reader.pop() {
                return frame;
            }
            let mut bytes = [0; 256];
            let count = stream.read(&mut bytes).await.expect("control read");
            assert_ne!(count, 0, "unexpected control EOF");
            reader.feed(&bytes[..count]);
        }
    })
    .await
    .unwrap_or_else(|error| {
        panic!(
            "the invocation settles: {error}; native transitions:\n{}",
            std::fs::read_to_string(trace_path).unwrap_or_default()
        )
    })
}

#[tokio::test]
#[allow(clippy::too_many_lines)] // One native retained-anchor/absence/terminal proof.
async fn expired_absence_observation_can_recover_without_signaling_a_released_group() {
    // Keep Darwin's short Unix socket path limit independent of TMPDIR.
    let dir = tempfile::tempdir_in("/tmp").expect("fixture directory");
    let gate_path = dir.path().join("absence.sock");
    let trace_path = dir.path().join("trace.jsonl");
    let gate = tokio::net::UnixListener::bind(&gate_path).expect("absence gate");
    let (runtime, supervisor) = std::os::unix::net::UnixStream::pair().unwrap();
    runtime.set_nonblocking(true).unwrap();
    let mut stream = tokio::net::UnixStream::from_std(runtime).unwrap();
    let mut outer =
        tokio::process::Command::new(crate::runtime::process_runner::supervisor_binary())
            .env("RUSTX_SUPERVISOR_ROLE", ROLE_OUTER)
            .env(
                crate::runtime::supervised_unit::COMMAND_ENVIRONMENT_ENV,
                r#"[["PATH","/usr/bin:/bin"]]"#,
            )
            .env(COMMAND_ENV, "sleep 30")
            .env(FAIL_SIGNAL_ENV, "1")
            .env(diagnostics::TRACE_ENV, &trace_path)
            .env(diagnostics::GROUP_ABSENCE_GATE_ENV, &gate_path)
            .stdin(Stdio::from(OwnedFd::from(supervisor)))
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .kill_on_drop(true)
            .spawn()
            .expect("real outer supervisor");
    let mut reader = FrameReader::new();
    let (kind, payload) = next_frame(&mut stream, &mut reader, &trace_path).await;
    assert_eq!(kind, MSG_ANCHOR_READY);
    let pgid = i32::from_le_bytes(payload.try_into().unwrap());
    assert!(pgid > 0, "the invocation owns a positive process-group id");
    crate::runtime::process_runner::send_start(&mut stream)
        .await
        .unwrap();
    assert_eq!(
        next_frame(&mut stream, &mut reader, &trace_path).await.0,
        MSG_OWNERSHIP_ESTABLISHED
    );
    assert!(crate::runtime::process_runner::send_terminate(&mut stream).await);
    let mut signals = Vec::new();
    let mut failures = Vec::new();
    // The gate is reached only after fallback signaling and anchor release.
    let (mut peer, _) = tokio::time::timeout(Duration::from_secs(20), gate.accept())
        .await
        .expect("outer reaches absence observation")
        .unwrap();
    let mut notification = [0];
    tokio::time::timeout(Duration::from_secs(20), peer.read_exact(&mut notification))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(notification, *b"P");
    let trace = std::fs::read_to_string(&trace_path).unwrap();
    assert!(trace.contains("GroupChildrenReaped"), "{trace}");
    assert!(!trace.contains("TerminalPublished"), "{trace}");
    // Force an expired EPERM observation, not a clock delay. The next probe
    // must still execute and must prove absence against the actual kernel.
    peer.write_all(b"R").await.unwrap();
    loop {
        let (kind, payload) = next_frame(&mut stream, &mut reader, &trace_path).await;
        match kind {
            MSG_SIGNAL_ATTEMPT => signals.push(payload),
            MSG_PROCESS_CONTROL_FAILURE => failures.push(String::from_utf8(payload).unwrap()),
            MSG_ALL_CHILDREN_REAPED => break,
            other => panic!("unexpected supervisor frame {other:#x}"),
        }
    }
    assert_eq!(failures.len(), 2, "{failures:?}");
    assert!(failures[0].contains("injected signaling failure"));
    assert!(failures[1].contains("did not become provably absent"));
    assert_eq!(
        signals,
        vec![
            signal_attempt_payload(pgid, Signal::SIGTERM, false),
            signal_attempt_payload(pgid, Signal::SIGKILL, true),
        ]
    );
    assert_eq!(killpg(Pid::from_raw(pgid), None), Err(Errno::ESRCH));
    crate::runtime::process_runner::send_terminal_ack(&mut stream).await;
    assert!(
        tokio::time::timeout(Duration::from_secs(20), outer.wait())
            .await
            .unwrap()
            .unwrap()
            .success()
    );
    assert!(reader.pop().is_none(), "no second terminal frame");
    assert_eq!(
        tokio::time::timeout(Duration::from_secs(20), stream.read(&mut [0]))
            .await
            .unwrap()
            .unwrap(),
        0,
        "complete control EOF"
    );
    let trace = std::fs::read_to_string(trace_path).unwrap();
    for event in [
        "FallbackContainment",
        "GroupChildrenReaped",
        "GroupAbsenceUnproven",
        "GroupAbsenceProven",
        "TerminalPublished",
    ] {
        assert_eq!(trace.matches(event).count(), 1, "{event}: {trace}");
    }
    let reaped = trace.find("GroupChildrenReaped").unwrap();
    assert!(trace.find("FallbackContainment").unwrap() < reaped);
    assert!(reaped < trace.find("GroupAbsenceUnproven").unwrap());
    assert!(
        trace.find("GroupAbsenceUnproven").unwrap() < trace.find("GroupAbsenceProven").unwrap()
    );
    assert!(trace.find("GroupAbsenceProven").unwrap() < trace.find("TerminalPublished").unwrap());
}
