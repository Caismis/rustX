use super::*;

fn parameters() -> ExecParams {
    ExecParams {
        command: vec!["unused".into()],
        cwd: "/".into(),
        env: BTreeMap::new(),
        process_id: Some("fragmented".into()),
        tty: true,
        stream_stdin: false,
        stream_stdout_stderr: false,
        output_bytes_cap: None,
        timeout_ms: None,
        size: None,
    }
}

#[tokio::test]
async fn tiny_reads_batch_with_no_consumer_progress_and_preserve_every_byte() {
    let params = parameters();
    let events = super::super::service_events::Events::default();
    let mut output = Output::default();
    let mut sequence = 0;
    let cap = 262_144;
    assert!(
        output
            .push(
                b"READY",
                cap,
                OutputStream::Stdout,
                &params,
                &events,
                &mut sequence
            )
            .unwrap()
            .is_none()
    );
    let mut last_receipt = output
        .flush(false, OutputStream::Stdout, &params, &events, &mut sequence)
        .unwrap();
    // Deliberately withhold every receipt while the native producer makes
    // 131,072 one-byte reads. Scheduling cannot rescue a per-read queue here.
    for _ in 0..131_072 {
        if let Some(receipt) = output
            .push(
                &[0],
                cap,
                OutputStream::Stdout,
                &params,
                &events,
                &mut sequence,
            )
            .unwrap()
        {
            last_receipt = Some(receipt);
        }
        assert!(output.pending.len() < OUTPUT_CHUNK_BYTES);
    }
    assert_eq!(sequence, 17);
    assert!(!events.overloaded.is_cancelled());
    output
        .push(
            b"DONE",
            cap,
            OutputStream::Stdout,
            &params,
            &events,
            &mut sequence,
        )
        .unwrap();
    assert_eq!(sequence, 17, "the short tail awaits progress or EOF");
    last_receipt = output
        .flush(false, OutputStream::Stdout, &params, &events, &mut sequence)
        .unwrap()
        .or(last_receipt);
    assert_eq!(sequence, 18);
    let mut received = Vec::new();
    for expected in 1..=sequence {
        let event = events.next().await;
        let NotificationMethod::CommandOutputDelta(delta) = event.notification.notification else {
            panic!("command output")
        };
        assert_eq!(delta.sequence, expected);
        assert!(!delta.cap_reached);
        let bytes = base64::engine::general_purpose::STANDARD
            .decode(delta.delta_base64)
            .unwrap();
        assert!(bytes.len() <= OUTPUT_CHUNK_BYTES);
        received.extend(bytes);
        let _ = event.emitted.send(());
    }
    assert_eq!(&received[..5], b"READY");
    assert!(received[5..5 + 131_072].iter().all(|byte| *byte == 0));
    assert_eq!(&received[5 + 131_072..], b"DONE");
    assert!(last_receipt.unwrap().await.is_ok());
}

#[test]
fn full_chunks_still_retire_the_original_saturated_queue() {
    let params = parameters();
    let events = super::super::service_events::Events::default();
    let mut output = Output::default();
    let mut sequence = 0;
    let bytes = [0; OUTPUT_CHUNK_BYTES];
    for _ in 0..32 {
        output
            .push(
                &bytes,
                usize::MAX,
                OutputStream::Stdout,
                &params,
                &events,
                &mut sequence,
            )
            .unwrap();
    }
    assert!(!events.overloaded.is_cancelled());
    assert!(
        output
            .push(
                &bytes,
                usize::MAX,
                OutputStream::Stdout,
                &params,
                &events,
                &mut sequence
            )
            .is_err()
    );
    assert!(events.overloaded.is_cancelled());
}

#[tokio::test]
async fn exact_cap_flushes_a_short_tail_and_overflow_emits_one_empty_marker() {
    let params = parameters();
    let events = super::super::service_events::Events::default();
    let mut output = Output::default();
    let mut sequence = 0;
    assert!(
        output
            .push(
                b"12345678",
                8,
                OutputStream::Stdout,
                &params,
                &events,
                &mut sequence
            )
            .unwrap()
            .is_some()
    );
    let first = events.next().await;
    let NotificationMethod::CommandOutputDelta(first) = first.notification.notification else {
        panic!("output")
    };
    assert_eq!(first.sequence, 1);
    assert!(!first.cap_reached);
    assert_eq!(first.delta_base64, "MTIzNDU2Nzg=");
    assert!(
        output
            .push(
                b"overflow",
                8,
                OutputStream::Stdout,
                &params,
                &events,
                &mut sequence
            )
            .unwrap()
            .is_some()
    );
    let second = events.next().await;
    let NotificationMethod::CommandOutputDelta(second) = second.notification.notification else {
        panic!("output")
    };
    assert_eq!(second.sequence, 2);
    assert!(second.cap_reached);
    assert!(second.delta_base64.is_empty());
    assert!(
        output
            .push(
                b"discard",
                8,
                OutputStream::Stdout,
                &params,
                &events,
                &mut sequence
            )
            .unwrap()
            .is_none()
    );
    assert_eq!(sequence, 2);
    assert!(output.pending.is_empty());
}

#[test]
fn reaching_the_cap_cannot_hide_a_saturated_consumer_behind_a_pending_tail() {
    let params = parameters();
    let events = super::super::service_events::Events::default();
    let mut output = Output::default();
    let mut sequence = 0;
    let cap = 262_144;
    output
        .push(
            &[0],
            cap,
            OutputStream::Stdout,
            &params,
            &events,
            &mut sequence,
        )
        .unwrap();
    output
        .flush(false, OutputStream::Stdout, &params, &events, &mut sequence)
        .unwrap();
    // The first short frame and 31 full frames fill the original 32 slots.
    for _ in 0..31 {
        output
            .push(
                &[0; OUTPUT_CHUNK_BYTES],
                cap,
                OutputStream::Stdout,
                &params,
                &events,
                &mut sequence,
            )
            .unwrap();
    }
    assert_eq!(sequence, 32);
    assert!(!events.overloaded.is_cancelled());
    // No EOF, acknowledgement or timer may be needed to flush the cap tail.
    assert!(
        output
            .push(
                &[0; OUTPUT_CHUNK_BYTES - 1],
                cap,
                OutputStream::Stdout,
                &params,
                &events,
                &mut sequence
            )
            .is_err()
    );
    assert_eq!(output.total, cap);
    assert!(output.pending.is_empty());
    assert!(events.overloaded.is_cancelled());
}
