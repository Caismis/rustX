//! A published catalog can be reused only after checking the current file bytes.
use std::{fs, path::Path};

use serde::Deserialize;

use super::{
    CatalogDocument, SESSION_CATALOG_SCHEMA_VERSION, SessionCatalog, SessionError,
    validate_document,
};

#[derive(Debug)]
pub(super) struct PublishedCatalog {
    pub generation: u64,
    pub bytes: Vec<u8>,
}

pub(super) fn decode(bytes: &[u8], path: &Path) -> Result<CatalogDocument, SessionError> {
    // Skip unrelated values instead of constructing a second catalog-sized DOM.
    #[derive(Deserialize)]
    struct Envelope {
        schema_version: Option<serde_json::Value>,
    }
    let envelope: Envelope =
        serde_json::from_slice(bytes).map_err(|error| SessionError::Catalog {
            detail: error.to_string(),
        })?;
    if envelope
        .schema_version
        .as_ref()
        .and_then(serde_json::Value::as_u64)
        != Some(u64::from(SESSION_CATALOG_SCHEMA_VERSION))
    {
        return Err(SessionError::Catalog { detail: "unsupported session catalog schema; CFG3 requires UUIDv7 identities and Session-owned intent; old development schemas are refused".into() });
    }
    let document = serde_json::from_slice(bytes).map_err(|error| SessionError::Catalog {
        detail: format!("cannot decode {}: {error}", path.display()),
    })?;
    validate_document(&document)?;
    Ok(document)
}

impl SessionCatalog {
    pub(super) fn stored_generation(&self) -> Result<Option<u64>, SessionError> {
        let path = self
            .product
            .confined(&self.path)
            .map_err(|error| SessionError::Io {
                path: self.root.clone(),
                detail: error.to_string(),
            })?;
        if !path.exists() {
            return Ok(None);
        }
        let bytes = fs::read(&path).map_err(|error| SessionError::Io {
            path: path.clone(),
            detail: error.to_string(),
        })?;
        if let Some(published) = &self.published
            && published.bytes == bytes
        {
            return Ok(Some(published.generation));
        }
        decode(&bytes, &path).map(|document| Some(document.generation))
    }
}
