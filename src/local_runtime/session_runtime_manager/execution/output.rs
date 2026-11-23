use rustx_execution_protocol::{
    ClientRequest,
    process::OperationId,
    save::{ConversationId, Locator, SaveOwner, SaveReply, SaveRequest},
};
use rustx_execution_server::ContinuationPage;

use super::{
    Arc, DataCapacity, ExecutionCancellation, Instant, LocalExecutionEndpoint,
    SessionExecutionScope, SessionId, cancelled_before_dispatch, io,
};

/// One optional-save selection. All steps use the exact endpoint selected at
/// the preview crossing, even if a Session subsequently loses its Client.
pub(crate) struct OutputBinding {
    pub(crate) owner: SaveOwner,
    endpoint: Arc<LocalExecutionEndpoint>,
}

impl SessionExecutionScope {
    pub(crate) fn session_id(&self) -> &SessionId {
        &self.session
    }

    pub(crate) fn bind_output(&self, conversation: &str) -> Option<OutputBinding> {
        let endpoint = self.endpoint().ok()?;
        if endpoint.managed_output.get() != Some(&true)
            || !endpoint.dispatch_open()
            || endpoint.control().is_none()
        {
            return None;
        }
        let admission = endpoint.admission()?;
        let owner = SaveOwner {
            incarnation: admission.server_incarnation,
            session: admission.session.clone(),
            instance: admission.instance.clone(),
            generation: admission.generation,
            conversation: ConversationId::parse(conversation)?,
            operation: OperationId::new(),
        };
        Some(OutputBinding { owner, endpoint })
    }

    pub(crate) async fn read_saved_output(
        &self,
        locator: Locator,
        conversation: ConversationId,
        offset: u64,
        limit: Option<u64>,
        cancellation: &ExecutionCancellation,
    ) -> io::Result<rustx_confined_fs::TextPage> {
        self.inspect_saved_output(
            ClientRequest::ReadSavedOutput {
                locator,
                conversation,
                offset: offset.into(),
                limit: limit.map(Into::into),
            },
            cancellation,
        )
        .await
        .map(|page| match page {
            ContinuationPage::Lines(lines) => super::project_lines(lines).1,
            _ => unreachable!("validated saved output Read reply"),
        })
    }

    pub(crate) async fn read_saved_output_range(
        &self,
        locator: Locator,
        conversation: ConversationId,
        offset: u64,
        cancellation: &ExecutionCancellation,
    ) -> io::Result<rustx_execution_server::RangeRead> {
        self.inspect_saved_output(
            ClientRequest::ReadSavedOutputRange {
                locator,
                conversation,
                offset: offset.into(),
            },
            cancellation,
        )
        .await
        .map(|page| match page {
            ContinuationPage::Range(range) => range,
            _ => unreachable!("validated saved output range"),
        })
    }

    pub(crate) async fn search_saved_output(
        &self,
        locator: Locator,
        conversation: ConversationId,
        spec: rustx_execution_protocol::SearchSpec,
        cancellation: &ExecutionCancellation,
    ) -> io::Result<rustx_execution_server::SearchResult> {
        self.inspect_saved_output(
            ClientRequest::SearchSavedOutput {
                locator,
                conversation,
                spec,
            },
            cancellation,
        )
        .await
        .map(|page| match page {
            ContinuationPage::Search(search) => search,
            _ => unreachable!("validated saved output Grep reply"),
        })
    }

    async fn inspect_saved_output(
        &self,
        request: ClientRequest,
        cancellation: &ExecutionCancellation,
    ) -> io::Result<ContinuationPage> {
        self.dispatch_data(
            cancellation,
            DataCapacity::Immediate,
            move |endpoint, data, writer, deadline, cancellation| {
                let _writer = writer;
                endpoint.with_connection(Some(data), true, |connection| {
                    connection.inspect_output(&request, deadline, || {
                        cancellation
                            .with_active(|| ())
                            .ok_or_else(cancelled_before_dispatch)
                    })
                })
            },
        )
        .await
    }
}

impl OutputBinding {
    pub(crate) async fn exchange(
        &self,
        request: SaveRequest,
        deadline: Instant,
        cancellation: &ExecutionCancellation,
    ) -> io::Result<SaveReply> {
        SessionExecutionScope::dispatch_data_on(
            self.endpoint.clone(),
            cancellation,
            DataCapacity::WaitForSlot,
            deadline,
            move |endpoint, data, writer, deadline, cancellation| {
                let _writer = writer;
                endpoint.with_connection(Some(data), true, |connection| {
                    connection.save_by(&request, deadline, || {
                        cancellation
                            .with_active(|| ())
                            .ok_or_else(cancelled_before_dispatch)
                    })
                })
            },
        )
        .await
    }
}
