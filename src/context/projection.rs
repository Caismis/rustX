//! The explicit context projection boundary.
//!
//! Since M7.5 (Issue #54) the canonical conversation model is
//! [`MessageLedger`] + [`ConversationSurface`]: the Ledger holds immutable
//! committed facts and the Surface is the sole authority for what is
//! currently active and in what order.
//!
//! [`ContextProjection`] is the finite request-preparation value derived
//! from one exact Surface state:
//!
//! ```text
//! Surface @ SurfaceRevision
//!   → finite active MessageIds
//!   → keyed Ledger hydration
//!   → ContextProjection { whole canonical messages, status, catalog }
//! ```
//!
//! Every projected item is a **complete canonical message**. The projection
//! never creates a partial Assistant message or a second message identity.
//!
//! The projected input measurement carries explicit provenance
//! ([`TokenMeasurement`]): a provider-reported measurement applies only when
//! the request context it measured — the exact Surface revision, the exact
//! hydrated messages, and the exact Agent Status / Skill catalog attachments
//! — is identical.
//!
//! [`MessageLedger`]: crate::conversation::MessageLedger
//! [`ConversationSurface`]: crate::conversation::ConversationSurface
//! [`TokenMeasurement`]: crate::runtime::types::TokenMeasurement

use serde::{Deserialize, Serialize};

use crate::conversation::SurfaceRevision;
use crate::message::types::MessageBlock;
use crate::model::types::{AgentStatusAttachment, SkillCatalogAttachment};
use crate::runtime::types::TokenMeasurement;

/// The deterministic model-visible projection of one Conversation Surface
/// revision.
///
/// The projection is a pure function of (Surface revision, hydrated active
/// messages, tool definitions, observed provider usage, Agent Status
/// attachment, Skill catalog attachment): identical inputs produce an
/// identical projection, including its estimated input measurement.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ContextProjection {
    /// The exact Surface revision this projection was built from.
    ///
    /// This is the seam Issue #55's `RequestSnapshot` consumes: a request's
    /// visible-conversation identity is `ConversationSurface @ revision`,
    /// never "whatever messages happened to exist around that time".
    pub surface_revision: SurfaceRevision,
    /// The ordered model-visible **complete canonical** messages of the
    /// current Surface.
    pub messages: Vec<MessageBlock>,
    /// The ephemeral Agent Status attachment of a pending fresh inbound
    /// turn, when one exists. The attachment is projection-only: it is never
    /// a Ledger fact and never appears on the Surface.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agent_status: Option<AgentStatusAttachment>,
    /// The ephemeral Skill catalog attachment of the attempt's immutable
    /// Skill snapshot, when any Skill is active. The attachment is
    /// projection-only capability context: it is never a Ledger fact and
    /// never appears on the Surface.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub skill_catalog: Option<SkillCatalogAttachment>,
    /// The deterministic planned input measurement of the full model
    /// request, including non-compacted contributors such as tool
    /// definitions, the Agent Status attachment, and the Skill catalog.
    pub estimated_input: TokenMeasurement,
}

impl ContextProjection {
    /// A deterministic fingerprint of this request context.
    ///
    /// The fingerprint is a FNV-1a hash over the canonical JSON of the
    /// Surface revision, the hydrated active messages, the exact Agent
    /// Status attachment, and the exact Skill catalog attachment. It decides
    /// whether a provider-reported input measurement applies to exactly this
    /// request context: a reported measurement is authoritative only when
    /// the context being measured is byte-for-byte identical. A Surface
    /// rewrite, an append, a changed status snapshot, or a changed catalog
    /// therefore all invalidate a stale measurement.
    ///
    /// # Panics
    ///
    /// Panics only if the canonical projection fails to serialize, which is
    /// unreachable for the canonical runtime-owned types.
    #[must_use]
    pub fn fingerprint(&self) -> u64 {
        let bytes = serde_json::to_vec(&self.surface_revision)
            .expect("surface revision serializes")
            .into_iter()
            .chain(serde_json::to_vec(&self.messages).expect("canonical messages serialize"))
            .chain(
                serde_json::to_vec(&self.agent_status).expect("agent status attachment serializes"),
            )
            .chain(
                serde_json::to_vec(&self.skill_catalog)
                    .expect("skill catalog attachment serializes"),
            );
        let mut hash = 0xcbf2_9ce4_8422_2325_u64;
        for byte in bytes {
            hash ^= u64::from(byte);
            hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
        }
        hash
    }
}

#[cfg(test)]
mod tests {
    use super::ContextProjection;
    use crate::context::status::{AgentStatusFact, AgentStatusSectionData, AgentStatusSectionId};
    use crate::conversation::SurfaceRevision;
    use crate::message::content::TextBlock;
    use crate::message::types::{
        InboundKind, MessageBlock, UserContentBlock, UserMessageBlock, UserSource,
    };
    use crate::model::types::{AgentStatusAttachment, SkillCatalogAttachment};
    use crate::runtime::identity::MessageId;
    use crate::runtime::types::{TokenMeasurement, TokenMeasurementSource};

    fn projection() -> ContextProjection {
        ContextProjection {
            surface_revision: SurfaceRevision::new(1),
            messages: vec![MessageBlock::User(UserMessageBlock {
                id: MessageId::new("msg-1"),
                content: vec![UserContentBlock::Text(TextBlock {
                    text: "hello".to_owned(),
                })],
                source: UserSource::Human,
                kind: InboundKind::Message,
                timestamp: None,
            })],
            agent_status: None,
            skill_catalog: None,
            estimated_input: TokenMeasurement {
                input_tokens: 7,
                source: TokenMeasurementSource::Estimated,
            },
        }
    }

    /// Identical request contexts produce identical fingerprints; a
    /// different Surface revision does not.
    #[test]
    fn fingerprints_are_deterministic_and_discriminating() {
        let projection = projection();
        assert_eq!(projection.fingerprint(), projection.clone().fingerprint());
        let mut other_revision = projection.clone();
        other_revision.surface_revision = SurfaceRevision::new(2);
        assert_ne!(
            projection.fingerprint(),
            other_revision.fingerprint(),
            "a surface revision change must invalidate a stale measurement"
        );
        let mut other_messages = projection.clone();
        other_messages.messages.clear();
        assert_ne!(projection.fingerprint(), other_messages.fingerprint());
    }

    /// The exact Skill catalog attachment participates in the fingerprint.
    #[test]
    fn skill_catalog_changes_the_fingerprint() {
        let projection = projection();
        let mut with_catalog = projection.clone();
        with_catalog.skill_catalog = Some(SkillCatalogAttachment {
            rendered: "## Skills\n\n- pdf: ...".to_owned(),
        });
        let mut other_catalog = with_catalog.clone();
        other_catalog
            .skill_catalog
            .as_mut()
            .expect("catalog present")
            .rendered = "## Skills\n\n- pdf: ...changed...".to_owned();
        assert_ne!(projection.fingerprint(), with_catalog.fingerprint());
        assert_ne!(with_catalog.fingerprint(), other_catalog.fingerprint());
    }

    /// The exact Agent Status attachment participates in the fingerprint.
    #[test]
    fn agent_status_changes_the_fingerprint() {
        let projection = projection();
        let mut with_status = projection.clone();
        with_status.agent_status = Some(AgentStatusAttachment {
            target_message_id: MessageId::new("msg-1"),
            rendered:
                "<system-reminder>\nCurrent time: 2026-08-08T16:31:00+08:00\n</system-reminder>"
                    .to_owned(),
        });
        let mut other_snapshot = with_status.clone();
        other_snapshot
            .agent_status
            .as_mut()
            .expect("status present")
            .rendered =
            "<system-reminder>\nCurrent time: 2026-08-08T16:32:00+08:00\n</system-reminder>"
                .to_owned();
        assert_ne!(projection.fingerprint(), with_status.fingerprint());
        assert_ne!(with_status.fingerprint(), other_snapshot.fingerprint());
    }

    /// Reserved section ids are recognized by the status subsystem.
    #[test]
    fn reserved_section_ids_are_stable() {
        assert_eq!(AgentStatusSectionId::TEMPORAL, "temporal");
        assert_eq!(
            AgentStatusSectionId::BACKGROUND_EXECUTION,
            "background_execution"
        );
        assert!(AgentStatusSectionId::new("temporal").is_reserved());
        assert!(!AgentStatusSectionId::new("custom").is_reserved());
        assert_eq!(
            AgentStatusSectionData::Facts {
                facts: vec![AgentStatusFact {
                    label: "running".to_owned(),
                    value: "1".to_owned(),
                }],
            },
            AgentStatusSectionData::Facts {
                facts: vec![AgentStatusFact {
                    label: "running".to_owned(),
                    value: "1".to_owned(),
                }],
            }
        );
    }
}
