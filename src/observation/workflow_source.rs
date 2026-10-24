//! One immutable native Workflow cut and its finite display header.
use crate::runtime::workflow::read_model::{WorkflowRunView, WorkflowSnapshot};
use serde::{Serialize, Serializer};
use std::sync::{Arc, OnceLock};

#[derive(Debug, Clone)]
pub(crate) struct WorkflowReadSource {
    original: Arc<WorkflowSnapshot>,
    prepared: Arc<OnceLock<(WorkflowSnapshot, Vec<super::cut::CollectionFrontier>)>>,
}
impl WorkflowReadSource {
    pub(crate) fn new(original: Arc<WorkflowSnapshot>) -> Self {
        Self {
            original,
            prepared: Arc::default(),
        }
    }
    pub(crate) fn revision(&self) -> crate::runtime::workflow::read_model::WorkflowRevision {
        self.original.revision
    }
    pub(crate) fn prepare(&self) {
        self.prepared.get_or_init(|| {
            let mut runs = Vec::with_capacity(self.original.runs.len());
            let mut collections = Vec::with_capacity(self.original.runs.len() + 1);
            for (ordinal, run) in self.original.runs.iter().enumerate() {
                // Serialize only borrowed scalar metadata before copying any
                // identifiers. Native retained instance bodies stay in the Arc.
                let Some(bytes) = super::bounded::bounded_item_bytes(&RunHeader(run), 4096) else {
                    continue;
                };
                let Ok(mut header) = serde_json::from_slice::<WorkflowRunView>(&bytes) else {
                    continue;
                };
                let mut omitted = false;
                header.instances = run
                    .instances
                    .iter()
                    .take(32)
                    .filter_map(|instance| {
                        let value = super::bounded::bounded_clone(instance, 2048);
                        omitted |= value.is_none();
                        value
                    })
                    .collect();
                let loaded = header.instances.len() as u64;
                collections.push(super::cut::CollectionFrontier {
                    domain: format!("workflows.runs.@{ordinal}.instances"),
                    revision: self.original.revision.0,
                    total: run.instances.len() as u64,
                    loaded,
                    next_offset: if omitted {
                        Some(0)
                    } else {
                        (loaded < run.instances.len() as u64).then_some(loaded)
                    },
                });
                runs.push(header);
            }
            let loaded = runs.len() as u64;
            collections.push(super::cut::CollectionFrontier {
                domain: "workflows.runs".into(),
                revision: self.original.revision.0,
                total: self.original.runs.len() as u64,
                loaded,
                next_offset: (loaded < self.original.runs.len() as u64).then_some(0),
            });
            (
                WorkflowSnapshot {
                    revision: self.original.revision,
                    runs,
                    omitted_runs: self.original.omitted_runs,
                },
                collections,
            )
        });
    }
    pub(crate) fn header(&self) -> WorkflowSnapshot {
        self.prepared
            .get()
            .expect("native Workflow header prepared outside Projection")
            .0
            .clone()
    }
    pub(crate) fn frontiers(&self) -> Vec<super::cut::CollectionFrontier> {
        self.prepared
            .get()
            .expect("native Workflow header prepared")
            .1
            .clone()
    }
}
impl Serialize for WorkflowReadSource {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        self.original.serialize(serializer)
    }
}
impl schemars::JsonSchema for WorkflowReadSource {
    fn schema_name() -> std::borrow::Cow<'static, str> {
        "WorkflowReadSource".into()
    }
    fn json_schema(generator: &mut schemars::SchemaGenerator) -> schemars::Schema {
        generator.subschema_for::<WorkflowSnapshot>()
    }
}
struct RunHeader<'a>(&'a WorkflowRunView);
impl Serialize for RunHeader<'_> {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        use serde::ser::SerializeStruct;
        let run = self.0;
        let mut value = serializer.serialize_struct("WorkflowRunView", 12)?;
        value.serialize_field("id", &run.id)?;
        value.serialize_field("workflow_id", &run.workflow_id)?;
        value.serialize_field("program_digest", &run.program_digest)?;
        value.serialize_field("resource_revision", &run.resource_revision)?;
        value.serialize_field("tool_call_id", &run.tool_call_id)?;
        value.serialize_field("state", &run.state)?;
        value.serialize_field(
            "instances",
            &[] as &[crate::runtime::workflow::read_model::WorkflowInstanceView],
        )?;
        value.serialize_field("omitted_instances", &run.omitted_instances)?;
        value.serialize_field("steps_consumed", &run.steps_consumed)?;
        value.serialize_field("steps_max", &run.steps_max)?;
        value.serialize_field("agents_consumed", &run.agents_consumed)?;
        value.serialize_field("model_calls_consumed", &run.model_calls_consumed)?;
        value.end()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::runtime::workflow::read_model::{
        WorkflowInstanceView, WorkflowNodeKind, WorkflowRevision, WorkflowState,
    };

    #[test]
    #[allow(clippy::too_many_lines)] // One native cut and all its original pagination assertions.
    fn t14_t15_workflow_source_retains_native_order_and_finite_cut_headers() {
        let node = crate::runtime::workflow::test_instance("observation", "node");
        let instances = (0..73)
            .map(|ordinal| WorkflowInstanceView {
                block: crate::runtime::workflow::WorkflowBlockInstance {
                    invocations: vec![ordinal],
                    ..node.block.clone()
                },
                node: Some(format!("node-{ordinal}")),
                visit: Some(0),
                kind: WorkflowNodeKind::Tool,
                state: WorkflowState::Running,
                child: None,
                child_agent_id: None,
                auxiliary_call_id: None,
                invocation: None,
                tool_id: None,
                interaction: None,
                iteration: None,
                iterations_max: None,
                loop_exit: None,
                checks_passed: None,
                review_accepted: None,
            })
            .collect::<Vec<_>>();
        let original = Arc::new(WorkflowSnapshot {
            revision: WorkflowRevision(7),
            omitted_runs: 3,
            runs: vec![WorkflowRunView {
                id: node.block.run,
                workflow_id: node.block.definition.workflow_id,
                program_digest: "d".repeat(64),
                resource_revision: crate::runtime::identity::RuntimeResourceRevision::new(5),
                tool_call_id: Some(crate::runtime::identity::ToolCallId::new("call")),
                state: WorkflowState::Running,
                instances,
                omitted_instances: 2,
                steps_consumed: 13,
                steps_max: 4096,
                agents_consumed: 1,
                model_calls_consumed: 2,
            }],
        });
        let source = WorkflowReadSource::new(original.clone());
        source.prepare();
        let header = source.header();
        assert_eq!(source.revision(), WorkflowRevision(7));
        assert_eq!(header.revision, original.revision);
        assert_eq!(header.omitted_runs, 3);
        assert_eq!(header.runs[0].instances, original.runs[0].instances[..32]);
        assert_eq!(header.runs[0].omitted_instances, 2);
        assert_eq!(header.runs[0].steps_consumed, 13);
        assert_eq!(header.runs[0].model_calls_consumed, 2);
        assert_eq!(
            source.frontiers()[0],
            crate::observation::cut::CollectionFrontier {
                domain: "workflows.runs.@0.instances".into(),
                revision: 7,
                total: 73,
                loaded: 32,
                next_offset: Some(32),
            }
        );
        assert!(crate::observation::bounded::bounded_bytes(&header, 786_432).is_some());
        assert_eq!(
            serde_json::to_value(&source).unwrap(),
            serde_json::to_value(&*original).unwrap()
        );
        let source = crate::runtime_client::domain::DomainSource::Workflow(Arc::new(
            crate::runtime_client::domain::SourceValue::new(Arc::new(source)),
        ));
        let cut = crate::observation::cut::CheckpointReadCut {
            conversation_id: crate::runtime::identity::ConversationId::new(
                "conv_01900000-0000-7000-8000-000000000012",
            ),
            activation_id: None,
            revision: 7,
            through: 7,
            source_cut: None,
            domain_revisions: std::collections::BTreeMap::default(),
            collection_cuts: std::collections::BTreeMap::default(),
            canonical_foreground: None,
            live_publication: None,
        };
        for (offset, next) in [(0, Some(32)), (32, Some(64)), (64, None)] {
            let page = source
                .page(
                    &cut,
                    "workflows.runs.@0.instances",
                    &["runs".into(), "@0".into(), "instances".into()],
                    offset,
                    32,
                    false,
                )
                .unwrap();
            assert_eq!(page.total, 73);
            assert_eq!(page.next_offset, next);
            assert_eq!(page.items[0].offset, offset);
            assert_eq!(
                page.items[0].value.as_ref().unwrap()["node"],
                format!("node-{offset}")
            );
        }
    }
}
