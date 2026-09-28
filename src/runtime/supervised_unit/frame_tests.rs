use std::io::Read;
use std::net::Shutdown;
use std::os::unix::net::UnixStream;

use super::{FrameReader, MSG_PROCESS_CONTROL_FAILURE, write_frame};

fn pair() -> (UnixStream, UnixStream) {
    let (sender, receiver) = UnixStream::pair().unwrap();
    nix::sys::socket::setsockopt(&sender, nix::sys::socket::sockopt::SndBuf, &4096).unwrap();
    sender.set_nonblocking(true).unwrap();
    (sender, receiver)
}

#[test]
fn nonblocking_frame_never_reports_partial_success() {
    let (mut sender, mut receiver) = pair();
    let payload = vec![0x5a; 1024 * 1024];
    // No receiver runs during this call: a frame larger than the constrained
    // send buffer cannot be committed by one nonblocking write.
    let result = write_frame(&mut sender, MSG_PROCESS_CONTROL_FAILURE, &payload);
    let _ = sender.shutdown(Shutdown::Write);
    let mut bytes = Vec::new();
    receiver.read_to_end(&mut bytes).unwrap();
    eprintln!(
        "write result: {result:?}; committed {} of {} bytes",
        bytes.len(),
        payload.len() + 5
    );
    let mut reader = FrameReader::new();
    reader.feed(&bytes);
    if result.is_ok() {
        let frame = reader
            .pop()
            .expect("success must mean the complete frame was committed");
        assert_eq!(frame, (MSG_PROCESS_CONTROL_FAILURE, payload));
        assert_eq!(reader.pop(), None);
    } else {
        assert!(
            nix::unistd::write(&sender, b"next frame").is_err(),
            "failed delivery must close the channel, including a partial prefix"
        );
    }
}

#[test]
fn consecutive_frames_complete_after_kernel_backpressure() {
    use super::{FrameWriter, MSG_NO_OWNERSHIP, wait_control_writable};
    use nix::errno::Errno;
    use nix::sys::socket::{MsgFlags, send};
    use std::os::fd::AsRawFd;
    let (mut sender, mut receiver) = pair();
    let mut filled = 0;
    loop {
        match send(sender.as_raw_fd(), &[0; 1024], MsgFlags::MSG_DONTWAIT) {
            Ok(count) => filled += count,
            Err(Errno::EAGAIN) => break,
            other => panic!("unexpected fill result: {other:?}"),
        }
    }
    assert!(filled > 0);
    let (blocked, observe_blocked) = std::sync::mpsc::channel();
    let payload = vec![0x5a; 1024 * 1024];
    let expected = payload.clone();
    let writer = std::thread::spawn(move || {
        let mut frame = FrameWriter::new(MSG_PROCESS_CONTROL_FAILURE, &payload).unwrap();
        let mut first_block = true;
        frame
            .complete(
                |bytes| {
                    let result = send(sender.as_raw_fd(), bytes, MsgFlags::MSG_DONTWAIT);
                    if result == Err(Errno::EAGAIN) && first_block {
                        first_block = false;
                        blocked.send(()).unwrap();
                    }
                    result
                },
                |remaining| wait_control_writable(&sender, remaining),
                std::time::Instant::now() + super::CONTROL_WRITE_BUDGET,
            )
            .unwrap();
        write_frame(&mut sender, MSG_NO_OWNERSHIP, &[]).unwrap();
        sender.shutdown(Shutdown::Write).unwrap();
    });
    // Release the real receiver only after the frame writer itself encounters
    // kernel backpressure. No sleep or elapsed-time assertion orders progress.
    observe_blocked
        .recv_timeout(std::time::Duration::from_secs(5))
        .unwrap();
    receiver
        .set_read_timeout(Some(std::time::Duration::from_secs(5)))
        .unwrap();
    receiver.read_exact(&mut vec![0; filled]).unwrap();
    let mut reader = FrameReader::new();
    let mut chunk = [0; 2048];
    loop {
        let count = receiver.read(&mut chunk).unwrap();
        if count == 0 {
            break;
        }
        reader.feed(&chunk[..count]);
    }
    writer.join().unwrap();
    assert_eq!(reader.pop(), Some((MSG_PROCESS_CONTROL_FAILURE, expected)));
    assert_eq!(reader.pop(), Some((MSG_NO_OWNERSHIP, Vec::new())));
    assert_eq!(reader.pop(), None);
    assert!(
        reader.buf.is_empty(),
        "no duplicated or corrupt trailing bytes"
    );
}

#[test]
fn interrupted_and_partial_writes_preserve_the_exact_offset() {
    use super::FrameWriter;
    use nix::errno::Errno;
    let mut frame = FrameWriter::new(MSG_PROCESS_CONTROL_FAILURE, b"failure").unwrap();
    let expected = frame.bytes.clone();
    let mut steps = [
        Err(Errno::EINTR),
        Ok(2),
        Err(Errno::EAGAIN),
        Ok(1),
        Err(Errno::EINTR),
        Ok(usize::MAX),
    ]
    .into_iter();
    let mut delivered = Vec::new();
    let mut waits = 0;
    frame
        .complete(
            |bytes| {
                let count = steps.next().expect("finite scripted writes")?;
                let count = count.min(bytes.len());
                delivered.extend_from_slice(&bytes[..count]);
                Ok(count)
            },
            |_| {
                waits += 1;
                Ok(())
            },
            std::time::Instant::now() + super::CONTROL_WRITE_BUDGET,
        )
        .unwrap();
    assert_eq!(delivered, expected);
    assert_eq!(frame.offset, frame.bytes.len());
    assert_eq!(waits, 1);
}

#[test]
fn closed_peer_is_a_channel_failure() {
    let (mut sender, receiver) = pair();
    drop(receiver);
    assert!(write_frame(&mut sender, MSG_PROCESS_CONTROL_FAILURE, b"failure").is_err());
    assert!(nix::unistd::write(&sender, b"next frame").is_err());
}

#[test]
fn delivery_action_requires_strictly_positive_remaining_budget() {
    let deadline = std::time::Instant::now();
    let tick = std::time::Duration::from_nanos(1);
    assert_eq!(
        super::control_write_remaining(deadline.checked_sub(tick).unwrap(), deadline),
        Ok(tick)
    );
    for now in [deadline, deadline + tick] {
        assert_eq!(
            super::control_write_remaining(now, deadline),
            Err("control frame delivery deadline expired".to_owned())
        );
    }
}

#[test]
fn expired_budget_never_attempts_another_write() {
    let mut frame = super::FrameWriter::new(MSG_PROCESS_CONTROL_FAILURE, b"failure").unwrap();
    let mut sends = 0;
    let mut waits = 0;
    // A monotonic clock sample inside complete is equal to or later than this
    // deadline. Both cases must expire; no clock tick needs to elapse.
    let deadline = std::time::Instant::now();
    let result = frame.complete(
        |bytes| {
            sends += 1;
            Ok(bytes.len())
        },
        |_| {
            waits += 1;
            Ok(())
        },
        deadline,
    );
    assert_eq!(
        result,
        Err("control frame delivery deadline expired".to_owned())
    );
    assert_eq!(sends, 0);
    assert_eq!(waits, 0);
    assert_eq!(frame.offset, 0);
}
