//! Typed data exchanges and validation on the admitted execution channel.

use super::*;

/// One admitted local connection. Dropping it disconnects the binding.
///
/// The only flows are typed exchanges the Server initiates: [`ping`](Self::ping)
/// liveness and [`read_range`](Self::read_range), the one implemented physical
/// operation. There is no raw message path. A transport, protocol, or
/// unexpected-reply failure retires the stream: it is never reused while a
/// reply could still be outstanding, and later calls fail closed.
pub struct LocalConnection {
    pub(super) owner: Option<Arc<ConnectionOwner>>,
    pub(super) delegated_work: Option<delegation::DelegatedWork>,
    /// The data lane — the only dispatch channel.
    pub(super) channel: RecordChannel,
    /// The paired control lane's worker, stopped exactly once.
    pub(super) control: Option<ControlWorker>,
    pub(super) usable: bool,
    /// Whether the orderly transport close was already sent: `close`
    /// and `Drop` never send it twice.
    pub(super) closed: bool,
    pub(super) capabilities: BTreeSet<Capability>,
    pub(super) user_root: PathBuf,
    pub(super) connection: ConnectionId,
    pub(super) admission: Admission,
}

/// A verified range reply from the Client.
///
/// The bytes were decoded and checked against the request bounds before
/// this value exists — the reply's own numbers are never trusted raw.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RangeRead {
    /// The returned bytes, already base64-decoded and bounded.
    pub bytes: Vec<u8>,
    /// The offset the bytes start at; equals the requested offset.
    pub offset: u64,
    /// The next offset to read, absent at end of file.
    pub next_offset: Option<u64>,
    /// The source-descriptor stamp of the file the bytes came from.
    pub metadata: FileStamp,
}

/// A bounded resource resolution and the paths that can invalidate it.
pub struct ResourceResolution {
    /// Verified physical target, if the Client found one.
    pub entry: Option<(PathBuf, EntryStamp)>,
    /// Workspace-relative watch dependencies.
    pub dependencies: Vec<PathBuf>,
}

/// A verified directory page reply from the Client.
///
/// Names, order, cursor, and every entry stamp were checked against the
/// request bounds before this value exists — the reply's own claims are
/// never trusted raw.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DirectoryListing {
    /// The revision the listing was verified under.
    pub revision: String,
    /// The page's entries in stable name order.
    pub entries: Vec<PageEntry>,
    /// The index the next page starts at, absent at the end.
    pub next_offset: Option<u32>,
    /// Hidden entries charged to an opted-in Skill discovery page.
    pub skipped: u32,
}

/// The result of one bounded Client continuation query.
pub enum ContinuationPage {
    /// A checked logical-text page.
    Lines(LinesPage),
    /// A checked byte range of a saved output.
    Range(RangeRead),
    /// A checked search page.
    Search(SearchResult),
}

/// A verified logical-text page reply from the Client.
///
/// The page's shown text, line and byte accounting, end reason, and
/// source stamp were checked against the request and the scanner's
/// complete-line contract before this value exists — the reply's own
/// claims are never trusted raw.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LinesPage {
    /// The shown text: the page's joined lines, bounded.
    pub text: String,
    /// The addressable line count of the whole source.
    pub total_lines: u64,
    /// The shown element count of this page.
    pub shown_lines: u32,
    /// Why the page ended.
    pub end: LineEnd,
    /// The source-descriptor stamp of the file the page came from.
    pub metadata: FileStamp,
}

/// A verified document page reply from the Client.
///
/// `source` stamps the physical document the page came from and
/// `logical_bytes` is the full decoded Markdown length — independent of
/// the compressed source's size. The page's shown text, line and byte
/// accounting, and end reason passed the same complete-line contract a
/// `Lines` reply answers to, so the reply's own claims are never trusted
/// raw.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DocumentPage {
    /// The source-descriptor stamp of the document the page came from.
    pub source: FileStamp,
    /// The byte length of the complete decoded Markdown text.
    pub logical_bytes: u64,
    /// The shown text: the page's joined lines, bounded.
    pub text: String,
    /// The addressable line count of the whole decoded text.
    pub total_lines: u64,
    /// The shown element count of this page.
    pub shown_lines: u32,
    /// Why the page ended.
    pub end: LineEnd,
}

/// A verified bounded-search reply from the Client.
///
/// `body` is the selected record text — newline-joined records only,
/// never notices — and `matches` the primary record count. The reply's
/// own bounds and accounting were checked against the request before
/// this value exists; record display bytes are never parsed back for
/// authority.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SearchResult {
    /// The selected records, at most `MAX_TEXT_BYTES` joined bytes.
    pub body: String,
    /// The selected primary record count.
    pub matches: u64,
    /// At least one match was excluded past the requested bound.
    pub hit_limit: bool,
    /// The joined-body byte budget stopped the selection.
    pub byte_limit: bool,
    /// At least one record preview was shortened — grep only.
    pub short_lines: bool,
}

/// This watch no longer covers its path and retains its native cleanup slot.
#[derive(Debug)]
pub struct WatchRetiring;

impl std::fmt::Display for WatchRetiring {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("watch cleanup is pending")
    }
}

impl std::error::Error for WatchRetiring {}

/// This MCP request was refused before it could reach the Execution Client.
#[derive(Debug)]
pub struct UnsentMcpExchange(io::Error);

impl std::fmt::Display for UnsentMcpExchange {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.0.fmt(formatter)
    }
}

impl std::error::Error for UnsentMcpExchange {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        Some(&self.0)
    }
}

enum ExchangeFailure {
    Refused(io::Error),
    Unknown(io::Error),
}

impl LocalConnection {
    /// The admission the peer was sent.
    #[must_use]
    pub fn admission(&self) -> &Admission {
        &self.admission
    }

    /// The User resource base as seen by this admitted peer.
    #[must_use]
    pub fn user_root(&self) -> &Path {
        &self.user_root
    }

    /// Whether this peer advertised a capability at admission.
    #[must_use]
    pub fn supports(&self, capability: Capability) -> bool {
        self.capabilities.contains(&capability)
    }

    /// Whether this data lane still belongs to the current authenticated pair.
    #[must_use]
    pub fn is_current(&self) -> bool {
        self.usable
            && !self.closed
            && self
                .owner
                .as_ref()
                .is_none_or(|owner| owner.is_live(&self.connection))
    }

    /// The server-generated identity of this connection.
    #[must_use]
    pub fn connection_id(&self) -> &ConnectionId {
        &self.connection
    }

    /// The permission gate, linearized under the binding mutex before any
    /// request byte is written: dispatch proceeds only while this exact
    /// connection is still the live one. A revocation or detach that wins
    /// the mutex first denies the exchange — no request is ever sent.
    fn gate(&self) -> io::Result<()> {
        if !self.usable {
            return Err(io::Error::new(
                io::ErrorKind::BrokenPipe,
                "connection retired after a protocol or transport failure",
            ));
        }
        let Some(owner) = &self.owner else {
            return if self.closed {
                Err(io::ErrorKind::BrokenPipe.into())
            } else {
                Ok(())
            };
        };
        if owner.permits_dispatch(&self.connection) {
            Ok(())
        } else {
            Err(io::Error::new(
                io::ErrorKind::PermissionDenied,
                "binding is not live on this connection",
            ))
        }
    }

    /// Orderly teardown: the transport's graceful close — a real
    /// RFC 6455 Close frame sent and flushed under the teardown budget,
    /// or the Unix stream's `shutdown(2)` — then the binding's live
    /// record disconnects and the channel is shut down. There is no
    /// wait for the peer's answering Close. Unlike `retire` the
    /// connection is not marked failed: a later dispatch still meets
    /// the binding's typed `PermissionDenied` denial, never a bare
    /// retired-connection error. Idempotent: an already orderly-closed
    /// connection only disconnects and shuts down.
    pub fn close(&mut self) {
        if self.usable && !self.closed {
            // Announce intentional shutdown to a Client that otherwise
            // treats a transport close as a possibly recoverable loss.
            let started = Instant::now();
            let cutoff = started + ORDERLY_CLOSE_BUDGET;
            if self.is_current()
                && self
                    .owner
                    .as_ref()
                    .is_some_and(|owner| !owner.revocation_confirmed())
                && let Some(control) = self.control()
            {
                let revoke_budget = ORDERLY_CLOSE_BUDGET
                    .saturating_sub(rustx_execution_protocol::transport::CONTROL_POLL_INTERVAL);
                let _ = control.request_revoke_by(started + revoke_budget);
            }
            self.closed = true;
            let _ = self.channel.close_graceful(Some(cutoff));
        }
        if let Some(control) = self.control.take() {
            control.stop();
        }
        if let Some(owner) = &self.owner {
            owner.disconnect(&self.connection, false).shutdown();
        }
        self.channel.shutdown();
    }

    /// The paired control lane's cloneable command handle — `None`
    /// after teardown takes the worker.
    #[must_use]
    pub fn control(&self) -> Option<ControlHandle> {
        self.control.as_ref().map(|control| control.handle.clone())
    }

    /// Marks the stream unusable, disconnects the binding, and shuts the
    /// socket down: after a protocol violation, truncation, or unexpected
    /// reply the peer's response accounting is unknowable, so the
    /// connection is torn down and never used again.
    pub(super) fn retire(&mut self, error: io::Error) -> io::Error {
        self.usable = false;
        if let Some(control) = self.control.take() {
            control.stop();
        }
        if let Some(owner) = &self.owner {
            owner
                .disconnect(
                    &self.connection,
                    rustx_execution_protocol::transport::recoverable_lane_loss(&error),
                )
                .shutdown();
        }
        self.channel.shutdown();
        error
    }

    /// Tears the connection down without a protocol verdict: the binding
    /// is disconnected and the socket shut, so the Client's drop clears
    /// every ticketed preparation it still holds. This is the fail-closed
    /// answer when a cleanup abort cannot reach the peer — the Session
    /// honestly loses execution rather than stranding bounded
    /// preparations while advertising a healthy binding.
    pub fn disconnect(&mut self) {
        let _ = self.retire(io::Error::other(
            "connection retired to clear stranded preparations",
        ));
    }

    /// Sends exactly one typed request and consumes its one typed reply.
    /// The request is encoded before the gate so an unencodable request
    /// fails without touching the binding or the wire; any transport
    /// failure after the gate retires the connection.
    pub(super) fn exchange(&mut self, request: &ClientRequest) -> io::Result<ClientReply> {
        self.exchange_admitted(request, None, || Ok(()), || ())
    }

    /// `exchange` with an optional absolute deadline and the caller's
    /// acceptance gate. The request is encoded and the binding's live
    /// check passes before `admit` runs: a refused admission — a
    /// cancelled caller — returns its error before a single request byte
    /// is written and never retires the connection. `sent` runs once the
    /// whole request frame is on the wire, before the reply is read.
    /// With `deadline`, the write and the read share one channel armed to
    /// the same absolute `Instant`; a timeout, partial reply, or any
    /// transport failure after admission retires the connection — the
    /// reply accounting is then unknowable, so there are no retries.
    pub(super) fn exchange_admitted(
        &mut self,
        request: &ClientRequest,
        deadline: Option<Instant>,
        admit: impl FnOnce() -> io::Result<()>,
        sent: impl FnOnce(),
    ) -> io::Result<ClientReply> {
        self.exchange_classified(request, deadline, admit, sent)
            .map_err(|failure| match failure {
                ExchangeFailure::Refused(error) | ExchangeFailure::Unknown(error) => error,
            })
    }

    fn exchange_classified(
        &mut self,
        request: &ClientRequest,
        deadline: Option<Instant>,
        admit: impl FnOnce() -> io::Result<()>,
        sent: impl FnOnce(),
    ) -> Result<ClientReply, ExchangeFailure> {
        let (encoded, deadline) = self
            .encode_exchange(request, deadline)
            .map_err(ExchangeFailure::Refused)?;
        self.gate().map_err(ExchangeFailure::Refused)?;
        admit().map_err(ExchangeFailure::Refused)?;
        if let Some(work) = &self.delegated_work {
            work.admit(request).map_err(ExchangeFailure::Refused)?;
        }
        self.exchange_encoded(&encoded, deadline, sent)
            .map_err(|failure| match failure {
                ExchangeFailure::Unknown(error) => ExchangeFailure::Unknown(self.retire(error)),
                refusal @ ExchangeFailure::Refused(_) => refusal,
            })
    }

    fn encode_exchange(
        &self,
        request: &ClientRequest,
        deadline: Option<Instant>,
    ) -> io::Result<(EncodedRecord, Option<Instant>)> {
        if self.owner.is_some() {
            return Ok((encode_record(request)?, deadline));
        }
        let deadline = deadline.unwrap_or_else(|| Instant::now() + PAGE_REQUEST_TIMEOUT);
        let remaining_ms = u32::try_from(
            deadline
                .saturating_duration_since(Instant::now())
                .as_millis(),
        )
        .unwrap_or(u32::MAX);
        let encoded = encode_record(&delegation::DataRequest {
            request: request.clone(),
            remaining_ms,
        })?;
        Ok((encoded, Some(deadline)))
    }

    fn exchange_encoded(
        &mut self,
        encoded: &EncodedRecord,
        deadline: Option<Instant>,
        sent: impl FnOnce(),
    ) -> Result<ClientReply, ExchangeFailure> {
        self.channel
            .send(encoded, deadline)
            .map_err(ExchangeFailure::Unknown)?;
        sent();
        if self.owner.is_some() {
            return self
                .channel
                .receive(deadline)
                .map_err(ExchangeFailure::Unknown);
        }
        let reply_deadline = deadline.map(|limit| limit + delegation::DATA_REPLY_BUDGET);
        match self
            .channel
            .receive(reply_deadline)
            .map_err(ExchangeFailure::Unknown)?
        {
            delegation::DataReply::Client(reply) => Ok(reply),
            delegation::DataReply::Rejected(error) => Err(ExchangeFailure::Refused(error.into())),
            delegation::DataReply::Unknown(error) => Err(ExchangeFailure::Unknown(error.into())),
        }
    }

    /// A liveness round-trip: `Ping` is answered by `Pong`.
    ///
    /// # Errors
    ///
    /// Returns an error when the binding no longer owns this connection, the
    /// exchange fails, or the reply is not a `Pong`; failures retire the
    /// stream.
    pub fn ping(&mut self) -> io::Result<()> {
        match self.exchange(&ClientRequest::Ping)? {
            ClientReply::Pong => Ok(()),
            _ => Err(self.retire(io::Error::new(
                io::ErrorKind::InvalidData,
                "unexpected reply to ping",
            ))),
        }
    }

    /// The one physical operation: a bounded, revision-checked range read of
    /// `relative_path` under the binding workspace.
    ///
    /// The peer must have advertised [`Capability::ReadRange`] at admission
    /// or the call is [`io::ErrorKind::Unsupported`] and no request is sent.
    /// `max_bytes` must be within `1..=`[`MAX_RAW_CHUNK_BYTES`]. The reply is
    /// trusted only after validation: the base64 must decode to at most
    /// `max_bytes` (its encoded length is checked before decoding), the
    /// offset must echo the request, the revision must be a 64-character
    /// lowercase hexadecimal stamp, a `next_offset` must continue the range
    /// exactly (and then the full bound was delivered), and a supplied
    /// `expected_revision` must match the returned revision. A violation
    /// retires the stream — it is never reused with a reply accounting that
    /// cannot be trusted.
    ///
    /// # Errors
    ///
    /// Returns [`io::ErrorKind::Unsupported`] when the peer lacks the range
    /// capability, [`io::ErrorKind::InvalidInput`] for an out-of-range
    /// request, the [`ReadError`] the Client reported mapped onto an I/O
    /// error kind, or [`io::ErrorKind::InvalidData`] for a malformed or
    /// untruthful reply.
    pub fn read_range(
        &mut self,
        relative_path: &Path,
        offset: u64,
        max_bytes: u32,
        expected_revision: Option<&str>,
    ) -> io::Result<RangeRead> {
        self.read_range_admitted(
            relative_path,
            offset,
            max_bytes,
            expected_revision,
            Instant::now() + PAGE_REQUEST_TIMEOUT,
            || Ok(()),
            || (),
        )
    }

    /// `read_range` against a caller-armed absolute deadline and
    /// acceptance gate, under the same contract as
    /// [`read_lines_admitted`](Self::read_lines_admitted): one wire
    /// budget, an admission refusal that returns before any request byte
    /// without retiring, and a `sent` observation between the written
    /// frame and the reply.
    ///
    /// # Errors
    ///
    /// Returns the same kinds as [`read_range`](Self::read_range), plus
    /// the `admit` error verbatim and [`io::ErrorKind::TimedOut`] when
    /// the deadline lapses mid-exchange — which retires the connection.
    #[allow(clippy::too_many_arguments)] // one admitted exchange's full context
    pub fn read_range_admitted(
        &mut self,
        relative_path: &Path,
        offset: u64,
        max_bytes: u32,
        expected_revision: Option<&str>,
        deadline: Instant,
        admit: impl FnOnce() -> io::Result<()>,
        sent: impl FnOnce(),
    ) -> io::Result<RangeRead> {
        if !self.capabilities.contains(&Capability::ReadRange) {
            return Err(io::Error::new(
                io::ErrorKind::Unsupported,
                "peer admitted no range capability",
            ));
        }
        let bound = usize::try_from(max_bytes).unwrap_or(usize::MAX);
        if !(1..=MAX_RAW_CHUNK_BYTES).contains(&bound) {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "range read exceeds chunk limit",
            ));
        }
        let reply = self.exchange_admitted(
            &ClientRequest::ReadRange {
                relative_path: relative_path.to_path_buf(),
                offset: DecimalU64::new(offset),
                max_bytes,
                expected_revision: expected_revision.map(str::to_owned),
            },
            Some(deadline),
            admit,
            sent,
        )?;
        if let ClientReply::Error { kind } = reply {
            return Err(read_error(kind));
        }
        self.validate_range(offset, bound, expected_revision, &reply)
    }

    /// One bounded whole-file fetch assembled from `ReadRange` chunks on
    /// this connection. `admit` is the caller's acceptance gate run after each
    /// request is encoded and the binding's live check passes, before
    /// that request's first byte; every chunk shares the one absolute
    /// `deadline`.
    ///
    /// The first chunk's stamp is pinned whole — `size`, `identity`,
    /// `revision`, `mtime` — and its `size` must not exceed `bound`
    /// before a single follow-up request is issued: an oversized source
    /// is an honest [`io::ErrorKind::FileTooLarge`] refusal that never
    /// retires the connection, exactly like a typed `Stale` or
    /// `conflict` from the peer. Every later request names the pinned
    /// revision; a reply stamped differently — or assembled bytes whose
    /// total contradicts the pinned size — means the peer cannot
    /// account for its answers, which retires the stream. At most
    /// `ceil(bound / MAX_RAW_CHUNK_BYTES)` requests are ever made for
    /// one call; there are no retries.
    ///
    /// # Errors
    ///
    /// Returns the same kinds as [`read_range`](Self::read_range), plus
    /// [`io::ErrorKind::FileTooLarge`] for a source larger than `bound`
    /// and the `admit` error verbatim.
    pub fn read_bounded_file(
        &mut self,
        relative_path: &Path,
        bound: u64,
        deadline: Instant,
        admit: impl Fn() -> io::Result<()>,
    ) -> io::Result<(Vec<u8>, FileStamp)> {
        let violation = |message: &'static str| io::Error::new(io::ErrorKind::InvalidData, message);
        let limit = usize::try_from(bound).unwrap_or(usize::MAX);
        let chunk_bound = u32::try_from(MAX_RAW_CHUNK_BYTES).unwrap_or(u32::MAX);
        let first =
            self.read_range_admitted(relative_path, 0, chunk_bound, None, deadline, &admit, || ())?;
        let stamp = first.metadata;
        let size = stamp.size.get();
        if size > bound {
            return Err(io::Error::new(
                io::ErrorKind::FileTooLarge,
                "source exceeds the transfer bound",
            ));
        }
        let mut bytes = first.bytes;
        let mut next = first.next_offset;
        while let Some(offset) = next {
            let chunk = self
                .read_range_admitted(
                    relative_path,
                    offset,
                    chunk_bound,
                    Some(&stamp.revision),
                    deadline,
                    &admit,
                    || (),
                )
                .map_err(|error| {
                    if error.kind() == io::ErrorKind::NotFound {
                        read_error(ReadError::Conflict)
                    } else {
                        error
                    }
                })?;
            if chunk.metadata != stamp {
                return Err(self.retire(violation("source stamp changed between chunks")));
            }
            if bytes.len() + chunk.bytes.len() > limit {
                return Err(self.retire(violation("chunks exceed the stamped bound")));
            }
            bytes.extend_from_slice(&chunk.bytes);
            next = chunk.next_offset;
        }
        if bytes.len() as u64 != size {
            return Err(self.retire(violation("chunks contradict the stamped size")));
        }
        Ok((bytes, stamp))
    }

    /// One managed-process exchange: the typed request is sent and its
    /// typed reply returned only after the exact key echo and the
    /// bounded wire shapes are verified — an `Output` page's encoded
    /// payload is length-checked against `MAX_PROCESS_DATA_BYTES`
    /// before the reply is trusted, and a reply answering any other key
    /// or shape retires the stream.
    ///
    /// The peer must have advertised [`Capability::Process`] at
    /// admission or the call is [`io::ErrorKind::Unsupported`] and no
    /// request is sent. Each call is one complete request/reply pair —
    /// the caller observes operation progress through successive
    /// `Query`/`OutputPage`/`InputReceipt` exchanges, so no exchange
    /// ever awaits a process lifetime.
    ///
    /// # Errors
    ///
    /// Returns [`io::ErrorKind::Unsupported`] when the peer lacks the
    /// process capability, [`io::ErrorKind::InvalidData`] for a reply
    /// that fails the key echo or the bounded-shape checks — which
    /// retires the stream — or the transport's own errors. When the
    /// request may have reached the Client without a verified reply,
    /// the error carries [`ProcessOutcomeUnknown`].
    pub fn process(
        &mut self,
        request: &rustx_execution_protocol::process::ProcessRequest,
    ) -> io::Result<rustx_execution_protocol::process::ProcessReply> {
        self.process_until(request, Instant::now() + PAGE_REQUEST_TIMEOUT)
    }

    /// [`Self::process`] under one absolute `deadline` covering the wire
    /// exchange. A deadline already passed refuses before anything is sent.
    ///
    /// # Errors
    ///
    /// As [`Self::process`], plus [`io::ErrorKind::TimedOut`] for a deadline
    /// that passed before the request was sent.
    pub fn process_until(
        &mut self,
        request: &rustx_execution_protocol::process::ProcessRequest,
        deadline: Instant,
    ) -> io::Result<rustx_execution_protocol::process::ProcessReply> {
        self.process_admitted(request, deadline, || Ok(()))
    }

    /// One process exchange with a cancellation gate immediately before send.
    ///
    /// # Errors
    ///
    /// Returns the same errors as [`Self::process_until`], or the gate's refusal.
    pub fn process_admitted(
        &mut self,
        request: &rustx_execution_protocol::process::ProcessRequest,
        deadline: Instant,
        admit: impl FnOnce() -> io::Result<()>,
    ) -> io::Result<rustx_execution_protocol::process::ProcessReply> {
        if Instant::now() >= deadline {
            return Err(io::Error::new(
                io::ErrorKind::TimedOut,
                "process deadline passed before dispatch",
            ));
        }
        if !self.capabilities.contains(&Capability::Process) {
            return Err(io::Error::new(
                io::ErrorKind::Unsupported,
                "peer admitted no process capability",
            ));
        }
        if matches!(
            request,
            rustx_execution_protocol::process::ProcessRequest::RegisterAgentBash { .. }
        ) && !self.capabilities.contains(&Capability::AgentBash)
        {
            return Err(io::Error::new(
                io::ErrorKind::Unsupported,
                "peer admitted no Agent Bash capability",
            ));
        }
        process_reply::validate_request(request)
            .map_err(|message| io::Error::new(io::ErrorKind::InvalidInput, message))?;
        let reply = self
            .exchange_classified(
                &ClientRequest::Process(request.clone()),
                Some(deadline),
                admit,
                || (),
            )
            .map_err(|failure| match failure {
                ExchangeFailure::Refused(error) => error,
                ExchangeFailure::Unknown(error) => ProcessOutcomeUnknown::wrap(error),
            })?;
        let verified = match reply {
            ClientReply::Process(reply) => process_reply::validate(request, &reply).map(|()| reply),
            _ => Err("unexpected reply to process request"),
        };
        verified.map_err(|message| {
            self.retire(ProcessOutcomeUnknown::wrap(io::Error::new(
                io::ErrorKind::InvalidData,
                message,
            )))
        })
    }

    /// Execute an optional output save over the existing admitted data lane.
    ///
    /// # Errors
    ///
    /// Capability, request-admission, transport or reply-integrity failure.
    pub fn save_by(
        &mut self,
        request: &rustx_execution_protocol::save::SaveRequest,
        deadline: Instant,
        admit: impl FnOnce() -> io::Result<()>,
    ) -> io::Result<rustx_execution_protocol::save::SaveReply> {
        use rustx_execution_protocol::save::{SaveReply, SaveRequest, decode_chunk};

        if !self.supports(Capability::ManagedOutput) {
            return Err(io::ErrorKind::Unsupported.into());
        }
        let reply = self.exchange_admitted(
            &ClientRequest::Save(request.clone()),
            Some(deadline),
            admit,
            || (),
        )?;
        let ClientReply::Save(reply) = reply else {
            return Err(self.retire(io::Error::new(
                io::ErrorKind::InvalidData,
                "unexpected save reply",
            )));
        };
        let valid = match (request, &reply) {
            (_, SaveReply::Refused { .. }) => true,
            (SaveRequest::Begin { owner, .. }, SaveReply::Begun { key }) => owner == &key.owner,
            (
                SaveRequest::Chunk {
                    key,
                    offset,
                    sequence,
                    data_base64,
                    ..
                },
                SaveReply::Chunked {
                    key: received,
                    sequence: echoed,
                    next_offset,
                    ..
                },
            ) => {
                key == received
                    && sequence == echoed
                    && decode_chunk(data_base64).is_ok_and(|data| {
                        offset.get().checked_add(data.len() as u64) == Some(next_offset.get())
                    })
            }
            (
                SaveRequest::Seal {
                    key,
                    length,
                    sha256,
                    extent,
                },
                SaveReply::Sealed {
                    key: received,
                    length: actual,
                    sha256: digest,
                    extent: saved,
                },
            )
            | (
                SaveRequest::Publish {
                    key,
                    length,
                    sha256,
                    extent,
                },
                SaveReply::Published {
                    key: received,
                    length: actual,
                    sha256: digest,
                    extent: saved,
                    ..
                },
            ) => (key, length, sha256, extent) == (received, actual, digest, saved),
            (SaveRequest::Abort { key }, SaveReply::Aborted { key: received, .. })
            | (SaveRequest::Probe { key }, SaveReply::Readability { key: received, .. }) => {
                key == received
            }
            _ => false,
        };
        if valid {
            Ok(reply)
        } else {
            Err(self.retire(io::Error::new(
                io::ErrorKind::InvalidData,
                "save reply contradicts its request",
            )))
        }
    }

    /// Exchanges one message with this binding's stdio MCP session.
    ///
    /// # Errors
    ///
    /// Missing capability, a pre-dispatch refusal, lost connection or
    /// contradictory operation identity.
    pub fn python_source_by(
        &mut self,
        request: &rustx_execution_protocol::mcp::PythonSourceRequest,
        deadline: Instant,
    ) -> io::Result<rustx_execution_protocol::mcp::PythonSourceReply> {
        self.python_source_admitted(request, deadline, || Ok(()))
    }

    /// Dispatches a Python source exchange after the caller's dispatch grant.
    ///
    /// # Errors
    /// Returns grant, transport, correlation and capability failures.
    pub fn python_source_admitted(
        &mut self,
        request: &rustx_execution_protocol::mcp::PythonSourceRequest,
        deadline: Instant,
        admit: impl FnOnce() -> io::Result<()>,
    ) -> io::Result<rustx_execution_protocol::mcp::PythonSourceReply> {
        if !self.supports(Capability::ManagedPython) {
            return Err(io::ErrorKind::Unsupported.into());
        }
        match self.exchange_admitted(
            &ClientRequest::PythonSource(request.clone()),
            Some(deadline),
            admit,
            || (),
        )? {
            ClientReply::PythonSource(reply) => Ok(reply),
            _ => Err(self.retire(io::Error::new(
                io::ErrorKind::InvalidData,
                "invalid Python source reply",
            ))),
        }
    }

    /// Read only explicitly Client-granted names for one HTTP MCP source.
    ///
    /// `admit` runs on the connection immediately before writing the request,
    /// matching other bounded exchanges so a pre-wire refusal sends nothing and
    /// leaves the live connection intact.
    ///
    /// # Errors
    /// Returns an admission refusal, denial, transport failure, or invalid reply.
    pub fn mcp_environment(
        &mut self,
        request: &rustx_execution_protocol::mcp_environment::Request,
        deadline: Instant,
        admit: impl FnOnce() -> io::Result<()>,
    ) -> io::Result<std::collections::BTreeMap<String, String>> {
        use rustx_execution_protocol::mcp_environment::MAX_VALUE_BYTES;
        if !request.valid() {
            return Err(io::ErrorKind::InvalidInput.into());
        }
        match self.exchange_admitted(
            &ClientRequest::McpEnvironment(request.clone()),
            Some(deadline),
            admit,
            || (),
        )? {
            ClientReply::McpEnvironment(reply) => {
                let total = reply
                    .values
                    .values()
                    .try_fold(0usize, |sum, value| sum.checked_add(value.len()));
                if reply
                    .values
                    .keys()
                    .cloned()
                    .collect::<std::collections::BTreeSet<_>>()
                    != request.names
                    || total.is_none_or(|bytes| bytes > MAX_VALUE_BYTES)
                    || reply
                        .values
                        .values()
                        .any(|value| value.is_empty() || value.contains('\0'))
                {
                    return Err(self.retire(io::ErrorKind::InvalidData.into()));
                }
                Ok(reply.values)
            }
            ClientReply::Error {
                kind: rustx_execution_protocol::ReadError::Denied,
            } => Err(io::ErrorKind::PermissionDenied.into()),
            _ => Err(self.retire(io::ErrorKind::InvalidData.into())),
        }
    }

    /// One typed exchange on the admitted Client's stdio MCP session.
    ///
    /// # Errors
    /// Returns a physical refusal, transport failure, or expired deadline.
    pub fn mcp_by(
        &mut self,
        request: rustx_execution_protocol::mcp::Request,
        deadline: Instant,
        admit: impl FnOnce() -> io::Result<()>,
    ) -> io::Result<rustx_execution_protocol::mcp::Reply> {
        use rustx_execution_protocol::mcp::{
            Failure, MAX_STARTUP_STDERR_BYTES, Reply, Request, StartupStderr,
        };

        if !self.supports(Capability::StdioMcp) {
            return Err(io::ErrorKind::Unsupported.into());
        }
        let key = request.key().clone();
        let state = matches!(&request, Request::Open { .. } | Request::State { .. });
        let close = matches!(&request, Request::Close { .. });
        let send = matches!(&request, Request::Send { .. });
        let page = match &request {
            Request::Receive {
                sequence, offset, ..
            } => Some((*sequence, *offset)),
            _ => None,
        };
        let reply = self
            .exchange_classified(&ClientRequest::Mcp(request), Some(deadline), admit, || ())
            .map_err(|failure| match failure {
                ExchangeFailure::Refused(error) => {
                    io::Error::new(error.kind(), UnsentMcpExchange(error))
                }
                ExchangeFailure::Unknown(error) => error,
            })?;
        if let ClientReply::Mcp(response) = reply
            && response.key == key
            && match &response.reply {
                Reply::Failed(_) | Reply::Closed => true,
                Reply::StartupFailed { failure, stderr } => {
                    state
                        && matches!(
                            failure,
                            Failure::Handshake
                                | Failure::UnsupportedRevision
                                | Failure::InvalidRecord
                                | Failure::Transport
                        )
                        && match stderr {
                            StartupStderr::Complete { text } => {
                                text.len() <= MAX_STARTUP_STDERR_BYTES
                            }
                            StartupStderr::Incomplete => true,
                        }
                }
                Reply::Opening | Reply::Ready { .. } => state,
                Reply::Closing => close || state,
                Reply::Queued => send,
                Reply::Pending => page.is_some(),
                Reply::Page {
                    sequence, offset, ..
                } => page == Some((*sequence, *offset)),
            }
        {
            return Ok(response.reply);
        }
        Err(self.retire(io::Error::new(
            io::ErrorKind::InvalidData,
            "stdio MCP reply contradicts its request",
        )))
    }

    /// Run one bounded Read or Grep over the authorized Client-owned continuation.
    ///
    /// # Errors
    ///
    /// Returns capability, cancellation, transport or typed read/search errors.
    pub fn inspect_output(
        &mut self,
        request: &ClientRequest,
        deadline: Instant,
        admit: impl FnOnce() -> io::Result<()>,
    ) -> io::Result<ContinuationPage> {
        let capability = match request {
            ClientRequest::ReadBashOutput { .. }
            | ClientRequest::SearchBashOutput {
                spec: SearchSpec::Grep { .. },
                ..
            } => Capability::AgentBash,
            ClientRequest::ReadSavedOutput { .. }
            | ClientRequest::ReadSavedOutputRange { .. }
            | ClientRequest::SearchSavedOutput {
                spec: SearchSpec::Grep { .. },
                ..
            } => Capability::ManagedOutput,
            _ => return Err(io::ErrorKind::InvalidInput.into()),
        };
        if !self.supports(capability) {
            return Err(io::ErrorKind::Unsupported.into());
        }
        let reply = self.exchange_admitted(request, Some(deadline), admit, || ())?;
        if let ClientReply::Error { kind } = reply {
            return Err(read_error(kind));
        }
        match request {
            ClientRequest::ReadBashOutput { offset, limit, .. }
            | ClientRequest::ReadSavedOutput { offset, limit, .. } => self
                .validate_lines(offset.get(), limit.map(DecimalU64::get), None, &reply)
                .map(ContinuationPage::Lines),
            ClientRequest::ReadSavedOutputRange { offset, .. } => self
                .validate_range(
                    offset.get(),
                    rustx_execution_protocol::MAX_TEXT_BYTES,
                    None,
                    &reply,
                )
                .map(ContinuationPage::Range),
            ClientRequest::SearchBashOutput {
                spec: SearchSpec::Grep { context, limit, .. },
                ..
            }
            | ClientRequest::SearchSavedOutput {
                spec: SearchSpec::Grep { context, limit, .. },
                ..
            } => self
                .validate_search(false, *context, *limit, &reply)
                .map(ContinuationPage::Search),
            _ => unreachable!("Continuation exchange must name Read or Grep"),
        }
    }

    /// Verifies a `Range` reply against the request before trusting it. Any
    /// deviation means the peer cannot account for its answers honestly, so
    /// the connection is retired.
    fn validate_range(
        &mut self,
        offset: u64,
        bound: usize,
        expected_revision: Option<&str>,
        reply: &ClientReply,
    ) -> io::Result<RangeRead> {
        let violation = |message: &'static str| io::Error::new(io::ErrorKind::InvalidData, message);
        let ClientReply::Range {
            data_base64,
            offset: reply_offset,
            next_offset,
            metadata,
        } = reply
        else {
            return Err(self.retire(violation("unexpected reply to range read")));
        };
        // The encoding inflates by 4/3: anything longer cannot decode to
        // a permitted range, so it is refused before decoding.
        if data_base64.len() > bound.div_ceil(3) * 4 {
            return Err(self.retire(violation("range reply exceeds requested bound")));
        }
        let bytes = match base64::engine::general_purpose::STANDARD.decode(data_base64) {
            Ok(bytes) if bytes.len() <= bound => bytes,
            _ => return Err(self.retire(violation("range reply exceeds requested bound"))),
        };
        if reply_offset.get() != offset {
            return Err(self.retire(violation("range reply offset does not echo request")));
        }
        if let Some(reason) = stamp_violation(metadata) {
            return Err(self.retire(violation(reason)));
        }
        if let Some(expected) = expected_revision
            && metadata.revision != expected
        {
            return Err(self.retire(violation("range reply revision ignores expectation")));
        }
        let Ok(length) = u64::try_from(bytes.len()) else {
            return Err(self.retire(violation("range length")));
        };
        let Some(end) = offset.checked_add(length) else {
            return Err(self.retire(violation("range reply overflows the offset space")));
        };
        let size = metadata.size.get();
        if offset > size
            || end > size
            || (*next_offset).map(DecimalU64::get) != (end < size).then_some(end)
            || (end < size && bytes.len() != bound)
        {
            return Err(self.retire(violation("range reply contradicts file size")));
        }
        Ok(RangeRead {
            bytes,
            offset,
            next_offset: (*next_offset).map(DecimalU64::get),
            metadata: metadata.clone(),
        })
    }

    /// Run one bounded tree mutation, subject to the final pre-wire authorization.
    ///
    /// # Errors
    ///
    /// Returns a clean error when the capability is missing, authorization fails,
    /// or the Client proves no named change. After admission, a transport failure
    /// or an uncertain reply returns [`WriteOutcomeUnknown`].
    pub fn mutate_tree(
        &mut self,
        mutation: TreeMutation,
        authorize: impl FnOnce() -> io::Result<()>,
    ) -> io::Result<()> {
        if !self.capabilities.contains(&Capability::TreeMutations) {
            return Err(io::Error::new(
                io::ErrorKind::Unsupported,
                "peer admitted no tree mutation capability",
            ));
        }
        let admitted = std::cell::Cell::new(false);
        let reply = self.exchange_admitted(
            &ClientRequest::MutateTree(mutation),
            Some(Instant::now() + PAGE_REQUEST_TIMEOUT),
            || {
                authorize()?;
                admitted.set(true);
                Ok(())
            },
            || (),
        );
        match reply {
            Ok(ClientReply::TreeMutated) => Ok(()),
            Ok(ClientReply::Error { kind }) => Err(read_error(kind)),
            Ok(_) => Err(self.retire(io::Error::other(WriteOutcomeUnknown))),
            Err(_) if admitted.get() => Err(io::Error::other(WriteOutcomeUnknown)),
            Err(error) => Err(error),
        }
    }

    /// Register a descriptor watch under this Session's live binding.
    ///
    /// # Errors
    ///
    /// Refuses a missing capability, a pre-wire authorization denial or a
    /// typed physical refusal. An ambiguous registration retires the pair.
    pub fn register_watch(
        &mut self,
        id: WatchId,
        path: &Path,
        authorize: impl FnOnce() -> io::Result<()>,
    ) -> io::Result<()> {
        if !self.capabilities.contains(&Capability::FileWatches) {
            return Err(io::ErrorKind::Unsupported.into());
        }
        let admitted = std::cell::Cell::new(false);
        let reply = self.exchange_admitted(
            &ClientRequest::Watch(WatchRequest::Register {
                id,
                path: path.to_path_buf(),
            }),
            Some(Instant::now() + PAGE_REQUEST_TIMEOUT),
            || {
                authorize()?;
                admitted.set(true);
                Ok(())
            },
            || (),
        );
        match reply {
            Ok(ClientReply::Watch(WatchReply::Registered { id: echoed })) if echoed == id => Ok(()),
            Ok(ClientReply::Watch(WatchReply::Refused { id: echoed, kind })) if echoed == id => {
                Err(read_error(kind))
            }
            Ok(_) => Err(self.retire(io::Error::other(WatchOutcomeUnknown))),
            Err(_) if admitted.get() => Err(io::Error::other(WatchOutcomeUnknown)),
            Err(error) => Err(error),
        }
    }

    /// Poll exactly one live native watcher for bounded, workspace-relative hints.
    ///
    /// # Errors
    ///
    /// The receipt must describe paths under the registered source. A malformed
    /// receipt or uncertain physical retirement retires the exact binding.
    pub fn poll_watch(&mut self, id: WatchId, source: &Path) -> io::Result<WatchChange> {
        let result = self.exchange_admitted(
            &ClientRequest::Watch(WatchRequest::Poll { id }),
            Some(Instant::now() + PAGE_REQUEST_TIMEOUT),
            || Ok(()),
            || (),
        )?;
        let notice = match result {
            ClientReply::Watch(WatchReply::Changed(notice)) if notice.id == id => notice,
            ClientReply::Watch(WatchReply::Retiring { id: echoed }) if echoed == id => {
                return Err(io::Error::new(io::ErrorKind::WouldBlock, WatchRetiring));
            }
            ClientReply::Watch(WatchReply::Refused { id: echoed, kind }) if echoed == id => {
                return Err(read_error(kind));
            }
            _ => {
                return Err(self.retire(io::Error::new(
                    io::ErrorKind::InvalidData,
                    "watch receipt did not prove a change or retirement",
                )));
            }
        };
        let valid = notice.changed_paths.len() <= MAX_WATCH_HINT_PATHS
            && (!notice.retired || notice.resync_required && notice.changed_paths.is_empty())
            && notice.changed_paths.iter().all(|path| {
                !path.is_absolute()
                    && path.starts_with(source)
                    && path.to_str().is_some_and(|path| {
                        !path.bytes().any(|byte| byte == 0)
                            && !path.split('/').any(|part| part == "." || part == "..")
                    })
            })
            && notice
                .changed_paths
                .iter()
                .map(|p| p.as_os_str().len().saturating_mul(6) + 8)
                .sum::<usize>()
                <= MAX_WATCH_HINT_BYTES;
        if valid {
            Ok(notice)
        } else {
            Err(self.retire(io::Error::new(
                io::ErrorKind::InvalidData,
                "watch change exceeded its bounds or authority",
            )))
        }
    }

    /// Schedules Workspace Python preparation on the admitted Client.
    ///
    /// # Errors
    ///
    /// Returns a capability, dispatch, or typed physical refusal.
    pub fn prepare_workspace_python(
        &mut self,
        deadline: Instant,
        admit: impl FnOnce() -> io::Result<()>,
    ) -> io::Result<()> {
        if !self.capabilities.contains(&Capability::WorkspacePython) {
            return Err(io::ErrorKind::Unsupported.into());
        }
        match self.exchange_admitted(
            &ClientRequest::PrepareWorkspacePython,
            Some(deadline),
            admit,
            || (),
        )? {
            ClientReply::WorkspacePythonScheduled => Ok(()),
            ClientReply::Error { kind } => Err(read_error(kind)),
            _ => Err(self.retire(io::ErrorKind::InvalidData.into())),
        }
    }

    /// Reads current readiness independently of the Session admission.
    ///
    /// # Errors
    ///
    /// Returns an unsupported capability or failed exchange.
    pub fn workspace_python_status(
        &mut self,
    ) -> io::Result<rustx_execution_protocol::WorkspacePythonStatus> {
        if !self.capabilities.contains(&Capability::WorkspacePython) {
            return Err(io::ErrorKind::Unsupported.into());
        }
        match self.exchange(&ClientRequest::WorkspacePythonStatus)? {
            ClientReply::WorkspacePythonStatus(status) => Ok(status),
            ClientReply::Error { kind } => Err(read_error(kind)),
            _ => Err(self.retire(io::ErrorKind::InvalidData.into())),
        }
    }

    /// Dispatch one bounded Workspace observation command on this binding.
    ///
    /// # Errors
    ///
    /// Returns a typed transport or admission refusal. A malformed reply
    /// retires the pair before its contents reach the caller.
    pub fn workspace_changes(
        &mut self,
        request: rustx_execution_protocol::changes::ChangeRequest,
        deadline: Instant,
        permit: impl FnOnce() -> io::Result<()>,
    ) -> io::Result<rustx_execution_protocol::changes::ChangeReply> {
        use rustx_execution_protocol::changes::{
            ChangeReply, ChangeRequest, MAX_CHANGE_BYTES, MAX_CHANGE_FILES,
        };

        if !self.capabilities.contains(&Capability::WorkspaceChanges) {
            return Err(io::ErrorKind::Unsupported.into());
        }
        if request.key().incarnation != self.admission.server_incarnation
            || request.key().session != self.admission.session
            || request.key().instance != self.admission.instance
            || request.key().generation != self.admission.generation
        {
            return Err(io::ErrorKind::PermissionDenied.into());
        }
        let request = ClientRequest::WorkspaceChanges(request);
        let reply = self.exchange_admitted(&request, Some(deadline), permit, || ())?;
        let ClientRequest::WorkspaceChanges(request) = &request else {
            unreachable!();
        };
        let ClientReply::WorkspaceChanges(reply) = reply else {
            return Err(self.retire(io::ErrorKind::InvalidData.into()));
        };
        let expected = match (request, &reply) {
            (_, ChangeReply::Unavailable)
            | (
                ChangeRequest::Begin { .. }
                | ChangeRequest::Capture { .. }
                | ChangeRequest::Finish { .. }
                | ChangeRequest::Compare { .. }
                | ChangeRequest::Release { .. },
                ChangeReply::Accepted,
            )
            | (ChangeRequest::Baseline { .. }, ChangeReply::BaselineReady | ChangeReply::Pending)
            | (ChangeRequest::Summary { .. }, ChangeReply::Summary(None))
            | (
                ChangeRequest::Summary { .. } | ChangeRequest::Comparison { .. },
                ChangeReply::Pending,
            ) => true,
            (ChangeRequest::Summary { .. }, ChangeReply::Summary(Some(summary))) => {
                summary.files.len() <= MAX_CHANGE_FILES && summary.total >= summary.files.len()
            }
            (
                ChangeRequest::Comparison { index, .. },
                ChangeReply::Comparison { index: echoed, .. },
            ) => index == echoed,
            _ => false,
        };
        if !expected
            || !serde_json::to_vec(&reply).is_ok_and(|encoded| encoded.len() <= MAX_CHANGE_BYTES)
        {
            return Err(self.retire(io::ErrorKind::InvalidData.into()));
        }
        Ok(reply)
    }

    /// The metadata query: `relative_path` names one entry under the
    /// binding workspace, stat'ed without following a final symlink.
    ///
    /// The peer must have advertised [`Capability::DirectoryMetadata`] at
    /// admission or the call is [`io::ErrorKind::Unsupported`] and no
    /// request is sent. The reply's stamp is trusted only after the shared
    /// `stamp_violation` validation; a malformed reply retires the
    /// stream.
    ///
    /// # Errors
    ///
    /// Returns [`io::ErrorKind::Unsupported`] when the peer lacks the
    /// capability, the [`ReadError`] the Client reported mapped onto an
    /// I/O error kind, or [`io::ErrorKind::InvalidData`] for a malformed
    /// reply.
    pub fn entry_metadata(&mut self, relative_path: &Path) -> io::Result<EntryStamp> {
        self.entry_metadata_until(relative_path, Instant::now() + PAGE_REQUEST_TIMEOUT)
    }

    /// Query metadata under the caller's absolute deadline, which may also
    /// cover related bounded reads from this connection.
    ///
    /// # Errors
    /// Returns the same errors as [`Self::entry_metadata`], and retires the
    /// connection if a sent request exceeds the deadline.
    pub fn entry_metadata_until(
        &mut self,
        relative_path: &Path,
        deadline: Instant,
    ) -> io::Result<EntryStamp> {
        if !self.capabilities.contains(&Capability::DirectoryMetadata) {
            return Err(io::Error::new(
                io::ErrorKind::Unsupported,
                "peer admitted no directory metadata capability",
            ));
        }
        let reply = self.exchange_admitted(
            &ClientRequest::GetMetadata {
                relative_path: relative_path.to_path_buf(),
            },
            Some(deadline),
            || Ok(()),
            || (),
        )?;
        if let ClientReply::Error { kind } = reply {
            return Err(read_error(kind));
        }
        let violation = |message: &'static str| io::Error::new(io::ErrorKind::InvalidData, message);
        let ClientReply::Metadata { metadata } = reply else {
            return Err(self.retire(violation("unexpected reply to metadata query")));
        };
        if let Some(reason) = stamp_violation(&metadata.stamp) {
            return Err(self.retire(violation(reason)));
        }
        Ok(metadata)
    }

    /// Resolves a Workspace resource within the admitted Client directory.
    ///
    /// # Errors
    /// Returns the Client's typed read error or retires the connection on an invalid reply.
    pub fn resolve_resource_entry_until(
        &mut self,
        relative_path: &Path,
        deadline: Instant,
    ) -> io::Result<ResourceResolution> {
        if !self.capabilities.contains(&Capability::DirectoryMetadata) {
            return Err(io::Error::new(
                io::ErrorKind::Unsupported,
                "peer admitted no directory metadata capability",
            ));
        }
        let valid_path = |path: &Path| {
            path.as_os_str().len() <= rustx_execution_protocol::MAX_RESOURCE_PATH_BYTES
                && path.components().all(|component| {
                    matches!(component, std::path::Component::Normal(name)
                            if !name.as_encoded_bytes().contains(&0))
                })
        };
        let valid_dependency = |path: &Path| !path.as_os_str().is_empty() && valid_path(path);
        let valid_links = |paths: &[PathBuf]| {
            paths.len() <= rustx_execution_protocol::MAX_RESOURCE_LINK_HOPS
                && paths.iter().all(|path| valid_dependency(path))
                && paths
                    .first()
                    .is_none_or(|first| relative_path.starts_with(first))
        };
        match self.exchange_admitted(
            &ClientRequest::ResolveResourceEntry {
                relative_path: relative_path.to_path_buf(),
            },
            Some(deadline),
            || Ok(()),
            || (),
        )? {
            ClientReply::Error { kind } => Err(read_error(kind)),
            ClientReply::ResourceEntry {
                relative_path: resolved,
                metadata,
                link_paths,
            } if valid_path(&resolved)
                && metadata.kind != rustx_execution_protocol::EntryKind::Symlink
                && valid_links(&link_paths)
                && (!link_paths.is_empty() || resolved == relative_path) =>
            {
                if let Some(reason) = stamp_violation(&metadata.stamp) {
                    return Err(self.retire(io::Error::new(io::ErrorKind::InvalidData, reason)));
                }
                Ok(ResourceResolution {
                    entry: Some((resolved, metadata)),
                    dependencies: link_paths,
                })
            }
            ClientReply::ResourceUnavailable {
                mut link_paths,
                unresolved_path,
            } if valid_links(&link_paths)
                && unresolved_path.as_deref().is_none_or(|path| {
                    valid_dependency(path)
                        && (!link_paths.is_empty() || relative_path.starts_with(path))
                })
                && (!link_paths.is_empty() || unresolved_path.is_some()) =>
            {
                link_paths.extend(unresolved_path);
                Ok(ResourceResolution {
                    entry: None,
                    dependencies: link_paths,
                })
            }
            _ => Err(self.retire(io::Error::new(
                io::ErrorKind::InvalidData,
                "invalid resource entry reply",
            ))),
        }
    }

    /// One bounded, revision-checked directory page under the binding
    /// workspace.
    ///
    /// The peer must have advertised [`Capability::DirectoryMetadata`] at
    /// admission. `limit` must be within `1..=`[`MAX_DIRECTORY_PAGE_ENTRIES`]
    /// and `offset` within [`MAX_DIRECTORY_NAMES`] — bounds are enforced
    /// before any request is sent. The reply is trusted only after
    /// validation: at most `limit` entries, each name a single component
    /// of at most 255 bytes — never `/`, `.`, or `..` — in strictly
    /// increasing order, every stamp passing `stamp_violation`, the
    /// directory revision well-formed, and a `next_offset` cursor that
    /// equals exactly `offset + entries.len()`, may only continue a
    /// page that filled the requested `limit`, and must stay strictly
    /// inside [`MAX_DIRECTORY_NAMES`] — delivered names may end exactly
    /// at the cap, but a continuation there could never exist. A
    /// violation retires the
    /// stream — it is never reused with reply accounting that cannot be
    /// trusted.
    ///
    /// # Errors
    ///
    /// Returns [`io::ErrorKind::Unsupported`] when the peer lacks the
    /// capability, [`io::ErrorKind::InvalidInput`] for out-of-range paging
    /// arguments, the [`ReadError`] the Client reported mapped onto an
    /// I/O error kind, or [`io::ErrorKind::InvalidData`] for a malformed
    /// or untruthful reply.
    pub fn directory_page(
        &mut self,
        relative_path: &Path,
        offset: u32,
        limit: u32,
        expected_revision: Option<&str>,
    ) -> io::Result<DirectoryListing> {
        self.directory_page_until(
            relative_path,
            offset,
            limit,
            expected_revision,
            Instant::now() + PAGE_REQUEST_TIMEOUT,
        )
    }

    /// Reads one directory page using the absolute deadline of the source capture.
    ///
    /// # Errors
    /// Returns the same errors as [`Self::directory_page`], including transport timeout.
    pub fn directory_page_until(
        &mut self,
        relative_path: &Path,
        offset: u32,
        limit: u32,
        expected_revision: Option<&str>,
        deadline: Instant,
    ) -> io::Result<DirectoryListing> {
        self.scan_directory(
            relative_path,
            offset,
            limit,
            expected_revision,
            deadline,
            false,
        )
    }

    /// Reads a discovery page, withholding hidden names and counting them toward the limit.
    ///
    /// # Errors
    /// Returns the same errors as [`Self::directory_page_until`] for visible names.
    pub fn skill_directory_page_until(
        &mut self,
        relative_path: &Path,
        offset: u32,
        limit: u32,
        expected_revision: Option<&str>,
        deadline: Instant,
    ) -> io::Result<DirectoryListing> {
        self.scan_directory(
            relative_path,
            offset,
            limit,
            expected_revision,
            deadline,
            true,
        )
    }

    fn scan_directory(
        &mut self,
        relative_path: &Path,
        offset: u32,
        limit: u32,
        expected_revision: Option<&str>,
        deadline: Instant,
        skip_hidden: bool,
    ) -> io::Result<DirectoryListing> {
        if !self.capabilities.contains(&Capability::DirectoryMetadata) {
            return Err(io::Error::new(
                io::ErrorKind::Unsupported,
                "peer admitted no directory metadata capability",
            ));
        }
        let name_cap = u32::try_from(MAX_DIRECTORY_NAMES).unwrap_or(u32::MAX);
        if !(1..=u32::try_from(MAX_DIRECTORY_PAGE_ENTRIES).unwrap_or(u32::MAX)).contains(&limit)
            || offset > name_cap
        {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "invalid directory page request",
            ));
        }
        let reply = self.exchange_admitted(
            &ClientRequest::ReadDirectory {
                relative_path: relative_path.to_path_buf(),
                offset,
                limit,
                expected_revision: expected_revision.map(str::to_owned),
                skip_hidden,
            },
            Some(deadline),
            || Ok(()),
            || (),
        )?;
        if let ClientReply::Error { kind } = reply {
            return Err(read_error(kind));
        }
        self.validate_directory(offset, limit, expected_revision, skip_hidden, &reply)
    }

    /// Verifies a `Directory` reply against the request before trusting
    /// it — names, order, cursor, and every stamp. Any deviation means
    /// the peer cannot account for its answers honestly, so the
    /// connection is retired.
    fn validate_directory(
        &mut self,
        offset: u32,
        limit: u32,
        expected_revision: Option<&str>,
        skip_hidden: bool,
        reply: &ClientReply,
    ) -> io::Result<DirectoryListing> {
        let violation = |message: &'static str| io::Error::new(io::ErrorKind::InvalidData, message);
        let ClientReply::Directory {
            revision,
            entries,
            next_offset,
            skipped,
        } = reply
        else {
            return Err(self.retire(violation("unexpected reply to directory read")));
        };
        if !valid_revision(revision) {
            return Err(self.retire(violation("directory reply carries an invalid revision")));
        }
        let names = u32::try_from(entries.len()).unwrap_or(u32::MAX);
        let delivered = names.saturating_add(*skipped);
        if delivered > limit || (!skip_hidden && *skipped != 0) {
            return Err(self.retire(violation("directory reply exceeds requested page")));
        }
        let mut previous: Option<&str> = None;
        for entry in entries {
            let name = entry.name.as_str();
            // One normal component, verbatim: no `/`, never `.` or `..`,
            // no longer than a native name, never NUL — the one byte no
            // path API could ever carry. Every other byte — control
            // characters included — is ordinary literal name data the
            // Server never joins into a path it opens.
            if name.len() > 255
                || name.is_empty()
                || name.contains('/')
                || name.contains('\0')
                || name == "."
                || name == ".."
                || (skip_hidden && (name.starts_with('.') || name == "node_modules"))
            {
                return Err(self.retire(violation("directory reply carries an invalid entry name")));
            }
            if previous.is_some_and(|previous| previous >= name) {
                return Err(self.retire(violation("directory reply names are out of order")));
            }
            previous = Some(name);
            if let Some(reason) = stamp_violation(&entry.metadata.stamp) {
                return Err(self.retire(violation(reason)));
            }
        }
        let Some(cursor) = offset.checked_add(delivered) else {
            return Err(self.retire(violation("directory reply overflows the name space")));
        };
        // The delivered names must stay inside the physical listing
        // bound even on a final page: a reply claiming entries past the
        // name cap is corrupt regardless of the cursor it carries.
        let name_cap = u32::try_from(MAX_DIRECTORY_NAMES).unwrap_or(u32::MAX);
        if cursor > name_cap {
            return Err(self.retire(violation("directory reply exceeds the name cap")));
        }
        // Continuation is legal only after a page that filled the
        // requested limit, and the cursor must land exactly past the
        // delivered entries — strictly inside the name cap, since a
        // resumption at the cap could never deliver another entry.
        match next_offset {
            Some(next) if delivered == limit && *next == cursor && cursor < name_cap => {}
            Some(_) => {
                return Err(self.retire(violation("directory reply cursor is inconsistent")));
            }
            None => {}
        }
        if let Some(expected) = expected_revision
            && revision != expected
        {
            return Err(self.retire(violation("directory reply revision ignores expectation")));
        }
        Ok(DirectoryListing {
            revision: revision.clone(),
            entries: entries.clone(),
            next_offset: *next_offset,
            skipped: *skipped,
        })
    }

    /// One bounded logical-text page of a workspace-relative or absolute
    /// Client path — the complete-line page contract the Read tool projects,
    /// scanned physically by the Client.
    ///
    /// The peer must have advertised [`Capability::ReadLines`] at
    /// admission or the call is [`io::ErrorKind::Unsupported`] and no
    /// request is sent. `offset` is 1-based and `limit`, when given, must
    /// be nonzero — both are enforced before anything crosses the wire.
    /// The reply is trusted only after validation: the stamp must pass
    /// `stamp_violation` and a supplied `expected_revision` must match,
    /// `total_lines` must be nonzero, `text` must stay within
    /// [`MAX_TEXT_BYTES`] and its `'\n'`-split must reproduce exactly
    /// `shown_lines` elements (never more than [`MAX_TEXT_LINES`]` + 1`),
    /// and `end` must agree with the request and the page's own numbers —
    /// `Complete` reaches the source end exactly, `RequestedLimit` shows
    /// precisely the requested bound with lines remaining, `LineLimit`
    /// and `ByteLimit` leave at least one more element, `FirstLineTooLarge`
    /// reports a first line beyond [`MAX_TEXT_BYTES`], and `OffsetPastEnd`
    /// addresses beyond the source. The page's accounting must also fit
    /// the stamped source size. Any deviation retires the stream — it is
    /// never reused with reply accounting that cannot be trusted.
    ///
    /// # Errors
    ///
    /// Returns [`io::ErrorKind::Unsupported`] when the peer lacks the
    /// capability, [`io::ErrorKind::InvalidInput`] for a zero offset or
    /// limit, the [`ReadError`] the Client reported mapped onto an I/O
    /// error kind — [`ReadError::InvalidUtf8`] included, a semantic
    /// refusal that never retires a healthy connection — or
    /// [`io::ErrorKind::InvalidData`] for a malformed or untruthful
    /// reply.
    pub fn read_lines(
        &mut self,
        relative_path: &Path,
        offset: u64,
        limit: Option<u64>,
        expected_revision: Option<&str>,
    ) -> io::Result<LinesPage> {
        self.read_lines_admitted(
            relative_path,
            offset,
            limit,
            expected_revision,
            Instant::now() + PAGE_REQUEST_TIMEOUT,
            || Ok(()),
            || (),
        )
    }

    /// `read_lines` against a caller-armed absolute deadline and
    /// acceptance gate: `deadline` bounds the request write and the
    /// reply read on the wire as one budget, `admit` runs after the
    /// request is encoded and the binding's live check passes but before
    /// the first request byte — its error returns verbatim and never
    /// retires the connection — and `sent` runs once the full request
    /// frame is written, before the reply is read.
    ///
    /// # Errors
    ///
    /// Returns the same kinds as [`read_lines`](Self::read_lines), plus
    /// the `admit` error verbatim and [`io::ErrorKind::TimedOut`] when
    /// the deadline lapses mid-exchange — which retires the connection.
    #[allow(clippy::too_many_arguments)] // one admitted exchange's full context
    pub fn read_lines_admitted(
        &mut self,
        relative_path: &Path,
        offset: u64,
        limit: Option<u64>,
        expected_revision: Option<&str>,
        deadline: Instant,
        admit: impl FnOnce() -> io::Result<()>,
        sent: impl FnOnce(),
    ) -> io::Result<LinesPage> {
        if !self.capabilities.contains(&Capability::ReadLines) {
            return Err(io::Error::new(
                io::ErrorKind::Unsupported,
                "peer admitted no lines capability",
            ));
        }
        if offset == 0 || limit == Some(0) {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "invalid text page request",
            ));
        }
        let reply = self.exchange_admitted(
            &ClientRequest::ReadLines {
                relative_path: relative_path.to_path_buf(),
                offset: DecimalU64::new(offset),
                limit: limit.map(DecimalU64::new),
                expected_revision: expected_revision.map(str::to_owned),
            },
            Some(deadline),
            admit,
            sent,
        )?;
        if let ClientReply::Error { kind } = reply {
            return Err(read_error(kind));
        }
        self.validate_lines(offset, limit, expected_revision, &reply)
    }

    /// Verifies a `Lines` reply against the request and the scanner's
    /// complete-line contract before trusting it. Any deviation means
    /// the peer cannot account for its answers honestly, so the
    /// connection is retired.
    fn validate_lines(
        &mut self,
        offset: u64,
        limit: Option<u64>,
        expected_revision: Option<&str>,
        reply: &ClientReply,
    ) -> io::Result<LinesPage> {
        let violation = |message: &'static str| io::Error::new(io::ErrorKind::InvalidData, message);
        let ClientReply::Lines {
            metadata,
            text,
            total_lines,
            shown_lines,
            end,
        } = reply
        else {
            return Err(self.retire(violation("unexpected reply to lines read")));
        };
        if let Some(reason) = stamp_violation(metadata) {
            return Err(self.retire(violation(reason)));
        }
        if let Some(expected) = expected_revision
            && metadata.revision != expected
        {
            return Err(self.retire(violation("lines reply revision ignores expectation")));
        }
        let size = metadata.size.get();
        if let Some(reason) = page_accounting_violation(
            size,
            text,
            total_lines.get(),
            *shown_lines,
            end,
            offset,
            limit,
        ) {
            return Err(self.retire(violation(reason)));
        }
        Ok(LinesPage {
            text: text.clone(),
            total_lines: total_lines.get(),
            shown_lines: *shown_lines,
            end: *end,
            metadata: metadata.clone(),
        })
    }

    /// One bounded logical-text page of a whitelisted structured document
    /// at a workspace-relative or absolute Client path. The Client decodes it with
    /// the shared rustX parser policy and serves the complete-line
    /// projection of the decoded Markdown.
    ///
    /// The peer must have advertised [`Capability::DocumentPages`] at
    /// admission or the call is [`io::ErrorKind::Unsupported`] and no
    /// request is sent. `offset` is 1-based and `limit`, when given, must
    /// be nonzero — both are enforced before anything crosses the wire.
    /// The reply is trusted only after validation: `source` must pass
    /// `stamp_violation`, stay within [`MAX_DOCUMENT_SOURCE_BYTES`], and a
    /// supplied `expected_revision` must match; `logical_bytes` — the
    /// decoded Markdown length, independent of the compressed source's
    /// size — must stay within [`MAX_DOCUMENT_LOGICAL_BYTES`]; and the
    /// page must pass the shared complete-line accounting every `Lines`
    /// reply answers to. A violation retires the stream — it is never
    /// reused with reply accounting that cannot be trusted. A typed
    /// semantic refusal — an unsupported extension, an oversized source,
    /// a parser rejection, or a missing text layer — is a distinct
    /// [`ReadError`] that never retires a healthy connection.
    ///
    /// # Errors
    ///
    /// Returns [`io::ErrorKind::Unsupported`] when the peer lacks the
    /// document capability, [`io::ErrorKind::InvalidInput`] for a zero
    /// offset or limit, the [`ReadError`] the Client reported mapped onto
    /// an I/O error kind, or [`io::ErrorKind::InvalidData`] for a
    /// malformed or untruthful reply.
    pub fn read_document(
        &mut self,
        relative_path: &Path,
        offset: u64,
        limit: Option<u64>,
        expected_revision: Option<&str>,
    ) -> io::Result<DocumentPage> {
        self.read_document_admitted(
            relative_path,
            offset,
            limit,
            expected_revision,
            Instant::now() + PAGE_REQUEST_TIMEOUT,
            || Ok(()),
            || (),
        )
    }

    /// `read_document` against a caller-armed absolute deadline and
    /// acceptance gate, under the same contract as
    /// [`read_lines_admitted`](Self::read_lines_admitted): one wire
    /// budget, an admission refusal that returns before any request byte
    /// without retiring, and a `sent` observation between the written
    /// frame and the reply.
    ///
    /// # Errors
    ///
    /// Returns the same kinds as [`read_document`](Self::read_document),
    /// plus the `admit` error verbatim and [`io::ErrorKind::TimedOut`]
    /// when the deadline lapses mid-exchange — which retires the
    /// connection.
    #[allow(clippy::too_many_arguments)] // one admitted exchange's full context
    pub fn read_document_admitted(
        &mut self,
        relative_path: &Path,
        offset: u64,
        limit: Option<u64>,
        expected_revision: Option<&str>,
        deadline: Instant,
        admit: impl FnOnce() -> io::Result<()>,
        sent: impl FnOnce(),
    ) -> io::Result<DocumentPage> {
        if !self.capabilities.contains(&Capability::DocumentPages) {
            return Err(io::Error::new(
                io::ErrorKind::Unsupported,
                "peer admitted no document capability",
            ));
        }
        if offset == 0 || limit == Some(0) {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "invalid document page request",
            ));
        }
        let reply = self.exchange_admitted(
            &ClientRequest::ReadDocument {
                relative_path: relative_path.to_path_buf(),
                offset: DecimalU64::new(offset),
                limit: limit.map(DecimalU64::new),
                expected_revision: expected_revision.map(str::to_owned),
            },
            Some(deadline),
            admit,
            sent,
        )?;
        if let ClientReply::Error { kind } = reply {
            return Err(read_error(kind));
        }
        self.validate_document(offset, limit, expected_revision, &reply)
    }

    /// Verifies a `DocumentPage` reply against the request, the source
    /// and logical bounds, and the shared complete-line contract before
    /// trusting it. Any deviation means the peer cannot account for its
    /// answers honestly, so the connection is retired.
    fn validate_document(
        &mut self,
        offset: u64,
        limit: Option<u64>,
        expected_revision: Option<&str>,
        reply: &ClientReply,
    ) -> io::Result<DocumentPage> {
        let violation = |message: &'static str| io::Error::new(io::ErrorKind::InvalidData, message);
        let ClientReply::DocumentPage {
            source,
            logical_bytes,
            text,
            total_lines,
            shown_lines,
            end,
        } = reply
        else {
            return Err(self.retire(violation("unexpected reply to document read")));
        };
        if let Some(reason) = stamp_violation(source) {
            return Err(self.retire(violation(reason)));
        }
        if let Some(expected) = expected_revision
            && source.revision != expected
        {
            return Err(self.retire(violation("document reply revision ignores expectation")));
        }
        // The reply's own bounds are wire policy: a source the Client
        // could never have decoded and a projection larger than the
        // decoder's own cap are both untruthful on their face.
        let logical = logical_bytes.get();
        if source.size.get() > MAX_DOCUMENT_SOURCE_BYTES as u64
            || logical > MAX_DOCUMENT_LOGICAL_BYTES
        {
            return Err(self.retire(violation("document reply exceeds the document bounds")));
        }
        if let Some(reason) = page_accounting_violation(
            logical,
            text,
            total_lines.get(),
            *shown_lines,
            end,
            offset,
            limit,
        ) {
            return Err(self.retire(violation(reason)));
        }
        Ok(DocumentPage {
            source: source.clone(),
            logical_bytes: logical,
            text: text.clone(),
            total_lines: total_lines.get(),
            shown_lines: *shown_lines,
            end: *end,
        })
    }

    /// One bounded Client search: the file-universe traversal is rooted at a
    /// workspace-relative or absolute Client path. An empty path names the
    /// workspace root; `spec` narrows it to one selected-records page.
    ///
    /// The peer must have advertised [`Capability::BoundedSearch`] at
    /// admission or the call is [`io::ErrorKind::Unsupported`] and no
    /// request is sent. The reply is trusted only after validation: the
    /// body must fit [`MAX_TEXT_BYTES`], the match count must stay within
    /// the requested bound and agree with the body's emptiness, a glob
    /// page can never claim shortened lines, and `hit_limit` is a proof —
    /// it requires `matches` to equal the request's bound exactly. A
    /// violation retires the stream — it is never reused with a reply
    /// accounting that cannot be trusted. Display bytes — record text
    /// that may contain any byte a Unix name carries — are never parsed
    /// back into accounting.
    ///
    /// # Errors
    ///
    /// Returns [`io::ErrorKind::Unsupported`] when the peer lacks the
    /// search capability, [`io::ErrorKind::InvalidInput`] for an
    /// out-of-range spec, the [`ReadError`] the Client reported mapped
    /// onto an I/O error kind, or [`io::ErrorKind::InvalidData`] for a
    /// malformed or untruthful reply.
    pub fn search(&mut self, relative_root: &Path, spec: &SearchSpec) -> io::Result<SearchResult> {
        self.search_admitted(
            relative_root,
            spec,
            Instant::now() + PAGE_REQUEST_TIMEOUT,
            || Ok(()),
            || (),
        )
    }

    /// `search` against a caller-armed absolute deadline and acceptance
    /// gate, under the same contract as
    /// [`read_lines_admitted`](Self::read_lines_admitted): one wire
    /// budget, an admission refusal that returns before any request byte
    /// without retiring, and a `sent` observation between the written
    /// frame and the reply.
    ///
    /// # Errors
    ///
    /// Returns the same kinds as [`search`](Self::search), plus the
    /// `admit` error verbatim and [`io::ErrorKind::TimedOut`] when the
    /// deadline lapses mid-exchange — which retires the connection.
    pub fn search_admitted(
        &mut self,
        relative_root: &Path,
        spec: &SearchSpec,
        deadline: Instant,
        admit: impl FnOnce() -> io::Result<()>,
        sent: impl FnOnce(),
    ) -> io::Result<SearchResult> {
        if !self.capabilities.contains(&Capability::BoundedSearch) {
            return Err(io::Error::new(
                io::ErrorKind::Unsupported,
                "peer admitted no search capability",
            ));
        }
        let limit = match spec {
            SearchSpec::Glob { pattern, limit } => {
                if pattern.len() > MAX_SEARCH_PATTERN_BYTES || limit.get() == 0 {
                    return Err(io::Error::new(
                        io::ErrorKind::InvalidInput,
                        "invalid search request",
                    ));
                }
                *limit
            }
            SearchSpec::Grep {
                pattern,
                glob,
                context,
                limit,
                ..
            } => {
                if pattern.len() > MAX_SEARCH_PATTERN_BYTES
                    || glob
                        .as_ref()
                        .is_some_and(|glob| glob.len() > MAX_SEARCH_PATTERN_BYTES)
                    || *context > MAX_SEARCH_CONTEXT_LINES
                    || limit.get() == 0
                {
                    return Err(io::Error::new(
                        io::ErrorKind::InvalidInput,
                        "invalid search request",
                    ));
                }
                *limit
            }
        };
        let reply = self.exchange_admitted(
            &ClientRequest::Search {
                relative_root: relative_root.to_path_buf(),
                spec: spec.clone(),
            },
            Some(deadline),
            admit,
            sent,
        )?;
        if let ClientReply::Error { kind } = reply {
            return Err(read_error(kind));
        }
        let context = match spec {
            SearchSpec::Glob { .. } => 0,
            SearchSpec::Grep { context, .. } => *context,
        };
        self.validate_search(
            matches!(spec, SearchSpec::Glob { .. }),
            context,
            limit,
            &reply,
        )
    }

    /// Verifies a `SearchPage` reply against the request's bound before
    /// trusting it. Any deviation means the peer cannot account for its
    /// answers honestly, so the connection is retired.
    ///
    /// The one honest zero-match body: a Grep with before-context can
    /// fill the 50 KiB projection on context rows alone — the matching
    /// line's own record never fits — so `matches == 0` may pair with a
    /// nonempty body only when `context > 0` *and* `byte_limit` explains
    /// the cut; an empty body under `byte_limit` is the same overflow
    /// landing before the first record. Glob has no context rows: a
    /// nonempty body without a match is impossible there. Every other
    /// combination — an over-cap body, a count past the requested limit
    /// or past the body's own byte length, `short_lines` on Glob,
    /// `hit_limit` without a proven full limit, or the mutually
    /// exclusive `hit_limit && byte_limit` — retires the connection.
    fn validate_search(
        &mut self,
        glob: bool,
        context: u32,
        limit: DecimalU64,
        reply: &ClientReply,
    ) -> io::Result<SearchResult> {
        let violation = |message: &'static str| io::Error::new(io::ErrorKind::InvalidData, message);
        let ClientReply::SearchPage {
            body,
            matches,
            hit_limit,
            byte_limit,
            short_lines,
        } = reply
        else {
            return Err(self.retire(violation("unexpected reply to search")));
        };
        let matches = matches.get();
        let zero_match_context_overflow = !glob && context > 0 && *byte_limit;
        if body.len() > MAX_TEXT_BYTES
            || matches > limit.get()
            || matches > body.len() as u64
            || (matches > 0 && body.is_empty())
            || (!body.is_empty() && matches == 0 && !zero_match_context_overflow)
            || (glob && *short_lines)
            || (*hit_limit && matches != limit.get())
            || (*hit_limit && *byte_limit)
        {
            return Err(self.retire(violation("search reply accounting is impossible")));
        }
        Ok(SearchResult {
            body: body.clone(),
            matches,
            hit_limit: *hit_limit,
            byte_limit: *byte_limit,
            short_lines: *short_lines,
        })
    }

    /// The conditional write: the workspace-relative or absolute Client path
    /// becomes exactly `bytes` when `expected_revision` still names the
    /// current file — or the target did not exist for `None`.
    ///
    /// The peer must have advertised [`Capability::ConditionalWrite`] at
    /// admission or the call is [`io::ErrorKind::Unsupported`] and no
    /// request is sent; `bytes` is bounded by [`MAX_RAW_CHUNK_BYTES`]
    /// before anything crosses the wire. The write is two phases on this
    /// same connection: one `PrepareWrite` retains the target and stages
    /// the payload privately, then — after `authorize` has re-checked its
    /// caller's gate against the prepared original — one `CommitWrite`
    /// publishes it, or one `AbortWrite` removes the stage when the gate
    /// denies and still admits the frame.
    ///
    /// The replies are verified, never trusted: the ticket must be a
    /// nonzero scalar the same connection commits or aborts under, and
    /// the prepared `original` must satisfy exactly the expectation that
    /// was sent — `Some` stamp with the expected revision for a replace,
    /// `None` for a create-only target. A violation retires the stream.
    ///
    /// The phases have different failure contracts. Before the commit
    /// request's first byte, every failure is a clean pre-commit error:
    /// the staged object cannot have published. Once that first byte is
    /// written the publish crossing may already have happened, so a
    /// transport error, a malformed reply, or a mismatched ticket retires
    /// the connection and returns a [`WriteOutcomeUnknown`]-downcastable
    /// error instead of a denial — an authoritative re-read is required,
    /// never a retry. An explicit typed [`ClientReply::Error`] still
    /// propagates as a refused write, and an explicit
    /// [`ClientReply::CommitUnknown`] with the correct ticket reports the
    /// same unknowable outcome.
    ///
    /// `authorize` runs on the verified prepared original before the
    /// commit is dispatched — the caller's last gate, invoked with
    /// `Some(&original)` for a replace or `None` for a create-only
    /// target. A denial sends `AbortWrite` while the gate still admits
    /// it and never commits.
    ///
    /// # Errors
    ///
    /// Returns [`io::ErrorKind::Unsupported`] when the peer lacks the
    /// write capability, [`io::ErrorKind::InvalidInput`] for an oversized
    /// payload, the [`ReadError`] the Client reported mapped onto an I/O
    /// The conditional write through the admitted Client: `path` resolves
    /// once against the reserved workspace at dispatch, then the whole
    /// prepare–authorize–commit sequence runs on that same connection.
    ///
    /// `path` names the exact file leaf — never the workspace root. The
    /// relative path the gate produces is sent to the Client; it is never
    /// re-parsed or re-opened by the Server after approval — the physical
    /// target the Client retains under its own workspace is the only one
    /// that exists. `authorize` is invoked by the connection with the
    /// prepared original — `Some(&stamp)` for a replace, `None` for a
    /// create-only target — as the caller's last gate before the commit
    /// frame is written. `expected_revision` maps onto
    /// [`WriteExpectation`]: `Some` requires that exact revision,
    /// `None` is create-only.
    ///
    /// # Errors
    ///
    /// Returns [`io::ErrorKind::Unsupported`] without the Client's write
    /// capability, [`io::ErrorKind::InvalidInput`] for an over-cap
    /// payload, the `authorize` error verbatim when its gate denies, a
    /// `rustx_execution_server::WriteOutcomeUnknown`-downcastable error
    /// once the commit outcome cannot be known, and the typed exchange's
    /// own errors.
    pub fn write_file(
        &mut self,
        relative_path: &Path,
        bytes: &[u8],
        expected_revision: Option<&str>,
        authorize: impl FnOnce(Option<&FileStamp>) -> io::Result<()>,
    ) -> io::Result<FileStamp> {
        // One absolute deadline covers prepare, the authorization abort,
        // and commit — it is armed once and never rearmed between phases.
        let deadline = Instant::now() + PAGE_REQUEST_TIMEOUT;
        let expectation = match expected_revision {
            Some(revision) => WriteExpectation::Revision(revision.to_owned()),
            None => WriteExpectation::Absent,
        };
        let prepared = self.prepare_write(
            relative_path,
            &expectation,
            bytes,
            Some(deadline),
            &|| Ok(()),
            &|| (),
        )?;
        if let Err(error) = authorize(prepared.original.as_ref()) {
            // The gate denied before commit: remove the preparation while
            // the binding still admits the frame — under the same
            // absolute deadline — and never commit. The denial is still
            // what the caller sees.
            if self.gate().is_ok() {
                let _ = self.abort_prepared(&prepared, Some(deadline), &|| Ok(()), &|| ());
            }
            return Err(error);
        }
        self.commit_prepared(&prepared, Some(deadline), &|| Ok(()), &|| ())
    }

    /// The prepare half of a conditional write: validates the request,
    /// dispatches `PrepareWrite` — or `BeginWrite` plus `WriteChunk`
    /// exchanges for a payload over [`MAX_RAW_CHUNK_BYTES`] — and returns
    /// the opaque ticketed [`PreparedMutation`] whose `size` is the
    /// exact byte count the later `Written` receipt must match.
    ///
    /// `expectation` decides what the named leaf must be: `Current`
    /// retains whatever is present — the pre-authorization object an
    /// Agent prepares before its permission gate and never itself a
    /// permission to publish — `Absent` is create-only, and `Revision`
    /// requires the exact current stamp.
    ///
    /// `deadline`, when present, is the caller's already-armed absolute
    /// wire budget — it is never rearmed here.
    ///
    /// # Errors
    ///
    /// Returns [`io::ErrorKind::Unsupported`] without the Client's write
    /// capability, [`io::ErrorKind::InvalidInput`] for an over-cap payload
    /// or malformed expectation, the [`ReadError`] the Client reported
    /// mapped onto an I/O error kind, or
    /// [`io::ErrorKind::InvalidData`] for a malformed or untruthful reply.
    pub fn prepare_write(
        &mut self,
        relative_path: &Path,
        expectation: &WriteExpectation,
        bytes: &[u8],
        deadline: Option<Instant>,
        admit: &impl Fn() -> io::Result<()>,
        sent: &impl Fn(),
    ) -> io::Result<PreparedMutation> {
        if !self.capabilities.contains(&Capability::ConditionalWrite) {
            return Err(io::Error::new(
                io::ErrorKind::Unsupported,
                "peer admitted no conditional write capability",
            ));
        }
        if bytes.len() > MAX_WRITE_BYTES {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "file write exceeds the write cap",
            ));
        }
        if let WriteExpectation::Revision(revision) = expectation
            && !valid_revision(revision)
        {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "expected revision outside the bounded write policy",
            ));
        }
        let reply = if bytes.len() <= MAX_RAW_CHUNK_BYTES {
            self.exchange_admitted(
                &ClientRequest::PrepareWrite {
                    relative_path: relative_path.to_path_buf(),
                    expectation: expectation.clone(),
                    data_base64: base64::engine::general_purpose::STANDARD.encode(bytes),
                },
                deadline,
                admit,
                sent,
            )?
        } else {
            return self.prepare_write_chunked(
                relative_path,
                expectation,
                bytes,
                deadline,
                admit,
                sent,
            );
        };
        if let ClientReply::Error { kind } = reply {
            return Err(read_error(kind));
        }
        self.valid_prepared(reply, expectation, bytes.len() as u64)
    }

    /// The chunked half of [`prepare_write`](Self::prepare_write) for a
    /// payload over [`MAX_RAW_CHUNK_BYTES`]: `BeginWrite` opens the
    /// ticketed preparation, then `WriteChunk` records transfer the
    /// payload at deterministic absolute offsets under the same armed
    /// deadline. A chunk-exchange failure aborts the opened ticket while
    /// the binding still admits the frame; a malformed echo retires.
    fn prepare_write_chunked(
        &mut self,
        relative_path: &Path,
        expectation: &WriteExpectation,
        bytes: &[u8],
        deadline: Option<Instant>,
        admit: &impl Fn() -> io::Result<()>,
        sent: &impl Fn(),
    ) -> io::Result<PreparedMutation> {
        let reply = self.exchange_admitted(
            &ClientRequest::BeginWrite {
                relative_path: relative_path.to_path_buf(),
                expectation: expectation.clone(),
                total: DecimalU64::new(bytes.len() as u64),
            },
            deadline,
            admit,
            sent,
        )?;
        if let ClientReply::Error { kind } = reply {
            return Err(read_error(kind));
        }
        let ClientReply::WritePrepared { ticket, .. } = reply else {
            return Err(self.retire(io::Error::new(
                io::ErrorKind::InvalidData,
                "unexpected reply to write begin",
            )));
        };
        if ticket.get() == 0 {
            return Err(self.retire(io::Error::new(
                io::ErrorKind::InvalidData,
                "write reply carries a zero ticket",
            )));
        }
        for (index, chunk) in bytes.chunks(MAX_RAW_CHUNK_BYTES).enumerate() {
            let offset = (index * MAX_RAW_CHUNK_BYTES) as u64;
            match self.exchange_admitted(
                &ClientRequest::WriteChunk {
                    ticket,
                    offset: DecimalU64::new(offset),
                    data_base64: base64::engine::general_purpose::STANDARD.encode(chunk),
                },
                deadline,
                admit,
                sent,
            ) {
                Err(error) => {
                    if self.gate().is_ok() {
                        let _ = self.abort_prepared(
                            &PreparedMutation {
                                ticket,
                                original: None,
                                size: bytes.len() as u64,
                                connection: self.connection.clone(),
                            },
                            deadline,
                            &|| Ok(()),
                            &|| (),
                        );
                    }
                    return Err(error);
                }
                Ok(ClientReply::WriteChunkReceived {
                    ticket: echoed_ticket,
                    offset: echoed,
                }) if echoed.get() == offset && echoed_ticket == ticket => {}
                Ok(_) => {
                    if self.gate().is_ok() {
                        let _ = self.abort_prepared(
                            &PreparedMutation {
                                ticket,
                                original: None,
                                size: bytes.len() as u64,
                                connection: self.connection.clone(),
                            },
                            deadline,
                            &|| Ok(()),
                            &|| (),
                        );
                    }
                    return Err(self.retire(io::Error::new(
                        io::ErrorKind::InvalidData,
                        "unexpected reply to write chunk",
                    )));
                }
            }
        }
        self.valid_prepared(reply, expectation, bytes.len() as u64)
    }

    /// Validates one `WritePrepared`/`EditPrepared`-shaped reply into the
    /// opaque [`PreparedMutation`]: the ticket must be nonzero and the
    /// reported original must satisfy exactly the expectation that was
    /// sent — a valid stamp matching the revision for `Revision`, any
    /// valid stamp or `None` for `Current`, and `None` alone for
    /// `Absent`.
    fn valid_prepared(
        &mut self,
        reply: ClientReply,
        expectation: &WriteExpectation,
        size: u64,
    ) -> io::Result<PreparedMutation> {
        let (ticket, original) = self.verify_write_prepared(reply, expectation)?;
        Ok(PreparedMutation {
            ticket,
            original,
            size,
            connection: self.connection.clone(),
        })
    }

    pub(super) fn verify_write_prepared(
        &mut self,
        reply: ClientReply,
        expectation: &WriteExpectation,
    ) -> io::Result<(DecimalU64, Option<FileStamp>)> {
        let violation = |message: &'static str| io::Error::new(io::ErrorKind::InvalidData, message);
        let ClientReply::WritePrepared { ticket, original } = reply else {
            return Err(self.retire(violation("unexpected reply to write prepare")));
        };
        if ticket.get() == 0 {
            return Err(self.retire(violation("write reply carries a zero ticket")));
        }
        let holds = match (expectation, &original) {
            (WriteExpectation::Revision(expected), Some(original)) => {
                original.revision == *expected && stamp_violation(original).is_none()
            }
            (WriteExpectation::Current, Some(original)) => stamp_violation(original).is_none(),
            (WriteExpectation::Current | WriteExpectation::Absent, None) => true,
            (WriteExpectation::Revision(_), None) | (WriteExpectation::Absent, Some(_)) => false,
        };
        if !holds {
            return Err(self.retire(violation("write reply ignores the expected revision")));
        }
        Ok((ticket, original))
    }

    /// The commit half of [`prepare_write`](Self::prepare_write) and
    /// [`prepare_edit`](Self::prepare_edit): the caller's absolute
    /// deadline still applies and is never rearmed. `Written` is trusted
    /// only when its stamp is valid and its size equals `prepared.size`.
    ///
    /// # Errors
    ///
    /// Returns a [`WriteOutcomeUnknown`]-downcastable error once the
    /// outcome cannot be known, or the typed exchange's own errors.
    pub fn commit_prepared(
        &mut self,
        prepared: &PreparedMutation,
        deadline: Option<Instant>,
        admit: &impl Fn() -> io::Result<()>,
        sent: &impl Fn(),
    ) -> io::Result<FileStamp> {
        if prepared.connection != self.connection {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "prepared mutation belongs to a different connection",
            ));
        }
        self.commit_write(prepared.ticket, prepared.size, deadline, admit, sent)
    }

    /// The abort half: removes the ticketed preparation while the binding
    /// still admits the frame. A malformed abort reply retires the
    /// connection like any other protocol violation.
    ///
    /// # Errors
    ///
    /// Returns the typed exchange's own errors, or
    /// [`io::ErrorKind::InvalidData`] for a malformed reply.
    pub fn abort_prepared(
        &mut self,
        prepared: &PreparedMutation,
        deadline: Option<Instant>,
        admit: &impl Fn() -> io::Result<()>,
        sent: &impl Fn(),
    ) -> io::Result<()> {
        if prepared.connection != self.connection {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "prepared mutation belongs to a different connection",
            ));
        }
        match self.exchange_admitted(
            &ClientRequest::AbortWrite {
                ticket: prepared.ticket,
            },
            deadline,
            admit,
            sent,
        ) {
            Ok(ClientReply::WriteAborted { ticket: echoed }) if echoed == prepared.ticket => Ok(()),
            Ok(ClientReply::Error { kind }) => Err(read_error(kind)),
            Ok(_) => Err(self.retire(io::Error::new(
                io::ErrorKind::InvalidData,
                "unexpected reply to write abort",
            ))),
            Err(error) => Err(error),
        }
    }

    /// One conditional edit: `PrepareEdit` retains the exact named
    /// original, applies the bounded text-edit algorithm on the Client,
    /// and stages the result privately; `authorize` then gates the
    /// publish against the verified original stamp before the existing
    /// `CommitWrite`/`AbortWrite` machinery commits or abandons it.
    ///
    /// The whole sequence shares one absolute [`PAGE_REQUEST_TIMEOUT`]
    /// deadline armed once — it covers the prepare exchange, the
    /// authorization window, and the commit or abort, and it is never
    /// rearmed. A safe pre-wire refusal — missing capability, an
    /// out-of-policy request, an `EditRefused` semantic failure, or the
    /// `authorize` denial followed by a well-formed abort — never
    /// retires the connection. Once `CommitWrite`'s first byte is on
    /// the wire the publish may have crossed: a transport failure, a
    /// malformed or mismatched receipt, and an explicit
    /// `CommitUnknown` alike retire the connection and surface
    /// [`WriteOutcomeUnknown`], never an ordinary retryable denial.
    ///
    /// # Errors
    ///
    /// Returns [`io::ErrorKind::Unsupported`] when the peer lacks the
    /// edit capability, [`io::ErrorKind::InvalidInput`] for a request
    /// outside the bounded replacement policy, a [`RefusedEdit`]
    /// downcastable error carrying the typed [`EditRefusal`] when the
    /// Client refused semantically, the `authorize` error verbatim on
    /// denial, a [`WriteOutcomeUnknown`]-downcastable error once the
    /// publish outcome cannot be known, or
    /// [`io::ErrorKind::InvalidData`] for a malformed or untruthful
    /// reply.
    pub fn edit_file(
        &mut self,
        relative_path: &Path,
        expected_revision: &str,
        replacements: &[TextReplacement],
        authorize: impl FnOnce(&FileStamp) -> io::Result<()>,
    ) -> io::Result<FileStamp> {
        let deadline = Instant::now() + PAGE_REQUEST_TIMEOUT;
        let expectation = WriteExpectation::Revision(expected_revision.to_owned());
        let prepared = self.prepare_edit(
            relative_path,
            &expectation,
            replacements,
            Some(deadline),
            &|| Ok(()),
            &|| (),
        )?;
        let Some(original) = prepared.original.as_ref() else {
            // `prepare_edit` always validates `original: Some` — this is
            // unreachable for a healthy peer and must never panic.
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "edit prepare reported no original",
            ));
        };
        if let Err(error) = authorize(original) {
            // The gate denied before commit: remove the preparation
            // while the binding still admits the frame — under the same
            // absolute deadline — and never commit. The denial is still
            // what the caller sees.
            if self.gate().is_ok() {
                let _ = self.abort_prepared(&prepared, Some(deadline), &|| Ok(()), &|| ());
            }
            return Err(error);
        }
        self.commit_prepared(&prepared, Some(deadline), &|| Ok(()), &|| ())
    }

    /// The prepare half of a conditional edit: validates the request,
    /// dispatches `PrepareEdit`, and returns the ticketed
    /// [`PreparedMutation`] whose `size` is the promised published size.
    /// `expectation` must be `Revision` or `Current` — an edit always
    /// names an existing original, so `Absent` is malformed. A semantic
    /// `EditRefused` is shape-checked against the request and surfaces
    /// as a [`RefusedEdit`]-downcastable error that never retires a
    /// healthy connection.
    ///
    /// `deadline`, when present, is the caller's already-armed absolute
    /// wire budget — it is never rearmed here.
    ///
    /// # Errors
    ///
    /// Returns [`io::ErrorKind::Unsupported`] without the Client's edit
    /// capability, [`io::ErrorKind::InvalidInput`] for a request outside
    /// the bounded replacement policy, a [`RefusedEdit`]-downcastable
    /// error carrying the typed [`EditRefusal`], the [`ReadError`] the
    /// Client reported mapped onto an I/O error kind, or
    /// [`io::ErrorKind::InvalidData`] for a malformed or untruthful
    /// reply.
    pub fn prepare_edit(
        &mut self,
        relative_path: &Path,
        expectation: &WriteExpectation,
        replacements: &[TextReplacement],
        deadline: Option<Instant>,
        admit: &impl Fn() -> io::Result<()>,
        sent: &impl Fn(),
    ) -> io::Result<PreparedMutation> {
        if !self.capabilities.contains(&Capability::ConditionalEdit) {
            return Err(io::Error::new(
                io::ErrorKind::Unsupported,
                "peer admitted no conditional edit capability",
            ));
        }
        let request_bytes = replacements.iter().fold(0_usize, |total, edit| {
            total
                .saturating_add(edit.old_text.len())
                .saturating_add(edit.new_text.len())
        });
        if matches!(expectation, WriteExpectation::Absent)
            || replacements.is_empty()
            || replacements.len() > MAX_EDIT_REPLACEMENTS
            || request_bytes > MAX_EDIT_REQUEST_BYTES
            || replacements.iter().any(|edit| edit.old_text.is_empty())
            || matches!(expectation, WriteExpectation::Revision(revision) if !valid_revision(revision))
        {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "edit request outside the bounded replacement policy",
            ));
        }
        let reply = self.exchange_admitted(
            &ClientRequest::PrepareEdit {
                relative_path: relative_path.to_path_buf(),
                expectation: expectation.clone(),
                replacements: replacements.to_vec(),
            },
            deadline,
            admit,
            sent,
        )?;
        self.valid_edit_prepared(reply, expectation, replacements)
    }

    pub(super) fn valid_edit_prepared(
        &mut self,
        reply: ClientReply,
        expectation: &WriteExpectation,
        replacements: &[TextReplacement],
    ) -> io::Result<PreparedMutation> {
        let violation = |message: &'static str| io::Error::new(io::ErrorKind::InvalidData, message);
        match reply {
            ClientReply::Error { kind } => return Err(read_error(kind)),
            // A semantic refusal is only plausible when its shape could
            // come from this request: every index names a replacement we
            // sent, an ambiguity reports at least two occurrences, an
            // overlap names two distinct edits, and an empty-edits refusal
            // can never answer a nonempty request.
            ClientReply::EditRefused { reason } => {
                if !valid_edit_refusal(&reason, replacements) {
                    return Err(self.retire(violation("edit refusal contradicts the request")));
                }
                return Err(io::Error::new(
                    edit_refusal_kind(&reason),
                    RefusedEdit(reason),
                ));
            }
            _ => {}
        }
        let ClientReply::EditPrepared {
            ticket,
            original,
            published_size,
        } = reply
        else {
            return Err(self.retire(violation("unexpected reply to edit prepare")));
        };
        if ticket.get() == 0 {
            return Err(self.retire(violation("edit reply carries a zero ticket")));
        }
        // An edit always names an existing original: its stamp must be
        // valid and — for `Revision` — carry exactly the required
        // revision; for `Current` any valid stamp is the original.
        let revision_holds = match expectation {
            WriteExpectation::Revision(expected) => original.revision == *expected,
            _ => true,
        };
        if !revision_holds || stamp_violation(&original).is_some() {
            return Err(self.retire(violation("edit reply ignores the expected revision")));
        }
        // The source it claims to have applied is bounded too: a Client
        // could never have decoded an over-cap original, and the promised
        // result must stay within the file cap.
        if original.size.get() > MAX_EDIT_FILE_BYTES as u64
            || published_size.get() > MAX_EDIT_FILE_BYTES as u64
        {
            return Err(self.retire(violation("edit reply exceeds the file bound")));
        }
        Ok(PreparedMutation {
            ticket,
            original: Some(original),
            size: published_size.get(),
            connection: self.connection.clone(),
        })
    }

    /// The commit half of [`write_file`](Self::write_file): the request is
    /// encoded and gated before its first byte, so a denial there is still
    /// a clean pre-commit error. Once the first byte is written the publish
    /// crossing may already have happened — every later failure retires the
    /// connection and reports [`WriteOutcomeUnknown`], never a denial and
    /// never a retry. A `Written` receipt is trusted only when its stamp
    /// is valid *and* its size equals the exact byte count this connection
    /// submitted: a receipt describing different bytes cannot prove the
    /// publish, so the outcome stays unknown. `deadline`, when present,
    /// is the caller's already-armed absolute wire budget — it is never
    /// rearmed here.
    ///
    /// A correctly-ticketed `CommitUnknown` is valid *syntax* but never a
    /// safe ordinary outcome: the publish may have crossed, the ticket is
    /// consumed, and the peer's later accounting cannot be trusted — so
    /// it retires the connection like every other post-commit failure.
    pub(super) fn commit_write(
        &mut self,
        ticket: DecimalU64,
        expected_size: u64,
        deadline: Option<Instant>,
        admit: &impl Fn() -> io::Result<()>,
        sent: &impl Fn(),
    ) -> io::Result<FileStamp> {
        let (encoded, deadline) =
            self.encode_exchange(&ClientRequest::CommitWrite { ticket }, deadline)?;
        self.gate()?;
        admit()?;
        let outcome_unknown =
            |connection: &mut Self| connection.retire(io::Error::other(WriteOutcomeUnknown));
        let reply = match self.exchange_encoded(&encoded, deadline, sent) {
            Ok(reply) => reply,
            Err(ExchangeFailure::Refused(error)) => return Err(error),
            Err(ExchangeFailure::Unknown(_)) => return Err(outcome_unknown(self)),
        };
        match reply {
            // An explicit typed refusal means the Client never crossed:
            // it propagates as an ordinary pre-commit error.
            ClientReply::Error { kind } => Err(read_error(kind)),
            // The Client crossed but its later stages failed — the same
            // unknowable outcome as a lost acknowledgement. The ticket
            // is consumed and the publish may be durably visible, so the
            // connection retires rather than stay live on untrusted
            // accounting.
            ClientReply::CommitUnknown { ticket: echoed } if echoed == ticket => {
                Err(outcome_unknown(self))
            }
            // The control lane's cancellation won arbitration before the
            // publish crossed: the Client proved nothing was written —
            // a typed clean-cancel outcome, never an unknown one.
            ClientReply::WriteCancelled { ticket: echoed } if echoed == ticket => {
                Err(io::Error::new(io::ErrorKind::Interrupted, CommitCancelled))
            }
            ClientReply::Written {
                ticket: echoed,
                metadata,
            } if echoed == ticket => {
                if stamp_violation(&metadata).is_none() && metadata.size.get() == expected_size {
                    Ok(metadata)
                } else {
                    Err(outcome_unknown(self))
                }
            }
            _ => Err(outcome_unknown(self)),
        }
    }
}

/// One live ticketed mutation preparation on this connection: the
/// opaque handle [`LocalConnection::prepare_write`] and
/// [`LocalConnection::prepare_edit`] return, consumable exactly once by
/// [`commit_prepared`](LocalConnection::commit_prepared) or
/// [`abort_prepared`](LocalConnection::abort_prepared). `size` is the
/// byte count the `Written` receipt must match exactly.
#[derive(Debug, Clone)]
pub struct PreparedMutation {
    /// The Client-issued ticket — never zero, never reused.
    pub ticket: DecimalU64,
    /// The prepared original: `Some` for a replace, `None` for a
    /// create-only target.
    pub original: Option<FileStamp>,
    /// The exact published byte count the commit receipt must carry.
    pub size: u64,
    /// The unique identity of the admitted connection the ticket belongs
    /// to. Tickets restart at 1 on every connection, so commit and abort
    /// check this before the wire: a `PreparedMutation` moved to a
    /// different `LocalConnection` is refused locally and never consumes
    /// another Session's same-numbered ticket.
    pub connection: ConnectionId,
}

/// The peer refused a conditional edit for one stable typed semantic
/// reason. Downcast the `io::Error` to recover the [`EditRefusal`];
/// nothing was staged and the connection stayed live.
#[derive(Debug)]
pub struct RefusedEdit(pub EditRefusal);

impl std::fmt::Display for RefusedEdit {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "edit refused: {:?}", self.0)
    }
}

impl std::error::Error for RefusedEdit {}

/// Whether one typed [`EditRefusal`] could honestly answer a request
/// carrying `replacements` edits — every index must name a sent
/// replacement and the shape must be plausible. A contradiction is a
/// protocol violation, not a refusal.
fn valid_edit_refusal(reason: &EditRefusal, replacements: &[TextReplacement]) -> bool {
    let in_range = |index: u32| (index as usize) < replacements.len();
    match *reason {
        // A pre-checked request is never empty and carries no empty
        // anchor: either refusal answering one is a contradiction.
        EditRefusal::EmptyEdits => replacements.is_empty(),
        EditRefusal::EmptyAnchor { index } => replacements
            .get(index as usize)
            .is_some_and(|edit| edit.old_text.is_empty()),
        EditRefusal::Missing { index } | EditRefusal::UnsafeNormalization { index } => {
            in_range(index)
        }
        EditRefusal::Ambiguous { index, occurrences } => in_range(index) && occurrences >= 2,
        EditRefusal::Overlap { first, second } => {
            first != second && in_range(first) && in_range(second)
        }
        EditRefusal::NoChange | EditRefusal::TooLarge => true,
    }
}

/// The I/O error kind one [`EditRefusal`] maps onto — the typed reason
/// in [`RefusedEdit`] is authoritative; the kind only classifies.
fn edit_refusal_kind(reason: &EditRefusal) -> io::ErrorKind {
    match reason {
        EditRefusal::Missing { .. } => io::ErrorKind::NotFound,
        EditRefusal::TooLarge => io::ErrorKind::FileTooLarge,
        _ => io::ErrorKind::InvalidInput,
    }
}

/// The commit outcome is unknowable: the `CommitWrite` request was
/// already written — or the Client already reported crossing — when the
/// reply could not be delivered, trusted, or finished.
///
/// The write may or may not have published. It is never a clean
/// pre-commit denial and must never be retried blindly: callers downcast
/// the `io::Error` to this marker, report an unknown outcome, and learn
/// the truth only from an authoritative re-read.
#[derive(Debug)]
pub struct WriteOutcomeUnknown;

impl std::fmt::Display for WriteOutcomeUnknown {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "write commit outcome unknown; authoritative reread required"
        )
    }
}

impl std::error::Error for WriteOutcomeUnknown {}

/// A process request may have reached the Client without a verified reply.
/// The exchange retires the connection. Callers must not replay the request.
#[derive(Debug)]
pub struct ProcessOutcomeUnknown(io::Error);

impl ProcessOutcomeUnknown {
    /// Whether this exchange error carries an unknown process outcome.
    #[must_use]
    pub fn is(error: &io::Error) -> bool {
        error
            .get_ref()
            .is_some_and(<dyn std::error::Error + Send + Sync>::is::<Self>)
    }

    fn wrap(error: io::Error) -> io::Error {
        let kind = match error.kind() {
            // These kinds signal a pre-dispatch refusal to process callers.
            io::ErrorKind::WouldBlock | io::ErrorKind::Interrupted => io::ErrorKind::Other,
            kind => kind,
        };
        io::Error::new(kind, Self(error))
    }
}

impl std::fmt::Display for ProcessOutcomeUnknown {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(formatter, "process request outcome unknown: {}", self.0)
    }
}

impl std::error::Error for ProcessOutcomeUnknown {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        Some(&self.0)
    }
}

/// The peer did not confirm the physical watch registration outcome.
#[derive(Debug)]
pub struct WatchOutcomeUnknown;

impl std::fmt::Display for WatchOutcomeUnknown {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("the Client did not confirm watch registration")
    }
}

impl std::error::Error for WatchOutcomeUnknown {}

/// The commit was arbitrated cancelled on the independent control lane
/// before the Client's publish crossing: the typed `WriteCancelled`
/// reply proves nothing named by the ticket was written. Downcast the
/// `io::Error` to this marker to distinguish a proven clean cancel —
/// reportable as an ordinary cancellation — from
/// [`WriteOutcomeUnknown`], which never shares that claim.
#[derive(Debug)]
pub struct CommitCancelled;

impl std::fmt::Display for CommitCancelled {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "write commit cancelled before publish")
    }
}

impl std::error::Error for CommitCancelled {}

/// The shared content-revision check: a SHA-256 stamp is exactly 64
/// lowercase hexadecimal characters, nothing else.
fn valid_revision(revision: &str) -> bool {
    revision.len() == 64
        && revision
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

/// The shared source-stamp validator every verified reply passes: the
/// revision is a well-formed stamp and the identity is the confined
/// `dev:ino` layout — two canonical decimal halves under the u64 pair
/// bound. Anything else means the peer cannot account for its answers
/// honestly.
fn stamp_violation(stamp: &FileStamp) -> Option<&'static str> {
    if !valid_revision(&stamp.revision) {
        return Some("reply carries an invalid revision");
    }
    if stamp.identity.len() > 41
        || !stamp.identity.split_once(':').is_some_and(|(dev, ino)| {
            [dev, ino]
                .into_iter()
                .all(|part| part.parse::<DecimalU64>().is_ok())
        })
    {
        return Some("reply carries an invalid identity");
    }
    None
}

/// The shared complete-line page accounting a `Lines` or `DocumentPage`
/// reply answers to: the page's own numbers must be internally consistent
/// and must agree with the request's `offset`/`limit` and the source's
/// `logical_bytes` (the stamped source size for a `Lines` page, the
/// decoded Markdown length for a `DocumentPage`). Returns the violation
/// message — never `Ok` data — because an inconsistent reply means the
/// peer cannot account for its answers honestly.
fn page_accounting_violation(
    logical_bytes: u64,
    text: &str,
    total: u64,
    shown_lines: u32,
    end: &LineEnd,
    offset: u64,
    limit: Option<u64>,
) -> Option<&'static str> {
    let shown = u64::from(shown_lines);
    let shown_limit = u64::try_from(MAX_TEXT_LINES + 1).unwrap_or(u64::MAX);
    let line_limit = u64::try_from(MAX_TEXT_LINES).unwrap_or(u64::MAX);
    let byte_limit = u64::try_from(MAX_TEXT_BYTES).unwrap_or(u64::MAX);
    let text_bytes = u64::try_from(text.len()).unwrap_or(u64::MAX);
    if total == 0
        || shown > shown_limit
        || shown > total
        || text.len() > MAX_TEXT_BYTES
        || total - 1 > logical_bytes
        || text_bytes > logical_bytes
    {
        return Some("lines reply accounting is impossible");
    }
    // The shown text is the shown elements joined by `\n`: `shown` zero
    // is a diagnostic-only page that must carry no bytes, and a nonempty
    // page must reproduce exactly `shown` split elements — every one of
    // the `total - shown` unshown source delimiters must then fit inside
    // the logical length too.
    if shown == 0 {
        if !text.is_empty() {
            return Some("lines reply shown count is inconsistent");
        }
    } else {
        if text.split('\n').count() as u64 != shown {
            return Some("lines reply shown count is inconsistent");
        }
        let minimal = text_bytes.saturating_add(total - shown);
        if minimal > logical_bytes {
            return Some("lines reply contradicts the stamped size");
        }
        // The `line_limit + 1` shape exists only for a page that retained
        // the trailing empty element — text then ends in a newline and
        // the page completed or hit its requested bound, never a budget
        // stop.
        if shown == shown_limit
            && !(text.ends_with('\n') && matches!(end, LineEnd::Complete | LineEnd::RequestedLimit))
        {
            return Some("lines reply shown count is inconsistent");
        }
    }
    // `end` must agree with the request and the page's own numbers:
    // `remaining` is the elements the offset can still address, `selected`
    // what the window asked for — both derived without overflow. A budget
    // stop must leave the window unconsumed.
    let remaining = total.saturating_sub(offset.saturating_sub(1));
    let selected = limit.unwrap_or(u64::MAX).min(remaining);
    let valid_end = match end {
        LineEnd::Complete => shown > 0 && shown == remaining && shown == selected,
        LineEnd::RequestedLimit => shown > 0 && shown == selected && selected < remaining,
        LineEnd::LineLimit => shown == line_limit && shown < selected,
        LineEnd::ByteLimit => shown > 0 && shown <= line_limit && shown < selected,
        LineEnd::FirstLineTooLarge { bytes } => {
            shown == 0
                && remaining > 0
                && bytes.get() > byte_limit
                && bytes.get() <= logical_bytes - (total - 1)
        }
        LineEnd::OffsetPastEnd => shown == 0 && remaining == 0,
    };
    if !valid_end {
        return Some("lines reply end reason is inconsistent");
    }
    // A complete page from the first line is the whole logical source:
    // its joined text must reproduce the logical length exactly.
    if *end == LineEnd::Complete && offset == 1 && text_bytes != logical_bytes {
        return Some("lines reply contradicts the stamped size");
    }
    None
}

/// The stable physical-read refusal carried inside an `io::Error`.
///
/// A typed exchange reports a peer's `ReadError` by wrapping the code
/// here so the caller can downcast `io::Error::get_ref` to
/// [`PhysicalError`] and render an exact diagnostic — never by parsing
/// message text. The display is the same stable static description; no
/// raw path, parser message, or payload ever rides inside it.
#[derive(Debug)]
pub struct PhysicalError(pub ReadError);

impl PhysicalError {
    /// The stable static description of the wrapped code.
    fn message(&self) -> &'static str {
        match self.0 {
            ReadError::Invalid => "physical request invalid",
            ReadError::Denied => "physical access denied",
            ReadError::NotFound => "physical path not found",
            ReadError::Conflict => "physical revision conflict",
            ReadError::InvalidUtf8 => "physical source is not UTF-8",
            ReadError::UnsupportedDocument => "not a supported document source",
            ReadError::DocumentTooLarge => "document exceeds the source bound",
            ReadError::DocumentDecodeFailed => "document decode failed",
            ReadError::NoTextLayer => "document has no extractable text layer",
            ReadError::SearchBudgetExceeded => "search exceeded a bounded budget",
            ReadError::SearchNeedsDirectory => "search root is not a directory",
            ReadError::DirectoryEntryLimit => "directory exceeds the entry bound",
            ReadError::Unavailable => "physical operation unavailable",
            ReadError::Busy => "physical preparation budget is full",
        }
    }
}

impl std::fmt::Display for PhysicalError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.message())
    }
}

impl std::error::Error for PhysicalError {}

/// Maps a stable [`ReadError`] wire code onto an I/O error kind. No raw
/// path or payload crosses here — only the kind, with the code preserved
/// downcastable inside the error.
fn read_error(kind: ReadError) -> io::Error {
    let error_kind = match kind {
        ReadError::Denied => io::ErrorKind::PermissionDenied,
        ReadError::NotFound => io::ErrorKind::NotFound,
        ReadError::Conflict => io::ErrorKind::AlreadyExists,
        ReadError::InvalidUtf8 | ReadError::DocumentDecodeFailed | ReadError::NoTextLayer => {
            io::ErrorKind::InvalidData
        }
        ReadError::UnsupportedDocument => io::ErrorKind::Unsupported,
        ReadError::DocumentTooLarge | ReadError::DirectoryEntryLimit => io::ErrorKind::FileTooLarge,
        ReadError::SearchBudgetExceeded => io::ErrorKind::QuotaExceeded,
        ReadError::Invalid | ReadError::SearchNeedsDirectory => io::ErrorKind::InvalidInput,
        ReadError::Unavailable => io::ErrorKind::Other,
        ReadError::Busy => io::ErrorKind::ResourceBusy,
    };
    io::Error::new(error_kind, PhysicalError(kind))
}

/// The small absolute budget a still-healthy connection gets to send its
/// WebSocket Close frame at orderly teardown — long enough for loopback,
/// short enough that a dead peer cannot stall `Drop`.
const ORDERLY_CLOSE_BUDGET: Duration = Duration::from_secs(1);

impl Drop for LocalConnection {
    /// Orderly teardown sends the transport's graceful close first — a
    /// WebSocket peer reads a real RFC 6455 Close frame, not a bare TCP
    /// reset — then detaches the binding. Failure paths (`retire`,
    /// `disconnect`, `revoke`) already shut the channel down abruptly
    /// and never take this path: a forced close must not masquerade as a
    /// clean finish.
    fn drop(&mut self) {
        self.close();
    }
}
