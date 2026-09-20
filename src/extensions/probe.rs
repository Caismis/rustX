//! Test-only compiled domain, selected by the real frozen native composition.
use crate::context::{
    ContextAssemblyError, ContextContributor, ContextProposal, ContributorInputSnapshot,
    UserMessageProposal,
};
use crate::runtime::identity::NativeContextContributor;
use std::sync::{
    Arc,
    atomic::{AtomicUsize, Ordering},
};

impl crate::agent::lifecycle::ToolResultObserver for ProbeConfig {
    fn observe_tool_result<'a>(
        &'a self,
        fact: &'a crate::agent::lifecycle::ToolResultObservation<'a>,
    ) -> futures_util::future::BoxFuture<
        'a,
        Result<Vec<ContextProposal>, crate::agent::lifecycle::LifecycleError>,
    > {
        Box::pin(async move {
            Ok(vec![ContextProposal::NativeUserMessage {
                message: UserMessageProposal {
                    content: vec![crate::message::UserContentBlock::Text(
                        crate::message::content::TextBlock {
                            text: format!("settled fact {}", fact.batch_position),
                        },
                    )],
                },
                metadata: crate::message::ContextKind::NativeEnvironment,
                presentation: None,
                emissions: vec![crate::message::ContributionEmission {
                    key: format!("tool-{}", fact.call_id),
                    fingerprint: "settled".into(),
                }],
            }])
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Behavior {
    Empty,
    Normal,
    OptionalFailure,
    MandatoryFailure,
    Invalid,
}

#[derive(Debug, Clone)]
pub struct ProbeConfig {
    pub identity: NativeContextContributor,
    pub behavior: Behavior,
    pub captures: Arc<AtomicUsize>,
    pub revision: Arc<AtomicUsize>,
    pub payload_bytes: Option<usize>,
    pub observe_tools: bool,
    pub gate: Option<(
        tokio::sync::watch::Sender<bool>,
        tokio::sync::watch::Receiver<bool>,
    )>,
}

impl PartialEq for ProbeConfig {
    fn eq(&self, other: &Self) -> bool {
        self.identity == other.identity
            && self.behavior == other.behavior
            && Arc::ptr_eq(&self.captures, &other.captures)
            && Arc::ptr_eq(&self.revision, &other.revision)
            && self.payload_bytes == other.payload_bytes
            && self.observe_tools == other.observe_tools
    }
}
impl Eq for ProbeConfig {}

impl ProbeConfig {
    pub fn new(identity: NativeContextContributor, behavior: Behavior) -> Self {
        Self {
            identity,
            behavior,
            captures: Arc::new(AtomicUsize::new(0)),
            revision: Arc::new(AtomicUsize::new(1)),
            payload_bytes: None,
            observe_tools: false,
            gate: None,
        }
    }
}

impl ContextContributor for ProbeConfig {
    fn requirement(&self) -> crate::context::assembly::ContributionRequirement {
        if self.behavior == Behavior::MandatoryFailure {
            crate::context::assembly::ContributionRequirement::Mandatory
        } else {
            crate::context::assembly::ContributionRequirement::Optional
        }
    }

    fn contribute<'a>(
        &'a self,
        _: &'a ContributorInputSnapshot,
    ) -> futures_util::future::BoxFuture<'a, Result<Vec<ContextProposal>, ContextAssemblyError>>
    {
        Box::pin(async move {
            let revision = self.revision.load(Ordering::SeqCst);
            self.captures.fetch_add(1, Ordering::SeqCst);
            if let Some((entered, release)) = &self.gate {
                entered.send_replace(true);
                release
                    .clone()
                    .wait_for(|released| *released)
                    .await
                    .map_err(|error| ContextAssemblyError::AcquisitionFailed(error.to_string()))?;
            }
            match self.behavior {
                Behavior::Empty => Ok(vec![]),
                Behavior::OptionalFailure | Behavior::MandatoryFailure => {
                    Err(ContextAssemblyError::AcquisitionFailed(
                        "controlled acquisition failure".into(),
                    ))
                }
                Behavior::Invalid => Ok(vec![ContextProposal::NativeUserMessage {
                    message: UserMessageProposal {
                        content: vec![crate::message::UserContentBlock::Text(
                            crate::message::content::TextBlock {
                                text: "forged".into(),
                            },
                        )],
                    },
                    metadata: crate::message::ContextKind::RuntimeToolObservation,
                    presentation: None,
                    emissions: vec![],
                }]),
                Behavior::Normal => Ok(vec![ContextProposal::NativeUserMessage {
                    message: UserMessageProposal {
                        content: vec![crate::message::UserContentBlock::Text(
                            crate::message::content::TextBlock {
                                text: self.payload_bytes.map_or_else(
                                    || {
                                        format!(
                                            "{} revision {revision}",
                                            self.identity.logical_key()
                                        )
                                    },
                                    |bytes| "x".repeat(bytes),
                                ),
                            },
                        )],
                    },
                    metadata: crate::message::ContextKind::NativeEnvironment,
                    presentation: None,
                    emissions: vec![crate::message::ContributionEmission {
                        key: "active".into(),
                        fingerprint: format!("revision-{revision}"),
                    }],
                }]),
            }
        })
    }
}
