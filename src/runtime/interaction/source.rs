//! Immutable read layout owned by one admitted pending interaction.
//! It is derived once on physical inspection after native publication, and has no settlement authority.
#[cfg(test)]
use super::InteractionKind;
use super::InteractionRequest;
#[cfg(test)]
use crate::runtime::identity::InteractionId;
#[cfg(test)]
use crate::runtime::read_layout::READ_WORK;
#[cfg(test)]
use serde_json::Value;
use std::sync::{Arc, OnceLock};

#[derive(Debug)]
pub(crate) struct PendingInteractionSource {
    request: Arc<InteractionRequest>,
    header: crate::observation::interaction::RuntimeClientInteractionHeader,
    layout: OnceLock<Result<crate::runtime::read_layout::SeekableObservationSource, &'static str>>,
}
impl PendingInteractionSource {
    pub(crate) fn new(request: InteractionRequest) -> Arc<Self> {
        let header = crate::observation::interaction::interaction_request_header(
            &request,
            super::InteractionSource::Primary,
        );
        Arc::new(Self {
            request: Arc::new(request),
            header,
            layout: OnceLock::new(),
        })
    }
    fn build_layout(
        request: &InteractionRequest,
    ) -> Result<crate::runtime::read_layout::SeekableObservationSource, &'static str> {
        // Stream the borrowed native request through its typed structural codec;
        // opaque arguments keep their original JSON semantics without a second Value tree.
        crate::runtime::read_layout::SeekableObservationSource::new(request)
    }
    pub(crate) fn header(
        &self,
    ) -> &crate::observation::interaction::RuntimeClientInteractionHeader {
        &self.header
    }
    #[cfg(test)]
    pub(crate) fn request(&self) -> &InteractionRequest {
        &self.request
    }
    pub(crate) fn request_arc(&self) -> Arc<InteractionRequest> {
        self.request.clone()
    }
    #[cfg(test)]
    pub(crate) fn id(&self) -> &InteractionId {
        &self.request.id
    }
    fn body_path(path: &[String]) -> &[String] {
        if path.first().is_some_and(|part| part == "request") {
            &path[1..]
        } else {
            path
        }
    }
    pub(crate) fn page(
        &self,
        path: &[String],
        offset: u64,
        limit: usize,
    ) -> Result<
        crate::runtime::read_layout::ObservationPage,
        crate::runtime::read_layout::ObservationReadError,
    > {
        self.layout
            .get_or_init(|| Self::build_layout(&self.request))
            .as_ref()
            .map_err(|error| *error)?
            .page(Self::body_path(path), offset, limit)
    }
    pub(crate) fn read(
        &self,
        path: &[String],
        offset: u64,
        max_bytes: u32,
    ) -> Result<
        crate::runtime::read_layout::ObservationChunk,
        crate::runtime::read_layout::ObservationReadError,
    > {
        self.layout
            .get_or_init(|| Self::build_layout(&self.request))
            .as_ref()
            .map_err(|error| *error)?
            .read(Self::body_path(path), offset, max_bytes)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::runtime::identity::{AttemptId, ConversationId, ToolCallId, ToolId};
    fn source(arguments: Value) -> Arc<PendingInteractionSource> {
        PendingInteractionSource::new(InteractionRequest {
            id: InteractionId::new("source-index"),
            conversation_id: ConversationId::new("conv_17886270-8682-7777-8444-99abcf664492"),
            execution: rustx::runtime::interaction::InteractionExecution::Attempt {
                attempt_id: AttemptId::new("attempt-source"),
                turn: 1,
            },
            kind: InteractionKind::Approval {
                invocation_id: crate::tools::types::ToolInvocationId::Agent {
                    call_id: ToolCallId::new("source-call"),
                },
                tool_id: ToolId::new("tool-source"),
                tool_name: "source".into(),
                origin: crate::tools::types::ToolOrigin::Builtin,
                mode: crate::tools::types::ToolInvocationMode::Foreground,
                arguments,
                reason: "native-source-test".into(),
            },
        })
    }
    #[test]
    fn t08_t12_t15_t20_pending_source_reads_seekable_chunks_without_reencoding_original() {
        let text = "α🙂".repeat(3_000_000);
        let args = serde_json::json!({"large":text,"small":(0..73).collect::<Vec<_>>()});
        let expected = serde_json::to_vec(&args).unwrap();
        let source = source(args);
        assert!(
            source.layout.get().is_none(),
            "native admission performs no layout IO"
        );
        let request = source.request_arc();
        assert!(std::ptr::eq(source.request(), request.as_ref()));
        let path = vec!["kind".into(), "arguments".into()];
        let fields = source.page(&path, 0, 32).unwrap();
        assert_eq!(fields.total, 2);
        assert!(fields.fields[0].value.is_none());
        assert!(fields.fields[0].encoded_bytes > 16 * 1024 * 1024);
        let rows = source
            .page(&["kind".into(), "arguments".into(), "@1".into()], 0, 32)
            .unwrap();
        assert_eq!(rows.total, 73);
        assert_eq!(rows.next_offset, Some(32));
        let last = source
            .page(&["kind".into(), "arguments".into(), "@1".into()], 64, 32)
            .unwrap();
        assert_eq!(last.fields.len(), 9);
        assert!(last.next_offset.is_none());
        let mut offset = 0;
        let mut actual = Vec::new();
        loop {
            let chunk = source.read(&path, offset, 65_536).unwrap();
            let (segments, bytes) = READ_WORK.with(std::cell::Cell::get);
            assert!(segments <= 2);
            assert!(bytes <= 131_072);
            actual.extend_from_slice(&chunk.bytes);
            if let Some(next) = chunk.next_offset {
                assert!(next > offset);
                offset = next;
            } else {
                break;
            }
        }
        assert_eq!(actual, expected);
        assert!(source.read(&path, expected.len() as u64 + 1, 1).is_err());
        assert!(source.read(&path, 0, 65_537).is_err());
        assert!(
            source
                .read(&["kind".into(), "arguments".into(), "foreign".into()], 0, 1)
                .is_err()
        );
    }
    #[test]
    fn t12_t15_pending_source_distinguishes_numeric_keys_and_bounds_recursive_inline_values() {
        let source = source(serde_json::json!({"0":"zero","1":(0..100).collect::<Vec<_>>()}));
        let path = vec!["kind".into(), "arguments".into()];
        let page = source.page(&path, 0, 32).unwrap();
        assert!(page.fields[0].value.is_some());
        assert!(page.fields[1].value.is_none());
        let literal = source
            .read(&["kind".into(), "arguments".into(), "0".into()], 0, 32)
            .unwrap();
        let ordinal = source
            .read(&["kind".into(), "arguments".into(), "@0".into()], 0, 32)
            .unwrap();
        assert_eq!(literal.bytes, ordinal.bytes);
        assert_eq!(literal.bytes, b"\"zero\"");
    }
    #[test]
    fn t10_pending_layout_failure_preserves_native_request_authority() {
        let mut value = Value::Null;
        for _ in 0..260 {
            value = Value::Array(vec![value]);
        }
        let source = source(value);
        assert!(source.layout.get().is_none());
        assert!(matches!(
            source.request().kind,
            InteractionKind::Approval { .. }
        ));
        assert_eq!(source.header().request_id, source.request().id);
        assert!(source.read(&[], 0, 1).is_err());
        assert!(source.page(&[], 0, 32).is_err());
        assert!(source.layout.get().unwrap().is_err());
    }

    #[test]
    fn t10_t12_t15_pending_workflow_header_defers_original_node_without_admission_io() {
        use crate::runtime::workflow::{
            WorkflowBlockInstance, WorkflowDefinitionPath, WorkflowNodeInstance,
        };
        let native = WorkflowNodeInstance {
            block: WorkflowBlockInstance {
                run: crate::runtime::workflow::WorkflowRunId {
                    conversation_id: ConversationId::new(
                        "conv_17886270-8682-7777-8444-99abcf664492",
                    ),
                    origin: (AttemptId::new("workflow-source")).into(),
                    invocation: 1,
                },
                definition: WorkflowDefinitionPath {
                    workflow_id: crate::runtime::workflow::WorkflowId::parse(
                        "workflow-observation",
                    )
                    .unwrap(),
                    blocks: vec!["block-name".repeat(80_000)],
                },
                invocations: vec![1, 2],
            },
            node: "node-name".repeat(80_000),
            visit: 7,
        };
        let mut original = Vec::new();
        crate::observation::structural_json::to_writer(&mut original, &native).unwrap();
        let mut request = source(Value::Null).request().clone();
        let InteractionKind::Approval { invocation_id, .. } = &mut request.kind else {
            unreachable!()
        };
        *invocation_id = crate::tools::types::ToolInvocationId::Workflow {
            node: Box::new(native),
        };
        let source = PendingInteractionSource::new(request);
        assert!(
            source.layout.get().is_none(),
            "actionable native admission has no derived IO"
        );
        let header = source.header();
        assert!(header.workflow_node.is_none());
        assert!(header.workflow_node_deferred);
        assert!(crate::observation::bounded::bounded_bytes(header, 16_384).is_some());
        let path = header.workflow_node_path.as_ref().unwrap();
        let mut offset = 0;
        let mut reconstructed = Vec::new();
        loop {
            let chunk = source.read(path, offset, 65_536).unwrap();
            reconstructed.extend(chunk.bytes);
            if let Some(next) = chunk.next_offset {
                assert!(next > offset);
                offset = next;
            } else {
                break;
            }
        }
        assert!(
            reconstructed == original,
            "source bytes preserve the typed native codec"
        );
        assert_eq!(source.header().request_id, source.request().id);
    }

    #[test]
    fn t15_pending_source_preserves_giant_property_keys_with_ordinal_key_reads() {
        let key = "工具-key".repeat(100_000);
        let source = source(serde_json::json!({key.clone():"value"}));
        let path = vec!["kind".into(), "arguments".into()];
        let page = source.page(&path, 0, 32).unwrap();
        assert!(!page.fields[0].key_complete);
        assert!(page.fields[0].key.as_ref().unwrap().len() <= 256);
        let path = vec![
            "kind".into(),
            "arguments".into(),
            "@0".into(),
            "$key".into(),
        ];
        let mut offset = 0;
        let mut bytes = Vec::new();
        loop {
            let chunk = source.read(&path, offset, 65_536).unwrap();
            bytes.extend(chunk.bytes);
            if let Some(next) = chunk.next_offset {
                offset = next;
            } else {
                break;
            }
        }
        assert_eq!(serde_json::from_slice::<String>(&bytes).unwrap(), key);
    }
}
