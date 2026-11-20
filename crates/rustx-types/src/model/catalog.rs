use std::fmt;
use std::sync::Arc;

use super::request_params::RequestParamsLayer;
use serde::{Deserialize, Deserializer, Serialize};

/// The identity of one Model Profile declared by a model.
///
/// The runtime assigns no meaning to the name: `fast`, `precise`, `creative`,
/// `thinking-32k`, and `deep` are all just names whose behaviour is exactly
/// the profile's output default and
/// `request_params`.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize)]
#[serde(transparent)]
#[derive(schemars::JsonSchema)]

pub struct ModelProfileId(pub Arc<str>);

impl<'de> Deserialize<'de> for ModelProfileId {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        Self::parse(String::deserialize(deserializer)?).map_err(serde::de::Error::custom)
    }
}

macro_rules! catalog_identity {
    ($name:ident, $what:literal, $reject_slash:literal, $require_non_empty_segments:literal) => {
        impl $name {
            /// Creates the identity without validation.
            ///
            /// Catalog loading always goes through the validating
            /// constructor; this exists for tests and for identities the
            /// runtime already validated.
            #[must_use]
            pub fn new(value: impl Into<String>) -> Self {
                Self(Arc::from(value.into()))
            }

            /// Validates and creates the identity.
            ///
            /// # Errors
            ///
            /// Returns [`crate::model::catalog::ModelCatalogError::InvalidIdentity`] when the value
            /// is empty, contains whitespace, or contains the `/` reference
            /// separator when this identity type reserves it. Model IDs may
            /// use `/`, but every segment must be non-empty.
            pub fn parse(
                value: impl Into<String>,
            ) -> Result<Self, crate::model::catalog::ModelCatalogError> {
                let value = value.into();
                if value.is_empty()
                    || ($reject_slash && value.contains('/'))
                    || ($require_non_empty_segments && value.split('/').any(str::is_empty))
                    || value.chars().any(char::is_whitespace)
                {
                    return Err(crate::model::catalog::ModelCatalogError::InvalidIdentity {
                        kind: $what,
                        value,
                    });
                }
                Ok(Self(Arc::from(value)))
            }

            /// The identity as a string slice.
            #[must_use]
            pub fn as_str(&self) -> &str {
                &self.0
            }
        }

        impl fmt::Display for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str(&self.0)
            }
        }
    };
}

catalog_identity!(ProviderId, "provider", true, false);
catalog_identity!(ModelId, "model", false, true);
catalog_identity!(ModelProfileId, "model profile", true, false);

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ModelCatalogError {
    /// The catalog input is malformed or does not satisfy its typed schema.
    Syntax {
        /// The parser detail.
        detail: String,
    },
    /// The document declares a schema version this runtime does not speak.
    UnsupportedSchemaVersion {
        /// The version this runtime supports.
        supported: u32,
        /// The version found in the document.
        found: u32,
    },
    /// The catalog declares no provider.
    EmptyCatalog,
    /// A provider or model-profile identity is empty, contains whitespace,
    /// or contains `/`. Model identities may contain `/` because the first
    /// slash in a model reference is the provider separator, but their
    /// slash-separated segments must be non-empty.
    InvalidIdentity {
        /// What kind of identity failed.
        kind: &'static str,
        /// The offending value.
        value: String,
    },
    /// A model reference is not of the form `provider-id/model-id`.
    InvalidModelRef {
        /// The offending value.
        value: String,
    },
    /// A provider endpoint is missing or is not an absolute HTTP(S) URL.
    InvalidBaseUrl {
        /// The provider.
        provider: ProviderId,
        /// The offending value.
        base_url: String,
    },
    /// A provider credential source is malformed.
    InvalidCredentialSource {
        /// The provider.
        provider: ProviderId,
        /// The failure detail (never a credential value).
        detail: String,
    },
    /// A provider declares no model.
    ProviderWithoutModels {
        /// The provider.
        provider: ProviderId,
    },
    /// Two models of one provider share an identity.
    DuplicateModel {
        /// The duplicated reference.
        model: ModelRef,
    },
    /// A model declares impossible context/output limits.
    InvalidLimits {
        /// The model.
        model: ModelRef,
        /// The failure detail.
        detail: String,
    },
    /// A model declares an unusable capability set.
    InvalidCapabilities {
        /// The model.
        model: ModelRef,
        /// The failure detail.
        detail: String,
    },
    /// A model declares an invalid Model Profile configuration.
    InvalidProfiles {
        /// The model.
        model: ModelRef,
        /// The failure detail.
        detail: String,
    },
    /// A Model declares an invalid complete reasoning order.
    InvalidReasoningProfileOrder {
        /// The Model.
        model: ModelRef,
        /// The authorized diagnostic detail, which may quote identities.
        detail: String,
    },
    /// A model declares bounded compatibility fields for a foreign protocol.
    InvalidCompat {
        /// The model.
        model: ModelRef,
        /// The validation detail.
        detail: String,
    },
    /// A configured request-parameter layer collides with a runtime-owned
    /// protected wire key.
    ProtectedKey {
        /// The model.
        model: ModelRef,
        /// The colliding key.
        key: String,
        /// Which configuration layer declared it.
        layer: RequestParamsLayer,
    },
    /// A configured request-parameter layer carries a number that would not
    /// survive a binary64 round trip.
    InexactNumber {
        /// The model.
        model: ModelRef,
        /// Which configuration layer declared it.
        layer: RequestParamsLayer,
    },
    /// A referenced provider does not exist.
    UnknownProvider {
        /// The referenced provider.
        provider: ProviderId,
    },
    /// A referenced model does not exist.
    UnknownModel {
        /// The referenced model.
        model: ModelRef,
    },
    /// A `$ENV_VAR` credential is absent or empty in the process
    /// environment.
    MissingEnvironmentCredential {
        /// The provider whose credential is unresolved.
        provider: ProviderId,
        /// The environment variable name (never a value).
        variable: String,
    },
}
/// The identity of one catalog provider.
///
/// A provider identity is an opaque local name. It never implies an
/// endpoint, a protocol, a credential store, or a vendor.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(transparent)]
#[derive(schemars::JsonSchema)]
pub struct ProviderId(Arc<str>);
/// The identity of one catalog model within its provider.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(transparent)]
#[derive(schemars::JsonSchema)]
pub struct ModelId(Arc<str>);
/// A fully qualified `provider/model` catalog reference.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, schemars::JsonSchema)]
#[schemars(with = "String")]
pub struct ModelRef(pub Arc<str>);

impl fmt::Display for ModelCatalogError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Syntax { detail } => write!(f, "malformed model catalog: {detail}"),
            Self::UnsupportedSchemaVersion { supported, found } => write!(
                f,
                "unsupported model catalog schema_version {found}; this runtime speaks {supported}"
            ),
            Self::EmptyCatalog => f.write_str("the model catalog declares no provider"),
            Self::InvalidIdentity { kind, value } => {
                write!(f, "invalid {kind} identity {value:?}")
            }
            Self::InvalidModelRef { value } => write!(
                f,
                "invalid model reference {value:?}; expected \"provider-id/model-id\""
            ),
            Self::InvalidBaseUrl { provider, base_url } => write!(
                f,
                "provider {provider} declares an invalid base_url {base_url:?}; \
                 an explicit absolute http(s) endpoint is mandatory"
            ),
            Self::InvalidCredentialSource { provider, detail } => {
                write!(f, "provider {provider} api_key source is invalid: {detail}")
            }
            Self::ProviderWithoutModels { provider } => {
                write!(f, "provider {provider} declares no model")
            }
            Self::DuplicateModel { model } => write!(f, "duplicate model identity {model}"),
            Self::InvalidLimits { model, detail } => {
                write!(f, "model {model} declares invalid limits: {detail}")
            }
            Self::InvalidCapabilities { model, detail } => {
                write!(f, "model {model} declares invalid capabilities: {detail}")
            }
            Self::InvalidProfiles { model, detail } => {
                write!(f, "model {model} declares invalid profiles: {detail}")
            }
            Self::InvalidReasoningProfileOrder { model, detail } => {
                write!(
                    f,
                    "model {model} declares invalid reasoning_profile_order: {detail}"
                )
            }
            Self::InvalidCompat { model, detail } => {
                write!(f, "model {model} declares invalid compat: {detail}")
            }
            Self::ProtectedKey { model, key, layer } => write!(
                f,
                "model {model} {layer} (request_params) declares runtime-owned protected wire key {key:?}"
            ),
            Self::InexactNumber { model, layer } => write!(
                f,
                "model {model} {layer} (request_params) carry a JSON number that is not exactly \
                 representable as an IEEE 754 binary64 value"
            ),
            Self::UnknownProvider { provider } => {
                write!(f, "unknown catalog provider {provider}")
            }
            Self::UnknownModel { model } => write!(f, "unknown catalog model {model}"),
            Self::MissingEnvironmentCredential { provider, variable } => write!(
                f,
                "provider {provider} credential environment variable {variable} is not set"
            ),
        }
    }
}

impl std::error::Error for ModelCatalogError {}

impl ModelRef {
    /// Borrow the validated native identity without formatting an owned String.
    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// Constructs an explicitly qualified name; qualification is naming only.
    #[must_use]
    #[allow(clippy::needless_pass_by_value)] // Own both explicit naming components at construction.
    pub fn new(provider: ProviderId, model: ModelId) -> Self {
        Self(Arc::from(format!("{provider}/{model}")))
    }

    /// Parses a nonempty catalog identity, independently of Provider bindings.
    /// # Errors
    /// Rejects invalid identity segments.
    pub fn parse(value: &str) -> Result<Self, ModelCatalogError> {
        ModelId::parse(value).map_err(|_| ModelCatalogError::InvalidModelRef {
            value: value.to_owned(),
        })?;
        Ok(Self(Arc::from(value)))
    }
}
impl fmt::Display for ModelRef {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl Serialize for ModelRef {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(&self.0)
    }
}

impl<'de> Deserialize<'de> for ModelRef {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let value = String::deserialize(deserializer)?;
        Self::parse(&value).map_err(serde::de::Error::custom)
    }
}

impl ModelCatalogError {
    /// The authored location and fixed category of this failure.
    ///
    /// Both are native constants: an identity appears only as `*`, and no
    /// authored value, identity or detail is ever part of either, so they can
    /// be shown where the [`fmt::Display`] text (which quotes values) cannot.
    #[must_use]
    pub fn classification(&self) -> (&'static str, &'static str) {
        match self {
            Self::Syntax { .. } => ("models", "malformed model catalog"),
            Self::UnsupportedSchemaVersion { .. } => {
                ("models", "unsupported model catalog schema version")
            }
            Self::EmptyCatalog => (
                "providers",
                "the composed model catalog declares no Provider or no Model",
            ),
            Self::InvalidIdentity { kind, .. } => match *kind {
                "provider" => ("providers", "invalid Provider identity"),
                "model profile" => ("models.*.profiles", "invalid Model Profile identity"),
                _ => ("models", "invalid Model identity"),
            },
            Self::InvalidModelRef { .. } => (
                "models",
                "invalid Model reference; expected provider-id/model-id",
            ),
            Self::InvalidBaseUrl { .. } => (
                "providers.*.base_url",
                "invalid Provider endpoint; an absolute http(s) URL without credentials, query or fragment is required",
            ),
            Self::InvalidCredentialSource { .. } => {
                ("providers.*.api_key", "invalid Provider credential source")
            }
            Self::ProviderWithoutModels { .. } => ("providers.*", "a Provider declares no Model"),
            Self::DuplicateModel { .. } => ("models.*", "duplicate Model identity"),
            Self::InvalidLimits { .. } => ("models.*", "invalid Model context or output limits"),
            Self::InvalidCapabilities { .. } => {
                ("models.*.capabilities", "unusable Model capabilities")
            }
            Self::InvalidProfiles { .. } => ("models.*.profiles", "invalid Model Profiles"),
            Self::InvalidReasoningProfileOrder { .. } => (
                "models.*.reasoning_profile_order",
                "invalid Model reasoning Profile order",
            ),
            Self::InvalidCompat { .. } => ("models.*.compat", "invalid Model compatibility fields"),
            Self::ProtectedKey { .. } => (
                "models.*.request_params",
                "request parameters set a runtime-owned key",
            ),
            Self::InexactNumber { .. } => (
                "models.*.request_params",
                "request parameters carry a number without an exact binary64 value",
            ),
            Self::UnknownProvider { .. } => (
                "models.*.provider",
                "a Model names a Provider the composed catalog does not define",
            ),
            Self::UnknownModel { .. } => (
                "agent.model",
                "a selected Model is not defined by the composed catalog",
            ),
            Self::MissingEnvironmentCredential { .. } => (
                "providers.*.api_key",
                "the credential environment variable is unset",
            ),
        }
    }
}
