//! Native semantic constraints of authored sources and their composition,
//! and the authored document schema.
use super::*;

/// A native semantic constraint an authored source or composition fails.
///
/// `field` (identities only as `*`) and `reason` are native constants, safe to
/// show anywhere, including automatic repair receipts. `detail` is the native
/// domain message, which may quote authored values; it stays inside launch
/// diagnostics.
#[derive(Debug, Clone)]
pub(in crate::local_runtime) struct SemanticFailure {
    pub(crate) field: &'static str,
    pub(crate) reason: &'static str,
    detail: String,
}

impl SemanticFailure {
    pub(in crate::local_runtime) fn new(
        field: &'static str,
        reason: &'static str,
        detail: String,
    ) -> Self {
        Self {
            field,
            reason,
            detail,
        }
    }

    pub(in crate::local_runtime) fn catalog(
        error: &crate::model::catalog::ModelCatalogError,
    ) -> Self {
        let (field, reason) = error.classification();
        Self::new(field, reason, error.to_string())
    }

    /// The safe one-line summary: category and location, never values.
    pub(crate) fn summary(&self) -> String {
        format!("{} at {}", self.reason, self.field)
    }

    /// The one document that authored the failing field, from the overlay
    /// provenance of the composition: every authored unit containing the
    /// field, or contained in it, must come from the same User or Workspace
    /// document. A field spanning both documents, or one no document
    /// authored, has no owner: its failure belongs to the composition.
    pub(in crate::local_runtime) fn owner(
        &self,
        provenance: &BTreeMap<String, Origin>,
    ) -> Option<PathBuf> {
        /// Whether one path is a prefix of the other; a `*` in the field
        /// stands for one identity, which may itself contain dots.
        fn overlaps(field: &[&str], unit: &[&str]) -> bool {
            match (field.split_first(), unit.split_first()) {
                (None, _) | (_, None) => true,
                (Some((&"*", field)), Some((_, _))) => {
                    (1..=unit.len()).any(|taken| overlaps(field, &unit[taken..]))
                }
                (Some((segment, field)), Some((name, unit))) => {
                    segment == name && overlaps(field, unit)
                }
            }
        }
        let field: Vec<_> = self.field.split('.').collect();
        let mut owners = provenance.iter().filter_map(|(unit, origin)| {
            let unit: Vec<_> = unit.split('.').collect();
            overlaps(&field, &unit).then_some(origin)
        });
        let first = owners.next()?;
        let document = match first {
            Origin::User { document, .. } | Origin::Workspace { document, .. } => document,
            Origin::Builtin | Origin::Process { .. } => return None,
        };
        owners
            .all(|origin| origin == first)
            .then(|| document.clone())
    }

    /// A launch failure at the authoring document, or at the composition
    /// (no file) when no single document authored the field.
    pub(in crate::local_runtime) fn launch(self, document: Option<PathBuf>) -> LaunchFailure {
        LaunchFailure::at(
            document,
            self.field,
            self.reason,
            "correct this value in User or Workspace rustx.toml",
            self.detail,
        )
    }
}

impl From<super::super::config::ConstraintFailure> for SemanticFailure {
    fn from(failure: super::super::config::ConstraintFailure) -> Self {
        Self::new(failure.field, failure.reason, failure.error.to_string())
    }
}

/// How complete a composition that satisfies every decidable constraint is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Composition {
    /// A default model is selected and every model-dependent constraint holds.
    Complete,
    /// No default model is selected: the composition is incomplete, not
    /// invalid, because a Session request may still select one.
    NoDefaultModel,
}

pub(in crate::local_runtime) fn catalog_document(
    layer: &RuntimeLayer,
) -> crate::model::catalog::ModelCatalogDocument {
    crate::model::authoring::Catalog {
        schema_version: crate::model::catalog::MODEL_CATALOG_SCHEMA_VERSION,
        providers: layer.providers.clone().unwrap_or_default(),
        models: layer.models.clone().unwrap_or_default(),
    }
    .into()
}

/// Source-intrinsic native semantics: every constraint an authored document
/// decides on its own (Issue #464).
///
/// A source may leave facts to the other scope it is composed with — the
/// default model, the Provider of a Model, the rest of the Model Catalog — but
/// everything else it authors is decided here, whether or not a default model
/// is selected: App Server process policy, each Provider and Model entry, and
/// every [`CurrentRuntimeConfig::validate_authored`] constraint. `.agents`
/// resources, credentials and preparation are never consulted.
pub(in crate::local_runtime) fn source_semantics(
    layer: &RuntimeLayer,
) -> Result<(), SemanticFailure> {
    if let Some(policy) = &layer.app_server {
        policy.validate().map_err(|detail| {
            SemanticFailure::new("app_server", "invalid App Server process policy", detail)
        })?;
    }
    ModelCatalog::validate_entries(catalog_document(layer))
        .map_err(|error| SemanticFailure::catalog(&error))?;
    let config = layer
        .clone()
        .resolve()
        .map_err(|detail| SemanticFailure::new("agent.model", "invalid model selection", detail))?;
    config.validate_authored()?;
    Ok(())
}

/// Native semantics of one complete User + Workspace composition: its
/// source-intrinsic constraints and the references between its scopes — Model
/// Catalog closure and, once a default model is selected, the selected models
/// and the context budgets they bound. Session resolution applies the same
/// validators to the same composition.
pub(in crate::local_runtime) fn composition_semantics(
    merged: &RuntimeLayer,
) -> Result<Composition, SemanticFailure> {
    source_semantics(merged)?;
    let catalog = ModelCatalog::from_document(catalog_document(merged));
    let config = merged
        .clone()
        .resolve()
        .map_err(|detail| SemanticFailure::new("agent.model", "invalid model selection", detail))?;
    let Some(model) = &config.agent.model else {
        return match catalog {
            Ok(_) | Err(crate::model::catalog::ModelCatalogError::EmptyCatalog) => {
                Ok(Composition::NoDefaultModel)
            }
            Err(error) => Err(SemanticFailure::catalog(&error)),
        };
    };
    let models = catalog.map_err(|error| SemanticFailure::catalog(&error))?;
    let (primary, summary) = crate::model::session::analyze_session_model_config(&models, model)
        .map_err(|error| {
            let (field, reason) = error.classification();
            SemanticFailure::new(field, reason, error.to_string())
        })?;
    let summary = summary.as_ref().unwrap_or(&primary);
    config
        .context_policy()
        .validate_budgets(
            (primary.context_window, primary.max_output_tokens),
            (summary.context_window, summary.max_output_tokens),
        )
        .map_err(|error| {
            SemanticFailure::new(
                "context",
                "context budgets cannot fit the selected models",
                error.message,
            )
        })?;
    Ok(Composition::Complete)
}

pub(in crate::local_runtime) fn authoring_schema() -> Value {
    serde_json::to_value(crate::toml_authoring::source_schema::<RuntimeLayer>())
        .expect("schema serializes")
}
