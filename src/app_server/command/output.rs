//! Per-operation verification of the Client's ordered output journal.
//!
//! Each delivered page is checked against the stream it names and the bytes
//! already retained for that stream before anything is buffered or announced.
//! The terminal snapshot is checked against what was actually delivered. A
//! contradiction is a protocol violation for the caller to retire on, never a
//! value to repair.

use rustx_execution_protocol::process::{
    OperationState, ProcessStream, SettledDisposition, SettledSnapshot,
};

const STDOUT: usize = 0;
const STDERR: usize = 1;

/// One verified page, exactly as the Client reported it.
pub(super) struct Delivery<'a> {
    pub(super) sequence: u64,
    pub(super) stream: ProcessStream,
    pub(super) offset: u64,
    pub(super) bytes: &'a [u8],
    pub(super) capped: bool,
    pub(super) last: bool,
}

/// What one operation's output must look like, accumulated page by page.
pub(super) struct OutputAccounting {
    pty: bool,
    cap: u64,
    /// The global ordered-output cursor: the total retained bytes so far.
    cursor: u64,
    /// Retained bytes per lane (`[stdout, stderr]`; PTY output is stdout).
    retained: [u64; 2],
    /// Lanes on which the Client has reported truncation at least once.
    capped: [bool; 2],
    /// Lanes whose truncation announcement has been queued.
    announced: [bool; 2],
    /// The final sequence the Client proved through `last` or `OutputEnd`.
    terminal: Option<u64>,
    settled: Option<SettledSnapshot>,
}

impl OutputAccounting {
    pub(super) fn new(pty: bool, cap: u32) -> Self {
        Self {
            pty,
            cap: u64::from(cap),
            cursor: 0,
            retained: [0; 2],
            capped: [false; 2],
            announced: [false; 2],
            terminal: None,
            settled: None,
        }
    }

    /// The cursor to request the next page from.
    pub(super) fn cursor(&self) -> u64 {
        self.cursor
    }

    fn lane(&self, stream: ProcessStream) -> Result<usize, &'static str> {
        match (stream, self.pty) {
            (ProcessStream::Pty, true) | (ProcessStream::Stdout, false) => Ok(STDOUT),
            (ProcessStream::Stderr, false) => Ok(STDERR),
            _ => Err("output stream does not belong to this operation's mode"),
        }
    }

    /// Admits one page, returning the lane it grew. Nothing changes on refusal.
    pub(super) fn admit(&mut self, delivery: &Delivery<'_>) -> Result<usize, &'static str> {
        if self.terminal.is_some() {
            return Err("output page follows the final entry");
        }
        let lane = self.lane(delivery.stream)?;
        let length = u64::try_from(delivery.bytes.len()).map_err(|_| "output page too long")?;
        if delivery.offset != self.retained[lane] {
            return Err("output offset does not continue the retained stream");
        }
        let retained = self.retained[lane]
            .checked_add(length)
            .ok_or("retained output overflows")?;
        if retained > self.cap {
            return Err("output exceeds the stream cap");
        }
        if self.cursor.checked_add(length) != Some(delivery.sequence) {
            return Err("output sequence does not advance by the page");
        }
        if self.capped[lane] && !delivery.capped {
            return Err("stream truncation was withdrawn");
        }
        self.retained[lane] = retained;
        self.cursor = delivery.sequence;
        self.capped[lane] |= delivery.capped;
        if delivery.last {
            self.terminal = Some(self.cursor);
        }
        Ok(lane)
    }

    /// Whether `lane`'s retained prefix is provably complete: it reached the
    /// cap and the Client has reported truncation, so no later byte of the lane
    /// can be retained. The truncation announcement belongs right after it.
    pub(super) fn prefix_complete(&self, lane: usize) -> bool {
        self.retained[lane] == self.cap && self.capped[lane]
    }

    /// Claims `lane`'s truncation announcement; `true` exactly once.
    pub(super) fn announce(&mut self, lane: usize) -> bool {
        !std::mem::replace(&mut self.announced[lane], true)
    }

    /// `OutputEnd` at the requested cursor proves the journal is complete.
    pub(super) fn end(&mut self, final_sequence: u64) -> Result<(), &'static str> {
        if self.terminal.is_some() || final_sequence != self.cursor {
            return Err("output end does not match the delivered cursor");
        }
        self.terminal = Some(self.cursor);
        Ok(())
    }

    /// A reply reporting the operation's state. Settlement is monotone: once the
    /// Client has settled, no later reply may describe it as unsettled.
    pub(super) fn observe(&mut self, state: &OperationState) -> Result<(), &'static str> {
        match state {
            OperationState::Settled(snapshot) => {
                if self.settled.is_some_and(|known| known != *snapshot) {
                    return Err("settled snapshot changed between replies");
                }
                if self
                    .terminal
                    .is_some_and(|end| end != snapshot.final_sequence.get())
                {
                    return Err("settled final sequence disagrees with the delivered terminal");
                }
                self.settled = Some(*snapshot);
                Ok(())
            }
            _ if self.settled.is_some() || self.terminal.is_some() => {
                Err("operation regressed from settled")
            }
            _ => Ok(()),
        }
    }

    /// A `OutputPending` answer is only honest while the journal can still grow.
    pub(super) fn pending_allowed(&self) -> Result<(), &'static str> {
        if self.settled.is_some() || self.terminal.is_some() {
            Err("output pending despite a settled journal")
        } else {
            Ok(())
        }
    }

    /// Checks the settled snapshot against every page actually delivered.
    pub(super) fn finish(&self, snapshot: &SettledSnapshot) -> Result<(), &'static str> {
        if self.settled != Some(*snapshot) {
            return Err("terminal snapshot was not observed");
        }
        let final_sequence = snapshot.final_sequence.get();
        if final_sequence != self.cursor {
            return Err("final sequence disagrees with the delivered cursor");
        }
        if self.retained[STDOUT].checked_add(self.retained[STDERR]) != Some(final_sequence) {
            return Err("final sequence disagrees with the retained bytes");
        }
        if snapshot.disposition == SettledDisposition::Unknown && snapshot.exit_code.is_some() {
            return Err("an unknown outcome carries an exit code");
        }
        if self.pty && (snapshot.stderr_total.get() != 0 || snapshot.stderr_truncated) {
            return Err("PTY operation reports pipe stderr");
        }
        for (lane, total, truncated) in [
            (
                STDOUT,
                snapshot.stdout_total.get(),
                snapshot.stdout_truncated,
            ),
            (
                STDERR,
                snapshot.stderr_total.get(),
                snapshot.stderr_truncated,
            ),
        ] {
            let retained = self.retained[lane];
            if total < retained {
                return Err("stream total is below its retained bytes");
            }
            if truncated != (total > retained) {
                return Err("stream truncation disagrees with its totals");
            }
            if self.capped[lane] && !truncated {
                return Err("reported truncation is missing from the settled snapshot");
            }
            if retained > self.cap {
                return Err("retained bytes exceed the stream cap");
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use rustx_execution_protocol::DecimalU64;
    use rustx_execution_protocol::process::{
        OperationState, ProcessStream, SettledDisposition, SettledSnapshot,
    };

    use super::*;

    fn delivery(sequence: u64, stream: ProcessStream, offset: u64, bytes: &[u8]) -> Delivery<'_> {
        Delivery {
            sequence,
            stream,
            offset,
            bytes,
            capped: false,
            last: false,
        }
    }

    /// A settled snapshot whose retained bytes are exactly `retained` per lane.
    fn settle(
        totals: (u64, u64),
        retained: (u64, u64),
        truncated: (bool, bool),
    ) -> SettledSnapshot {
        assert!(totals.0 >= retained.0 && totals.1 >= retained.1);
        SettledSnapshot {
            disposition: SettledDisposition::Known,
            physical_settlement_proven: true,
            exit_code: Some(0),
            stdout_total: DecimalU64::new(totals.0),
            stderr_total: DecimalU64::new(totals.1),
            stdout_truncated: truncated.0,
            stderr_truncated: truncated.1,
            final_sequence: DecimalU64::new(retained.0 + retained.1),
        }
    }

    #[test]
    fn many_tiny_fragments_sum_to_the_settled_totals() {
        let mut accounting = OutputAccounting::new(false, 4096);
        let mut lanes = [0_u64; 2];
        for index in 0..2_000_u64 {
            let (stream, lane) = if index % 3 == 0 {
                (ProcessStream::Stderr, STDERR)
            } else {
                (ProcessStream::Stdout, STDOUT)
            };
            let byte = [b'a' + u8::try_from(index % 26).unwrap()];
            let page = Delivery {
                sequence: accounting.cursor() + 1,
                stream,
                offset: lanes[lane],
                bytes: &byte,
                capped: false,
                last: false,
            };
            assert_eq!(accounting.admit(&page).unwrap(), lane);
            lanes[lane] += 1;
        }
        assert_eq!(lanes[0] + lanes[1], 2_000);
        accounting.end(2_000).unwrap();
        let settled = settle((lanes[0], lanes[1]), (lanes[0], lanes[1]), (false, false));
        accounting
            .observe(&OperationState::Settled(settled))
            .unwrap();
        accounting.finish(&settled).unwrap();
    }

    #[test]
    fn page_offsets_continue_each_stream_not_the_global_cursor() {
        let mut accounting = OutputAccounting::new(false, 64);
        accounting
            .admit(&delivery(2, ProcessStream::Stdout, 0, b"ab"))
            .unwrap();
        // Stderr starts at its own offset zero, not at the global cursor.
        accounting
            .admit(&delivery(3, ProcessStream::Stderr, 0, b"c"))
            .unwrap();
        assert!(
            accounting
                .admit(&delivery(4, ProcessStream::Stdout, 1, b"d"))
                .is_err()
        );
        accounting
            .admit(&delivery(4, ProcessStream::Stdout, 2, b"d"))
            .unwrap();
    }

    #[test]
    fn a_stream_outside_the_operation_mode_is_refused() {
        let mut pipes = OutputAccounting::new(false, 64);
        assert!(
            pipes
                .admit(&delivery(1, ProcessStream::Pty, 0, b"x"))
                .is_err()
        );
        let mut terminal = OutputAccounting::new(true, 64);
        assert!(
            terminal
                .admit(&delivery(1, ProcessStream::Stderr, 0, b"x"))
                .is_err()
        );
        assert_eq!(
            terminal.admit(&delivery(1, ProcessStream::Pty, 0, b"x")),
            Ok(0)
        );
    }

    #[test]
    fn cursor_regression_and_overflow_are_refused_without_changing_state() {
        let mut accounting = OutputAccounting::new(false, 64);
        accounting
            .admit(&delivery(4, ProcessStream::Stdout, 0, b"abcd"))
            .unwrap();
        assert!(
            accounting
                .admit(&delivery(3, ProcessStream::Stdout, 4, b"x"))
                .is_err()
        );
        assert!(
            accounting
                .admit(&delivery(u64::MAX, ProcessStream::Stdout, 4, b"x"))
                .is_err()
        );
        assert_eq!(accounting.cursor(), 4);
    }

    #[test]
    fn exact_cap_is_legitimate_and_one_byte_over_is_refused_then_truncated() {
        let mut exact = OutputAccounting::new(false, 4);
        exact
            .admit(&delivery(4, ProcessStream::Stdout, 0, b"abcd"))
            .unwrap();
        exact.end(4).unwrap();
        let settled = settle((4, 0), (4, 0), (false, false));
        exact.observe(&OperationState::Settled(settled)).unwrap();
        exact.finish(&settled).unwrap();

        let mut over = OutputAccounting::new(false, 4);
        over.admit(&delivery(4, ProcessStream::Stdout, 0, b"abcd"))
            .unwrap();
        // A fifth retained byte is never admitted.
        assert!(
            over.admit(&delivery(5, ProcessStream::Stdout, 4, b"e"))
                .is_err()
        );
        over.end(4).unwrap();
        let truncated = settle((5, 0), (4, 0), (true, false));
        over.observe(&OperationState::Settled(truncated)).unwrap();
        over.finish(&truncated).unwrap();
    }

    #[test]
    fn a_truncation_announcement_is_claimed_once_at_the_end_of_the_retained_prefix() {
        let mut accounting = OutputAccounting::new(false, 4);
        let mut first = delivery(4, ProcessStream::Stdout, 0, b"abcd");
        first.capped = true;
        accounting.admit(&first).unwrap();
        assert!(accounting.prefix_complete(STDOUT));
        assert!(accounting.announce(STDOUT));
        assert!(!accounting.announce(STDOUT));
        assert!(!accounting.prefix_complete(STDERR));
    }

    #[test]
    fn exact_cap_without_observed_truncation_is_not_announced_before_settlement() {
        let mut accounting = OutputAccounting::new(false, 4);
        accounting
            .admit(&delivery(4, ProcessStream::Stdout, 0, b"abcd"))
            .unwrap();
        assert!(!accounting.prefix_complete(STDOUT));
        accounting.end(4).unwrap();
        let settled = settle((4, 0), (4, 0), (false, false));
        accounting
            .observe(&OperationState::Settled(settled))
            .unwrap();
        accounting.finish(&settled).unwrap();
    }

    #[test]
    fn zero_output_settles_without_truncation() {
        let mut accounting = OutputAccounting::new(false, 16);
        accounting.end(0).unwrap();
        let settled = settle((0, 0), (0, 0), (false, false));
        accounting
            .observe(&OperationState::Settled(settled))
            .unwrap();
        accounting.finish(&settled).unwrap();
    }

    #[test]
    fn early_segment_truncation_is_legitimate_below_the_cap() {
        let mut accounting = OutputAccounting::new(false, 1024);
        let mut capped = delivery(3, ProcessStream::Stdout, 0, b"abc");
        capped.capped = true;
        accounting.admit(&capped).unwrap();
        accounting.end(3).unwrap();
        let settled = settle((9, 0), (3, 0), (true, false));
        accounting
            .observe(&OperationState::Settled(settled))
            .unwrap();
        accounting.finish(&settled).unwrap();
    }

    #[test]
    fn withdrawn_truncation_and_missing_snapshot_truncation_are_refused() {
        let mut accounting = OutputAccounting::new(false, 1024);
        let mut capped = delivery(1, ProcessStream::Stdout, 0, b"a");
        capped.capped = true;
        accounting.admit(&capped).unwrap();
        assert!(
            accounting
                .admit(&delivery(2, ProcessStream::Stdout, 1, b"b"))
                .is_err()
        );
        accounting.end(1).unwrap();
        let settled = settle((1, 0), (1, 0), (false, false));
        accounting
            .observe(&OperationState::Settled(settled))
            .unwrap();
        assert!(accounting.finish(&settled).is_err());
    }

    #[test]
    fn terminal_disagreements_are_refused() {
        let mut accounting = OutputAccounting::new(false, 64);
        accounting
            .admit(&delivery(2, ProcessStream::Stdout, 0, b"ab"))
            .unwrap();
        assert!(accounting.end(3).is_err());
        accounting.end(2).unwrap();
        let forged = SettledSnapshot {
            final_sequence: DecimalU64::new(9),
            ..settle((2, 0), (2, 0), (false, false))
        };
        assert!(
            accounting
                .observe(&OperationState::Settled(forged))
                .is_err()
        );
        assert!(
            accounting
                .admit(&delivery(3, ProcessStream::Stdout, 2, b"c"))
                .is_err()
        );
    }

    #[test]
    fn settlement_cannot_regress_or_be_described_as_pending() {
        let mut running = OutputAccounting::new(false, 64);
        running.observe(&OperationState::Running).unwrap();
        running.end(0).unwrap();
        assert!(running.observe(&OperationState::Running).is_err());
        assert!(running.pending_allowed().is_err());

        let mut pending = OutputAccounting::new(false, 64);
        pending.pending_allowed().unwrap();
        let settled = settle((0, 0), (0, 0), (false, false));
        pending.observe(&OperationState::Settled(settled)).unwrap();
        assert!(pending.pending_allowed().is_err());
    }

    #[test]
    fn an_unknown_outcome_never_carries_an_exit_code() {
        let unknown = SettledSnapshot {
            disposition: SettledDisposition::Unknown,
            ..settle((0, 0), (0, 0), (false, false))
        };
        let mut forged = OutputAccounting::new(false, 64);
        forged.end(0).unwrap();
        forged.observe(&OperationState::Settled(unknown)).unwrap();
        assert!(forged.finish(&unknown).is_err());

        let honest = SettledSnapshot {
            exit_code: None,
            ..unknown
        };
        let mut accounting = OutputAccounting::new(false, 64);
        accounting.end(0).unwrap();
        accounting
            .observe(&OperationState::Settled(honest))
            .unwrap();
        accounting.finish(&honest).unwrap();
    }

    #[test]
    fn a_pty_snapshot_reports_no_pipe_stderr() {
        let mut accounting = OutputAccounting::new(true, 64);
        accounting
            .admit(&delivery(1, ProcessStream::Pty, 0, b"x"))
            .unwrap();
        accounting.end(1).unwrap();
        let settled = settle((1, 4), (1, 0), (false, false));
        accounting
            .observe(&OperationState::Settled(settled))
            .unwrap();
        assert!(accounting.finish(&settled).is_err());
    }
}
