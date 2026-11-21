//! Per-request admission of typed process replies.
//!
//! A reply is accepted only when its variant, its echoed identity and
//! ordinal fields, and any base64 payload are exactly what its request can
//! honestly produce. Anything else is a protocol violation: the connection
//! that carried it is retired, never repaired or trusted.

use base64::Engine as _;
use rustx_execution_protocol::DecimalU64;
use rustx_execution_protocol::process::{
    MAX_PROCESS_DATA_BYTES, OperationState, ProcessReply, ProcessRequest,
};

/// Refuses a request the Server must never send. Nothing has crossed the
/// wire yet, so the caller fails locally and the connection stays healthy.
pub(super) fn validate_request(request: &ProcessRequest) -> Result<(), &'static str> {
    match request {
        ProcessRequest::OutputPage { max_bytes, .. } => {
            let limit = u32::try_from(MAX_PROCESS_DATA_BYTES).unwrap_or(u32::MAX);
            if (1..=limit).contains(max_bytes) {
                Ok(())
            } else {
                Err("output page bound is outside the process data limit")
            }
        }
        ProcessRequest::WriteStdin { delta_base64, .. } => {
            canonical_len(delta_base64, MAX_PROCESS_DATA_BYTES).map(drop)
        }
        _ => Ok(()),
    }
}

/// Accepts `reply` only as an honest answer to `request`.
pub(super) fn validate(request: &ProcessRequest, reply: &ProcessReply) -> Result<(), &'static str> {
    if reply.key() != request.key() {
        return Err("process reply echoes a foreign key");
    }
    match (request, reply) {
        (
            ProcessRequest::Register { .. } | ProcessRequest::RegisterAgentBash { .. },
            ProcessReply::Registered { .. }
            | ProcessReply::Conflict { .. }
            | ProcessReply::Unknown { .. }
            | ProcessReply::Refused { .. }
            | ProcessReply::State {
                state: OperationState::Settled(_),
                ..
            },
        )
        | (
            ProcessRequest::Start { .. } | ProcessRequest::Query { .. },
            ProcessReply::State { .. } | ProcessReply::Unknown { .. },
        )
        | (
            ProcessRequest::CloseStdin { .. }
            | ProcessRequest::WriteStdin { .. }
            | ProcessRequest::Resize { .. }
            | ProcessRequest::InputReceipt { .. },
            ProcessReply::Conflict { .. } | ProcessReply::Unknown { .. },
        )
        | (
            ProcessRequest::OutputPage { .. },
            ProcessReply::OutputPending { .. }
            | ProcessReply::Conflict { .. }
            | ProcessReply::Unknown { .. },
        )
        | (
            ProcessRequest::ReleaseOutput { .. } | ProcessRequest::DiscardBashOutput { .. },
            ProcessReply::OutputReleased { .. }
            | ProcessReply::Conflict { .. }
            | ProcessReply::Unknown { .. },
        )
        | (ProcessRequest::BashOutputTail { .. }, ProcessReply::Unknown { .. })
        | (
            ProcessRequest::BashCapture { .. },
            ProcessReply::Unknown { .. } | ProcessReply::Conflict { .. },
        ) => Ok(()),
        (ProcessRequest::BashOutputTail { .. }, ProcessReply::BashOutputTail { text, .. })
            if text.len() <= rustx_execution_protocol::process::BASH_PREVIEW_BYTES =>
        {
            Ok(())
        }
        (ProcessRequest::BashCapture { .. }, ProcessReply::BashCapture { capture, .. }) => {
            validate_bash_capture(capture.as_ref())
        }
        (
            ProcessRequest::OutputPage {
                cursor, max_bytes, ..
            },
            ProcessReply::Output {
                sequence,
                offset,
                delta_base64,
                ..
            },
        ) => output(
            cursor.get(),
            *max_bytes,
            sequence.get(),
            offset.get(),
            delta_base64,
        ),
        (
            ProcessRequest::OutputPage { cursor, .. },
            ProcessReply::OutputEnd { final_sequence, .. },
        ) => {
            if final_sequence.get() == cursor.get() {
                Ok(())
            } else {
                Err("output end does not sit at the requested cursor")
            }
        }
        (
            ProcessRequest::WriteStdin { sequence, .. }
            | ProcessRequest::CloseStdin { sequence, .. }
            | ProcessRequest::Resize { sequence, .. },
            ProcessReply::InputAccepted {
                sequence: echoed, ..
            },
        )
        | (
            ProcessRequest::InputReceipt { sequence, .. },
            ProcessReply::InputReceipt {
                sequence: echoed, ..
            },
        ) => echoes(*sequence, *echoed),
        _ => Err("process reply variant does not answer its request"),
    }
}

fn validate_bash_capture(
    capture: Option<&rustx_execution_protocol::process::BashCaptureResult>,
) -> Result<(), &'static str> {
    use rustx_execution_protocol::process::{BASH_PREVIEW_BYTES, BashCaptureState};
    let Some(capture) = capture else {
        return Ok(());
    };
    if [&capture.stdout, &capture.stderr, &capture.combined]
        .into_iter()
        .any(|text| text.len() > BASH_PREVIEW_BYTES)
    {
        return Err("Bash preview exceeds its bound");
    }
    if capture.total.is_none()
        != matches!(
            capture.output,
            BashCaptureState::Partial | BashCaptureState::Unavailable
        )
        || (capture.total.is_none() && !capture.truncated)
        || capture.signal.is_some_and(|signal| signal <= 0)
    {
        return Err("Bash capture completeness is inconsistent");
    }
    Ok(())
}

fn echoes(expected: DecimalU64, echoed: DecimalU64) -> Result<(), &'static str> {
    if expected == echoed {
        Ok(())
    } else {
        Err("process reply echoes a foreign input sequence")
    }
}

/// One output page: canonical bounded bytes that advance the cursor by exactly
/// their length, starting no later than the cursor in the stream's own offset
/// space.
fn output(
    cursor: u64,
    max_bytes: u32,
    sequence: u64,
    offset: u64,
    delta_base64: &str,
) -> Result<(), &'static str> {
    let length = canonical_len(
        delta_base64,
        usize::try_from(max_bytes).unwrap_or(usize::MAX),
    )?;
    if length == 0 {
        return Err("output page carries no bytes");
    }
    let length = u64::try_from(length).map_err(|_| "output page length overflows")?;
    if cursor.checked_add(length) != Some(sequence) {
        return Err("output sequence does not advance by the delivered bytes");
    }
    if offset > cursor {
        return Err("output stream offset exceeds the global cursor");
    }
    if offset.checked_add(length).is_none() {
        return Err("output stream offset overflows");
    }
    Ok(())
}

/// Decodes a canonical base64 payload no larger than `bound` raw bytes. The
/// encoded length is checked before decoding, and the decode must re-encode to
/// exactly the received text — padding and trailing bits included.
fn canonical_len(encoded: &str, bound: usize) -> Result<usize, &'static str> {
    if encoded.len() > bound.div_ceil(3) * 4 {
        return Err("base64 payload exceeds its bound");
    }
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(encoded)
        .map_err(|_| "base64 payload is malformed")?;
    if bytes.len() > bound {
        return Err("base64 payload exceeds its bound");
    }
    if base64::engine::general_purpose::STANDARD.encode(&bytes) != encoded {
        return Err("base64 payload is not canonical");
    }
    Ok(bytes.len())
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;
    use std::path::PathBuf;

    use rustx_execution_protocol::process::{
        InputOutcome, OperationId, ProcessOperationKey, ProcessProgram, ProcessSpec, ProcessStdin,
        ProcessStream, ServerIncarnation, SettledDisposition, SettledSnapshot,
    };
    use rustx_execution_protocol::{ClientInstanceId, Generation, SessionId};

    use super::*;

    fn key() -> ProcessOperationKey {
        ProcessOperationKey {
            incarnation: ServerIncarnation::generate(),
            session: SessionId::new("session-1"),
            instance: ClientInstanceId::new("client-1"),
            generation: Generation::try_from(1u64).unwrap(),
            operation: OperationId::new(),
        }
    }

    fn page(key: &ProcessOperationKey, cursor: u64, max_bytes: u32) -> ProcessRequest {
        ProcessRequest::OutputPage {
            key: key.clone(),
            cursor: DecimalU64::new(cursor),
            max_bytes,
        }
    }

    fn output_reply(
        key: &ProcessOperationKey,
        sequence: u64,
        offset: u64,
        delta_base64: &str,
    ) -> ProcessReply {
        ProcessReply::Output {
            key: key.clone(),
            sequence: DecimalU64::new(sequence),
            stream: ProcessStream::Stdout,
            offset: DecimalU64::new(offset),
            delta_base64: delta_base64.to_owned(),
            capped: false,
            last: false,
        }
    }

    fn settled() -> OperationState {
        OperationState::Settled(SettledSnapshot {
            disposition: SettledDisposition::Known,
            exit_code: Some(0),
            stdout_total: DecimalU64::new(0),
            stderr_total: DecimalU64::new(0),
            stdout_truncated: false,
            stderr_truncated: false,
            final_sequence: DecimalU64::new(0),
        })
    }

    #[test]
    fn a_foreign_key_is_refused_for_every_variant() {
        let request = page(&key(), 0, 16);
        let other = key();
        let reply = ProcessReply::OutputPending { key: other };
        assert_eq!(
            validate(&request, &reply),
            Err("process reply echoes a foreign key")
        );
    }

    #[test]
    fn registration_admits_only_its_honest_answers() {
        let key = key();
        let spec = ProcessSpec {
            cwd: PathBuf::new(),
            program: ProcessProgram::Argv(vec![String::from("/bin/true")]),
            env: BTreeMap::new(),
            timeout_ms: 1_000,
            pty: None,
            stdin: ProcessStdin::Closed,
            capture: true,
            output_cap: 1024,
        };
        let request = ProcessRequest::Register {
            key: key.clone(),
            fingerprint: spec.fingerprint(),
            spec,
        };
        for reply in [
            ProcessReply::Registered { key: key.clone() },
            ProcessReply::Conflict { key: key.clone() },
            ProcessReply::Unknown { key: key.clone() },
            ProcessReply::State {
                key: key.clone(),
                state: settled(),
            },
        ] {
            assert_eq!(validate(&request, &reply), Ok(()), "{reply:?}");
        }
        let running = ProcessReply::State {
            key: key.clone(),
            state: OperationState::Running,
        };
        assert!(validate(&request, &running).is_err());
        let output = output_reply(&key, 1, 0, "QQ==");
        assert!(validate(&request, &output).is_err());
    }

    #[test]
    fn start_and_query_answer_only_with_state_or_unknown() {
        let key = key();
        let start = ProcessRequest::Start { key: key.clone() };
        let query = ProcessRequest::Query { key: key.clone() };
        let state = ProcessReply::State {
            key: key.clone(),
            state: OperationState::Starting,
        };
        let unknown = ProcessReply::Unknown { key: key.clone() };
        let released = ProcessReply::OutputReleased { key: key.clone() };
        for request in [&start, &query] {
            assert_eq!(validate(request, &state), Ok(()));
            assert_eq!(validate(request, &unknown), Ok(()));
            assert!(validate(request, &released).is_err());
        }
    }

    #[test]
    fn an_honest_page_advances_the_cursor_by_its_exact_bytes() {
        let key = key();
        let request = page(&key, 3, 16);
        // "hello" is five raw bytes at stream offset 0; the cursor starts at 3.
        assert_eq!(
            validate(&request, &output_reply(&key, 8, 0, "aGVsbG8=")),
            Ok(())
        );
    }

    #[test]
    fn a_page_of_two_raw_bytes_for_one_requested_is_refused() {
        let key = key();
        let request = page(&key, 0, 1);
        // "AAA=" is a canonical encoding of two zero bytes.
        assert_eq!(
            validate(&request, &output_reply(&key, 2, 0, "AAA=")),
            Err("base64 payload exceeds its bound")
        );
    }

    #[test]
    fn malformed_and_noncanonical_base64_is_refused() {
        let key = key();
        let request = page(&key, 0, 16);
        for encoded in ["QQ", "QQ=", "AB==", "Q", "QQ==QQ=="] {
            assert!(
                validate(&request, &output_reply(&key, 1, 0, encoded)).is_err(),
                "{encoded} must not validate"
            );
        }
    }

    #[test]
    fn empty_and_forged_output_positions_are_refused() {
        let key = key();
        let request = page(&key, 4, 16);
        assert!(validate(&request, &output_reply(&key, 4, 0, "")).is_err());
        // The sequence must advance by exactly the delivered length.
        assert!(validate(&request, &output_reply(&key, 6, 0, "QQ==")).is_err());
        // A cursor regression or overflow is never a page.
        assert!(validate(&request, &output_reply(&key, 3, 0, "QQ==")).is_err());
        let overflow = page(&key, u64::MAX, 16);
        assert!(validate(&overflow, &output_reply(&key, u64::MAX, 0, "QQ==")).is_err());
        // The stream offset of a page can never lie beyond the global cursor.
        assert!(validate(&request, &output_reply(&key, 5, 9, "QQ==")).is_err());
    }

    #[test]
    fn output_end_must_sit_at_the_requested_cursor() {
        let key = key();
        let request = page(&key, 7, 16);
        let honest = ProcessReply::OutputEnd {
            key: key.clone(),
            final_sequence: DecimalU64::new(7),
        };
        let forged = ProcessReply::OutputEnd {
            key: key.clone(),
            final_sequence: DecimalU64::new(9),
        };
        assert_eq!(validate(&request, &honest), Ok(()));
        assert!(validate(&request, &forged).is_err());
    }

    #[test]
    fn input_acknowledgements_must_echo_their_sequence_and_variant() {
        let key = key();
        let write = ProcessRequest::WriteStdin {
            key: key.clone(),
            sequence: DecimalU64::new(2),
            delta_base64: String::from("QQ=="),
            close: false,
        };
        let accepted = |sequence| ProcessReply::InputAccepted {
            key: key.clone(),
            sequence: DecimalU64::new(sequence),
        };
        assert_eq!(validate(&write, &accepted(2)), Ok(()));
        assert!(validate(&write, &accepted(3)).is_err());
        assert!(validate(&write, &output_reply(&key, 1, 0, "QQ==")).is_err());

        let receipt = ProcessRequest::InputReceipt {
            key: key.clone(),
            sequence: DecimalU64::new(2),
            abandon_pending: false,
        };
        let applied = ProcessReply::InputReceipt {
            key: key.clone(),
            sequence: DecimalU64::new(2),
            status: InputOutcome::Applied,
        };
        assert_eq!(validate(&receipt, &applied), Ok(()));
        assert!(validate(&receipt, &accepted(2)).is_err());
    }

    #[test]
    fn release_answers_only_with_release_or_refusal() {
        let key = key();
        let release = ProcessRequest::ReleaseOutput {
            key: key.clone(),
            final_sequence: DecimalU64::new(0),
        };
        assert_eq!(
            validate(&release, &ProcessReply::OutputReleased { key: key.clone() }),
            Ok(())
        );
        assert!(validate(&release, &ProcessReply::OutputPending { key: key.clone() }).is_err());
    }

    #[test]
    fn request_bounds_are_checked_before_the_wire() {
        let key = key();
        assert!(validate_request(&page(&key, 0, 0)).is_err());
        assert!(validate_request(&page(&key, 0, 1)).is_ok());
        let limit = u32::try_from(MAX_PROCESS_DATA_BYTES).unwrap();
        assert!(validate_request(&page(&key, 0, limit)).is_ok());
        assert!(validate_request(&page(&key, 0, limit + 1)).is_err());
    }
}
