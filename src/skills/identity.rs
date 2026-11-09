//! Stable normalized frontmatter identity. Host paths and runtime resource bytes
//! are independent binding/file facts, never inputs to the definition digest.
use crate::runtime::identity::SkillVersionId;
use sha2::{Digest, Sha256};

pub(crate) fn definition_version_id(definition: &impl serde::Serialize) -> SkillVersionId {
    let mut hash = Sha256::new();
    hash.update(b"rustx-skill-definition-v1\n");
    hash.update(serde_json::to_vec(definition).expect("normalized Skill definition serializes"));
    SkillVersionId::new(format!("sha256:{:x}", hash.finalize()))
}
