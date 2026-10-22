//! Source capture: rereading the User and Workspace documents, overlaying
//! them, resolving paths and resources, and producing one immutable
//! [`ProspectiveSessionConfig`] or captured application input.
#[cfg(test)]
use super::ConfigurationHooks;
use super::semantics::{SemanticFailure, catalog_document, composition_semantics};
use super::{
    AdmittedSource, BTreeMap, Digest, IndependentPolicy, LaunchFailure, LayerCapture, ModelCatalog,
    Origin, OsStrExt, Path, PathBuf, ProspectiveSessionConfig, ResourceDefinition, ResourceFamily,
    RuntimeLayer, SessionConfigInput, SessionLocations, Sha256, SourceCapture, SourceReread,
    SourceScope, UserConfigManager, UserConfigSources, application, read_bounded, repair,
};

/// Canonical authoring identity determines the base of relative user bindings.
pub(in crate::local_runtime) fn canonical_settings_source(path: &Path) -> Result<PathBuf, String> {
    if !path.is_absolute() {
        return Err("user settings source must be absolute".into());
    }
    let canonical = normalize_missing(path)?;
    // Writer metadata lives beside its document, under names rustX reserves:
    // a source with such a name would share its path with that metadata.
    if canonical
        .file_name()
        .is_some_and(crate::local_runtime::settings::is_writer_artifact)
    {
        return Err(format!(
            "user settings source `{}` uses a file name rustX reserves for writer metadata (`.rustx-locks`, `.rustx-stage-*`); rename the file",
            canonical.display()
        ));
    }
    Ok(canonical)
}

impl UserConfigManager {
    /// Bind absolute user roots without reading configuration or credentials.
    /// # Errors
    /// Relative source paths are rejected rather than interpreted using process cwd.
    pub fn new(mut sources: UserConfigSources) -> Result<Self, String> {
        for path in [
            &sources.home_directory,
            &sources.config_path,
            &sources.runtime_root,
        ] {
            if !path.is_absolute() {
                return Err("user source bindings must be absolute".into());
            }
        }
        sources.home_directory = normalize_missing(&sources.home_directory)?;
        sources.config_path = canonical_settings_source(&sources.config_path)?;
        sources.runtime_root = normalize_missing(&sources.runtime_root)?;
        let explicit = Origin::Process {
            base: sources.home_directory.join("rustx"),
        };
        Ok(Self {
            sources,
            runtime_root_origin: explicit,
            repairs: repair::RepairLedger::default(),
            #[cfg(test)]
            test_hooks: ConfigurationHooks::default(),
        })
    }

    /// Bootstrap process bindings once using the canonical user document and
    /// optional host overrides. Supplied sources provide the default locations;
    /// no Session cwd participates in this operation.
    /// # Errors
    /// Rejects invalid user authoring and nonabsolute source bindings.
    pub fn bootstrap(
        sources: UserConfigSources,
        runtime_root: Option<PathBuf>,
    ) -> Result<Self, LaunchFailure> {
        let mut sources = sources;
        sources.config_path = canonical_settings_source(&sources.config_path)?;
        if let Some(root) = runtime_root {
            if !root.is_absolute() {
                return Err("runtime root must be absolute".into());
            }
            sources.runtime_root = normalize_missing(&root)?;
        }
        let manager = Self::new(sources)?;
        Ok(manager)
    }
    /// Canonical native process bindings, without a wire projection.
    pub(crate) fn source_bindings(&self) -> &UserConfigSources {
        &self.sources
    }

    /// User-scoped product root, independent of any Session or process cwd.
    #[must_use]
    pub fn runtime_root(&self) -> &Path {
        &self.sources.runtime_root
    }

    /// Read process policy from the canonical user document only.
    /// # Errors
    /// Invalid user authoring or policy is rejected before traffic admission.
    pub fn app_server_policy(
        &self,
    ) -> Result<super::super::app_server_policy::AppServerPolicy, String> {
        let path = &self.sources.config_path;
        let bytes = reread(&admit_source(path)?).map_err(|error| error.to_string())?;
        let policy = layer_of(path, bytes.as_deref(), false)
            .map_err(|error| error.to_string())?
            .app_server
            .unwrap_or_default();
        policy.validate()?;
        Ok(policy)
    }

    /// Resolve only locations; no configuration, credentials, or runtime creation.
    /// # Errors
    /// Invalid explicit paths and Tool restrictions are rejected.
    pub fn resolve_locations(
        &self,
        request: &SessionConfigInput,
    ) -> Result<(SessionLocations, String), String> {
        if !request.cwd.is_absolute() {
            return Err("Session cwd must be absolute".into());
        }
        let workspace = canonical_directory(&request.cwd)?;
        let identity = workspace_identity(&workspace);
        let runtime_root = self.sources.runtime_root.clone();
        if normalize_missing(&runtime_root)? != runtime_root {
            return Err("bound runtime_root physical authority changed".into());
        }
        Ok((
            SessionLocations {
                workspace,
                runtime_root,
            },
            identity,
        ))
    }
}

#[allow(
    clippy::too_many_lines,
    clippy::missing_panics_doc,
    clippy::unnecessary_debug_formatting
)] // derived paths always have parents; debug escapes diagnostic paths
impl UserConfigManager {
    /// Capture policies independently of fallible context/resource preparation.
    /// Both consume the same immutable source overlay. Policy publication never
    /// borrows a second read merely because another closure fails.
    pub(crate) fn capture_application(
        &self,
        request: &SessionConfigInput,
    ) -> Result<application::CapturedApplication, String> {
        self.capture_application_at_boundary(request, || {})
    }

    /// A Session scope's capture: its source generation, resolved and
    /// validated with the source default, carrying the Session's own
    /// selection unvalidated. Whether the new catalog still admits that
    /// selection is the Session's fact to report and correct, never a reason
    /// to fail every unit of a valid source generation.
    pub(crate) fn capture_session_application(
        &self,
        input: &SessionConfigInput,
    ) -> Result<application::CapturedApplication, String> {
        let mut captured = self.capture_application(&SessionConfigInput::new(input.cwd.clone()))?;
        if let Ok(context) = &mut captured.context {
            context.input.model.clone_from(&input.model);
        }
        Ok(captured)
    }

    pub(in crate::local_runtime) fn capture_application_at_boundary(
        &self,
        request: &SessionConfigInput,
        after_layers: impl FnOnce(),
    ) -> Result<application::CapturedApplication, String> {
        let (layers, documents) = self
            .capture_layers(request)
            .map_err(|error| error.to_string())?;
        after_layers();
        let policy = layers
            .merged
            .clone()
            .resolve()
            .map_err(|error| error.clone())?;
        policy.timeout_policy().map_err(|error| error.to_string())?;
        policy
            .tool_deadline_policy()
            .map_err(|error| error.to_string())?;
        let revisions = layers.revisions.clone();
        let mut policy_origins = layers.provenance.clone();
        RuntimeLayer::record_default_origins(&policy, &mut policy_origins);
        let policy_effective = layers.merged.clone();
        let process = layers.merged.app_server.clone().unwrap_or_default();
        process.validate()?;
        let context = self
            .resolve_captured_layers(layers)
            .and_then(Self::resolve_runtime_configuration)
            .and_then(|capture| self.resolve_resources(request, capture))
            .map_err(|error| error.diagnostic.reason);
        self.verify_documents(&documents, &revisions)
            .map_err(|error| error.to_string())?;
        let manifest = context
            .as_ref()
            .map_or(&revisions, |capture| &capture.source_revisions);
        let revision = source_manifest_revision(manifest);
        let policy = IndependentPolicy {
            config: policy,
            effective: policy_effective,
            provenance: policy_origins,
            revision: revision.clone(),
        };
        Ok(application::CapturedApplication {
            policy,
            process,
            revision,
            context,
        })
    }

    /// Resolve current canonical sources for exactly this Session context.
    /// No credentials or external preparation are acquired here.
    ///
    /// # Errors
    /// Rejects invalid paths, authoring, authority, and semantic configuration.
    pub fn resolve_session(
        &self,
        request: &SessionConfigInput,
    ) -> Result<ProspectiveSessionConfig, LaunchFailure> {
        let (layers, documents) = self.capture_layers(request)?;
        let resolved = self.resolve_model_candidate(request, layers)?;
        let resolved = Self::resolve_runtime_configuration(resolved)?;
        let prospective = self.resolve_resources(request, resolved)?;
        self.verify_documents(&documents, &prospective.source_revisions)?;
        Ok(prospective)
    }

    fn resolve_model_candidate(
        &self,
        request: &SessionConfigInput,
        layers: LayerCapture,
    ) -> Result<SourceCapture, LaunchFailure> {
        let capture = self.resolve_captured_layers(layers)?;
        let config = &capture.config;
        config.context.validate().map_err(|e| e.to_string())?;
        let (primary, summary) = crate::model::session::analyze_session_model_config(
            &capture.models,
            request
                .model
                .as_ref()
                .unwrap_or_else(|| config.initial_model()),
        )
        .map_err(|e| e.to_string())?;
        let summary = summary.as_ref().unwrap_or(&primary);
        config
            .context_policy()
            .validate_budgets(
                (primary.context_window, primary.max_output_tokens),
                (summary.context_window, summary.max_output_tokens),
            )
            .map_err(|error| {
                LaunchFailure::at(
                    Some(self.sources.config_path.clone()),
                    "context",
                    "context budgets cannot fit the selected models",
                    "correct reserves, recent-history budget, or explicit model limits",
                    error.message,
                )
            })?;
        Ok(capture)
    }

    // Execution-domain validation belongs only to full Session resolution.
    fn resolve_runtime_configuration(
        capture: SourceCapture,
    ) -> Result<SourceCapture, LaunchFailure> {
        capture.config.validate().map_err(|e| e.to_string())?;
        Ok(capture)
    }

    // One strict source parse, authority, overlay and provenance owner. Lowering
    // is structural; semantic validation belongs to the consuming domain. Both
    // documents are read through their admitted directories, which stay
    // admitted for the capture's revision fence ([`Self::verify_documents`]).
    fn capture_layers(
        &self,
        request: &SessionConfigInput,
    ) -> Result<(LayerCapture, [AdmittedSource; 2]), LaunchFailure> {
        let host = &self.sources;
        let (locations, identity) = self.resolve_locations(request)?;
        let user_path = host.config_path.clone();
        let project_path = locations.workspace.join("rustx.toml");
        let documents = [admit_source(&user_path)?, admit_source(&project_path)?];
        let read = |source: &AdmittedSource, workspace| {
            let bytes = source
                .read()
                .map_err(|error| unavailable(source.path(), client_detail(error)))?;
            let layer = layer_of(source.path(), bytes.as_deref(), workspace)?;
            Ok::<_, LaunchFailure>((layer, super::super::settings::revision(bytes.as_deref())))
        };
        let (user, user_revision) = read(&documents[0], false)?;

        // Both ordinary documents may be absent. Typed resolution determines
        // whether their composed configuration is complete.
        let (project, workspace_revision) = read(&documents[1], true)?;
        let revisions = BTreeMap::from([
            (user_path.clone(), user_revision),
            (project_path.clone(), workspace_revision),
        ]);
        // Binding authoring remains strictly validated, but cannot rebind this manager.

        storage_disjoint(&locations.runtime_root, &locations.workspace)?;

        let mut merged = RuntimeLayer {
            app_server: user.app_server.clone(),
            ..RuntimeLayer::default()
        };
        let mut provenance = BTreeMap::new();
        let mut project_resources = Vec::new();
        for (mut layer, origin) in [
            (
                user,
                Origin::User {
                    document: user_path.clone(),
                    base: user_path.parent().expect("parent").into(),
                },
            ),
            (
                project,
                Origin::Workspace {
                    document: project_path.clone(),
                    base: project_path.parent().expect("parent").into(),
                },
            ),
        ] {
            project_resources.extend(rebase_paths(
                &mut layer,
                &origin,
                &locations.workspace,
                false,
            )?);
            merged.overlay(layer, &origin, &mut provenance);
        }
        Ok((
            LayerCapture {
                revisions,
                locations,
                identity,
                merged,
                provenance,
                project_resources,
            },
            documents,
        ))
    }

    /// The revision fence of a capture's two documents, after every other
    /// input was read: each is reread through its admitted directory, which
    /// the pathname must still name ([`AdmittedSource::reread`]), and must
    /// still have its captured revision. A directory replaced or redirected
    /// since admission is refused, never answered from the replacement, even
    /// with identical bytes.
    #[cfg_attr(
        not(test),
        allow(
            clippy::unused_self,
            reason = "the manager reaches its test hook at the verification boundary"
        )
    )]
    fn verify_documents(
        &self,
        documents: &[AdmittedSource; 2],
        revisions: &BTreeMap<PathBuf, String>,
    ) -> Result<(), LaunchFailure> {
        #[cfg(test)]
        self.test_hooks.reach("before_capture_verification");
        for source in documents {
            let actual = super::super::settings::revision(reread(source)?.as_deref());
            if revisions.get(source.path()) != Some(&actual) {
                return Err("configuration changed during capture; rescan to retry".into());
            }
        }
        Ok(())
    }

    fn resolve_captured_layers(
        &self,
        capture: LayerCapture,
    ) -> Result<SourceCapture, LaunchFailure> {
        let LayerCapture {
            mut revisions,
            locations,
            identity,
            merged,
            mut provenance,
            project_resources,
        } = capture;
        for root in [
            self.sources.home_directory.join("rustx/.agents"),
            locations.workspace.join(".agents"),
        ] {
            revisions.insert(
                root.clone(),
                super::super::resource_directory::revision(&root),
            );
        }
        let user_path = self.sources.config_path.clone();
        let model_result = ModelCatalog::from_document(catalog_document(&merged));
        // A semantic failure names the document that authored its field.
        let owner = |failure: &SemanticFailure| {
            // Process policy stays outside the overlay; its source contract
            // admits it in the bound User document only.
            if failure.field == "app_server" {
                Some(user_path.clone())
            } else {
                failure.owner(&provenance)
            }
        };
        let model_failure = |e: crate::model::catalog::ModelCatalogError| {
            LaunchFailure::at(
                owner(&SemanticFailure::catalog(&e)),
                "models",
                "invalid model catalog semantics",
                "define Providers and Models in rustx.toml",
                e.to_string(),
            )
        };
        if merged
            .agent
            .as_ref()
            .and_then(|agent| agent.model.as_ref())
            .is_none_or(|model| model.model.is_none())
        {
            // Incomplete only when everything the composition decides holds;
            // an independently invalid value is invalid with or without a
            // default model.
            composition_semantics(&merged).map_err(|failure| {
                let document = owner(&failure);
                failure.launch(document)
            })?;
            let mut error = LaunchFailure::at(
                Some(user_path.clone()),
                "agent.model.model",
                "no unambiguous default model selected",
                "set agent.model.model in User or Workspace rustx.toml",
                "no unambiguous default model selected".into(),
            );
            error.incomplete = true;
            error.partial = Some(Box::new(super::super::diagnostics::PartialProjection::new(
                &locations,
                serde_json::to_value(&merged).map_err(|e| e.to_string())?,
            )));
            return Err(error);
        }
        let models = model_result.map_err(model_failure)?;
        let effective = merged.clone();
        let mut config = merged.resolve()?;
        let mcp = super::super::mcp_resources::load(
            &self.sources.home_directory.join("rustx/.agents"),
            Some(&locations.workspace),
        );
        let mut diagnostics: Vec<_> = mcp
            .invalid_scopes
            .iter()
            .map(|error| {
                crate::runtime::capability_inspection::ResourceDiagnostic::collection(
                    crate::runtime::capability_inspection::ResourceFamily::Mcp,
                    error,
                )
            })
            .collect();
        let definitions = mcp
            .locations
            .iter()
            .map(
                |(id, location)| crate::runtime::capability_inspection::ResourceDefinition {
                    family: crate::runtime::capability_inspection::ResourceFamily::Mcp,
                    name: id.to_string(),
                    location: location.clone(),
                    valid: mcp.definitions.get(id).is_some_and(Result::is_ok),
                },
            )
            .collect();
        for (source, selection) in &config.agent.tools.sources {
            if let crate::capabilities::ToolSourceId::Mcp(id) = source {
                let demanded = match selection {
                    crate::capabilities::selection::SourceToolSelection::All => true,
                    crate::capabilities::selection::SourceToolSelection::Exact(names) => {
                        !names.is_empty()
                    }
                };
                if demanded
                    && !mcp.definitions.contains_key(id)
                    && let Some(error) = mcp.invalid_scopes.last()
                {
                    return Err(LaunchFailure::resource(error.clone()));
                }
            }
        }
        revisions.extend(mcp.revisions);
        for (id, definition) in mcp.definitions {
            match definition {
                Ok(definition) => {
                    config.mcp_servers.insert(id.clone(), definition.resolve());
                }
                Err(error) => {
                    diagnostics.push(
                        crate::runtime::capability_inspection::ResourceDiagnostic::resource(
                            crate::runtime::capability_inspection::ResourceFamily::Mcp,
                            &id,
                            &error,
                        ),
                    );
                    if config
                        .agent
                        .tools
                        .sources
                        .get(&crate::capabilities::ToolSourceId::Mcp(id.clone()))
                        .is_some_and(|selection| match selection {
                            crate::capabilities::selection::SourceToolSelection::All => true,
                            crate::capabilities::selection::SourceToolSelection::Exact(names) => {
                                !names.is_empty()
                            }
                        })
                    {
                        return Err(LaunchFailure::resource(error));
                    }
                }
            }
        }
        for (id, origin) in mcp.origins {
            provenance.insert(format!("mcp_servers.{id}"), origin);
        }

        RuntimeLayer::record_default_origins(&config, &mut provenance);
        provenance.insert("runtime_root".into(), self.runtime_root_origin.clone());
        Ok(SourceCapture {
            definitions,
            diagnostics,
            revisions,
            effective,
            locations,
            identity,
            config,
            models,
            provenance,
            project_resources,
        })
    }

    fn resolve_resources(
        &self,
        request: &SessionConfigInput,
        resolved: SourceCapture,
    ) -> Result<ProspectiveSessionConfig, LaunchFailure> {
        let SourceCapture {
            definitions: mut resource_definitions,
            mut diagnostics,
            mut revisions,
            effective,
            locations,
            identity,
            config,
            models,
            provenance,
            project_resources,
        } = resolved;
        let host = &self.sources;
        for path in &project_resources {
            crate::runtime::resources::validate_project_resource_path(&locations.workspace, path)
                .map_err(LaunchFailure::resource)?;
        }
        // Both resource roots are fixed process/Workspace bindings; profiles select visibility.
        let skill_sources = crate::skills::automatic_skill_roots(
            Some(host.home_directory.as_path()),
            &locations.workspace,
        );
        // Capture authority once, including host path aliases and missing leaves.
        // Configuration preparation retains this physical identity; validators must not rebind it.
        let agent_root = normalize_missing(&host.home_directory.join("rustx/.agents/agents"))?;
        let project_context_files = {
            crate::runtime::load_project_context_files(&locations.workspace)
                .map_err(LaunchFailure::resource)?
        };
        let (subagents, role_sources) =
            super::super::agent_resources::load_authorized(Some(&locations.workspace), &agent_root)
                .map_err(LaunchFailure::resource)?;
        revisions.extend(
            role_sources
                .values()
                .map(|source| (source.selected.clone(), source.revision.clone())),
        );
        for source in role_sources.values() {
            if let (Some(path), Some(revision)) = (&source.overridden, &source.overridden_revision)
            {
                revisions.insert(path.clone(), revision.clone());
            }
        }
        let mut workflows = {
            super::super::workflow_resources::load(
                Some(&locations.workspace),
                &host.home_directory.join("rustx/.agents"),
            )
            .map_err(LaunchFailure::resource)?
        };
        for id in &config.agent.workflows {
            if let Some(error) = workflows.invalid().get(id) {
                return Err(LaunchFailure::resource(error.clone()));
            }
        }
        let workspace = crate::tools::workspace::Workspace::new(&locations.workspace)
            .map_err(|e| e.to_string())?;
        let skill_discovery = {
            crate::skills::SkillDiscovery::with_config(
                &workspace,
                crate::skills::SkillDiscoveryConfig {
                    automatic: skill_sources.clone(),
                },
            )
            .discover()
        };
        if let Some(crate::runtime::agent_profile::AgentSkillSelection::Exact(names)) =
            &config.agent.skills
        {
            for name in names {
                if !skill_discovery
                    .packages
                    .iter()
                    .any(|package| package.name() == name && !package.disable_model_invocation())
                {
                    return Err(LaunchFailure::at(
                        None,
                        "agent.skills",
                        "selected Skill is unavailable",
                        "select a valid visible Skill package",
                        name.clone(),
                    ));
                }
            }
        }
        let skills = crate::skills::SkillSnapshot::from_discovery(skill_discovery.clone());
        for name in &config.agent.agents {
            if let Some(error) = subagents.invalid().get(name) {
                return Err(LaunchFailure::resource(error.clone()));
            }
        }
        let managed_python = super::super::managed_python_resources::discover(
            Some(&locations.workspace),
            &host.home_directory.join("rustx/.agents"),
        )
        .map_err(LaunchFailure::resource)?;
        let MetadataAdmission {
            definitions,
            catalog: available_catalog,
            availability,
            native_leaves,
        } = MetadataAdmission::compose(&config, &subagents, &workflows, &managed_python);
        workflows.admit_metadata(
            &available_catalog,
            &availability,
            &skills,
            &subagents,
            &native_leaves,
        );
        let root_agent_project_files =
            super::super::agent_resources::load_profile_files(&config.agent.agents_md.files)
                .map_err(LaunchFailure::resource)?;
        let policy = crate::capabilities::AgentActivation {
            profile: config.agent.clone(),
            admitted_agents: subagents.names().into_iter().cloned().collect(),
            admitted_workflows: workflows.enabled_ids().clone(),
            project_files: root_agent_project_files.clone(),
        };
        let main = crate::capabilities::inspect_profile(
            &definitions.iter().collect::<Vec<_>>(),
            &policy,
            &skills,
            &availability,
        )?;
        let names = subagents.names().into_iter().cloned().collect();
        let admitted_workflows = workflows.enabled_ids();
        let authority = crate::runtime::agent_profile::AgentProfileAuthority {
            tools: &available_catalog,
            availability: &availability,
            skills: &skills,
            agents: &names,
            workflows: &admitted_workflows,
            scope: crate::runtime::agent_profile::AgentScope::OneShotChild,
        };
        let profiles: BTreeMap<_, _> = subagents
            .definitions()
            .map(|definition| {
                (
                    definition.name().clone(),
                    crate::runtime::agent_profile::resolve_agent_profile(
                        definition.profile(),
                        &authority,
                    ),
                )
            })
            .collect();
        let mut inspection = crate::runtime::capability_inspection::CapabilityInspection::collect(
            Some(&main),
            profiles.iter().map(|(name, profile)| {
                (
                    name,
                    profile,
                    subagents
                        .get(name)
                        .expect("resolved definition")
                        .instructions_source(),
                )
            }),
            &workflows,
            &availability,
            &skills,
        );
        resource_definitions.extend(
            role_sources
                .iter()
                .map(|(name, source)| ResourceDefinition {
                    family: ResourceFamily::Agent,
                    name: name.to_string(),
                    location: crate::runtime::resources::ResourceLocation {
                        scope: if source.layer == "workspace" {
                            SourceScope::Workspace
                        } else {
                            SourceScope::User
                        },
                        path: source.selected.clone(),
                        shadowed: source.overridden.clone(),
                    },
                    valid: !subagents.invalid().contains_key(name),
                }),
        );
        resource_definitions.extend(managed_python.locations.iter().map(|(id, location)| {
            ResourceDefinition {
                family: ResourceFamily::ManagedPython,
                name: id.managed_python().expect("Python catalog identity").into(),
                location: location.clone(),
                valid: managed_python.packages().get(id).is_some_and(Result::is_ok),
            }
        }));
        resource_definitions.extend(
            skills
                .provenance()
                .iter()
                .map(|skill| (skill, true))
                .chain(skill_discovery.invalid.iter().map(|skill| (skill, false)))
                .map(|(skill, valid)| ResourceDefinition {
                    family: ResourceFamily::Skill,
                    name: skill.name.clone(),
                    location: crate::runtime::resources::ResourceLocation {
                        scope: match skill.source {
                            crate::skills::SkillSource::User => SourceScope::User,
                            crate::skills::SkillSource::Workspace => SourceScope::Workspace,
                        },
                        path: skill.location.clone().into(),
                        shadowed: skill
                            .shadowed
                            .first()
                            .map(|lower| lower.location.clone().into()),
                    },
                    valid,
                }),
        );
        inspection.definitions.extend(resource_definitions);
        inspection
            .definitions
            .sort_by(|a, b| (&a.family, &a.name).cmp(&(&b.family, &b.name)));
        {
            use crate::runtime::capability_inspection::ResourceDiagnostic;
            diagnostics.extend(
                ResourceDiagnostic::of_agents(&subagents)
                    .chain(ResourceDiagnostic::of_workflows(&workflows))
                    .chain(ResourceDiagnostic::of_managed_python(&managed_python)),
            );
        }
        for root in [
            host.home_directory.join("rustx/.agents"),
            locations.workspace.join(".agents"),
        ] {
            if revisions.get(&root) != Some(&super::super::resource_directory::revision(&root)) {
                return Err(LaunchFailure::at(
                    Some(root),
                    "resources",
                    "authored resources changed during candidate construction",
                    "rescan after source edits settle",
                    "candidate discarded".into(),
                ));
            }
        }
        for file in project_context_files
            .iter()
            .chain(root_agent_project_files.iter())
            .chain(
                subagents
                    .definitions()
                    .flat_map(|definition| definition.profile().project_instructions.files.iter()),
            )
        {
            let bytes = read_bounded(&file.path)?;
            let text = std::str::from_utf8(&bytes)
                .map_err(|_| LaunchFailure::from("instruction text is not UTF-8"))?;
            if text.trim_start_matches('\u{feff}') != file.content {
                return Err("instruction input changed during capture; rescan to retry".into());
            }
            revisions.insert(
                file.path.clone(),
                super::super::settings::revision(Some(&bytes)),
            );
        }
        for (path, expected) in &revisions {
            // Resource roots were compared above; the two documents are
            // fenced through their admitted directories by the capture.
            if path == &host.home_directory.join("rustx/.agents")
                || path == &locations.workspace.join(".agents")
                || path == &host.config_path
                || path == &locations.workspace.join("rustx.toml")
            {
                continue;
            }
            let bytes = match read_bounded(path) {
                Ok(bytes) => Some(bytes),
                Err(_) if !path.exists() => None,
                Err(error) => return Err(error.into()),
            };
            if super::super::settings::revision(bytes.as_deref()) != *expected {
                return Err("configuration inputs changed during capture; rescan to retry".into());
            }
        }
        if crate::runtime::load_project_context_files(&locations.workspace)
            .map_err(LaunchFailure::resource)?
            != project_context_files
        {
            return Err(
                "project instruction discovery changed during capture; rescan to retry".into(),
            );
        }
        diagnostics.sort();
        diagnostics.dedup();
        diagnostics.truncate(256);
        inspection.resource_diagnostics = diagnostics;
        let revision = source_manifest_revision(&revisions);
        Ok(ProspectiveSessionConfig {
            component_revisions: [
                application::ApplyUnit::ExecutionPolicy,
                application::ApplyUnit::SharedCapacity,
                application::ApplyUnit::Capabilities,
                application::ApplyUnit::Instructions,
                application::ApplyUnit::Provider,
            ]
            .into_iter()
            .map(|unit| (unit, revision.clone()))
            .collect(),
            source_revisions: revisions,
            effective,
            root_agent_project_files,
            skill_discovery,
            project_context_files,
            inspection,
            input: request.clone(),
            sources: host.clone(),
            managed_python,
            workflows: std::sync::Arc::new(workflows),
            subagents,
            agent_root,
            role_sources,
            locations,
            config: std::sync::Arc::new(config),
            models,
            provenance,
            identity,
            skill_sources,
            project_resources,
        })
    }
}

pub(in crate::local_runtime) fn absolute(base: &Path, path: &Path) -> PathBuf {
    if path.is_absolute() {
        path.into()
    } else {
        base.join(path)
    }
}

/// Persistent Linux/macOS identity: full lowercase SHA-256 of Unix-native
/// path bytes. The caller supplies the canonical workspace, without further
/// case/Unicode normalization. Never use Rust's unspecified `OsStr` encoding.
pub(in crate::local_runtime) fn workspace_identity(canonical_workspace: &Path) -> String {
    format!(
        "{:x}",
        Sha256::digest(canonical_workspace.as_os_str().as_bytes())
    )
}

pub(in crate::local_runtime) fn canonical_directory(path: &Path) -> Result<PathBuf, String> {
    let path = std::fs::canonicalize(path)
        .map_err(|e| format!("cannot resolve {}: {e}", path.display()))?;
    if !path.is_dir() {
        return Err(format!("{} is not a directory", path.display()));
    }
    Ok(path)
}

/// The storage-isolation rule between the bound runtime root and one selected
/// Workspace: neither contains the other. Session resolution and repair scope
/// admission apply this one rule to the same canonical pathnames.
pub(in crate::local_runtime) fn storage_disjoint(
    runtime_root: &Path,
    workspace: &Path,
) -> Result<(), String> {
    if runtime_root.starts_with(workspace) || workspace.starts_with(runtime_root) {
        return Err("runtime_root must be disjoint from the workspace".into());
    }
    Ok(())
}

pub(in crate::local_runtime) fn normalize_missing(path: &Path) -> Result<PathBuf, String> {
    if present_on_disk(path)? {
        return std::fs::canonicalize(path).map_err(|e| e.to_string());
    }
    let parent = path.parent().ok_or("path has no existing ancestor")?;
    let name = path.file_name().ok_or("invalid path component")?;
    Ok(normalize_missing(parent)?.join(name))
}

fn unavailable(path: &Path, detail: String) -> LaunchFailure {
    LaunchFailure::at(
        Some(path.into()),
        "$",
        "selected configuration file is unavailable",
        "correct the selected path or create the file",
        detail,
    )
}

fn client_detail(error: crate::runtime_client::types::RuntimeClientError) -> String {
    match error {
        crate::runtime_client::types::RuntimeClientError::InvalidRequest { message } => message,
        other => format!("{other:?}"),
    }
}

/// Admit a `rustx.toml` ([`AdmittedSource::admit`]): a pathname reached only
/// by following a symlink, or through a non-directory, is unavailable.
fn admit_source(path: &Path) -> Result<AdmittedSource, LaunchFailure> {
    AdmittedSource::admit(path).map_err(|error| {
        unavailable(
            path,
            format!("its directory cannot be reached without following a symlink: {error}"),
        )
    })
}

/// A checked read of an admitted `rustx.toml` ([`AdmittedSource::reread`]).
fn reread(source: &AdmittedSource) -> Result<Option<Vec<u8>>, LaunchFailure> {
    source.reread().map_err(|failure| match failure {
        SourceReread::Redirected => unavailable(
            source.path(),
            "its directory was replaced or redirected; rustX never follows it".into(),
        ),
        SourceReread::Unadmitted(error) => unavailable(
            source.path(),
            format!("its directory cannot be reached without following a symlink: {error}"),
        ),
        SourceReread::Unreadable(error) => unavailable(source.path(), client_detail(error)),
    })
}

/// The layer authored by a document's bytes; an absent document authors none.
fn layer_of(
    path: &Path,
    bytes: Option<&[u8]>,
    workspace: bool,
) -> Result<RuntimeLayer, LaunchFailure> {
    bytes.map_or_else(
        || Ok(RuntimeLayer::default()),
        |bytes| parse_layer(path, bytes, workspace),
    )
}

pub(in crate::local_runtime) fn parse_layer(
    path: &Path,
    bytes: &[u8],
    workspace: bool,
) -> Result<RuntimeLayer, LaunchFailure> {
    let layer: RuntimeLayer =
        crate::toml_authoring::parse_detailed(bytes).map_err(|e| LaunchFailure::parse(path, e))?;
    source_restrictions(path, &layer, workspace)?;
    Ok(layer)
}

/// The source-specific contract of an already strictly parsed document: process
/// policy belongs to the User scope, and an explicit version is the current one.
pub(in crate::local_runtime) fn source_restrictions(
    path: &Path,
    layer: &RuntimeLayer,
    workspace: bool,
) -> Result<(), LaunchFailure> {
    if workspace && layer.app_server.is_some() {
        return Err(LaunchFailure::at(
            Some(path.into()),
            "app_server",
            "App Server process policy cannot be authored by a Workspace",
            "author app_server in the bound User rustx.toml",
            "process policy belongs to the User scope; native application distinguishes live limits from restart bindings".into(),
        ));
    }
    if let Some(version) = layer.schema_version
        && version != super::super::config::CURRENT_RUNTIME_SCHEMA_VERSION
    {
        return Err(LaunchFailure::at(
            Some(path.into()),
            "schema_version",
            "unsupported runtime schema version",
            "use the current canonical authoring schema",
            format!(
                "unsupported runtime schema_version {version}; this runtime speaks {}",
                super::super::config::CURRENT_RUNTIME_SCHEMA_VERSION
            ),
        ));
    }
    Ok(())
}

fn rebase_paths(
    layer: &mut RuntimeLayer,
    origin: &Origin,
    workspace: &Path,
    validate_resources: bool,
) -> Result<Vec<PathBuf>, String> {
    let mut resources = Vec::new();
    let base = match origin {
        Origin::User { base, .. } | Origin::Workspace { base, .. } | Origin::Process { base } => {
            base
        }
        Origin::Builtin => return Ok(resources),
    };
    let mut path = |value: &mut PathBuf| -> Result<(), String> {
        if value.as_os_str().is_empty() {
            return Err("path must be non-empty".into());
        }
        let resolved = absolute(base, value);
        if matches!(origin, Origin::Workspace { .. }) {
            if validate_resources {
                crate::runtime::resources::validate_project_resource_path(workspace, &resolved)
                    .map_err(|e| e.to_string())?;
            }
            resources.push(resolved.clone());
        }
        *value = resolved;
        Ok(())
    };
    if let Some(agent) = &mut layer.agent
        && let Some(policy) = &mut agent.agents_md
    {
        for file in &mut policy.files {
            path(file)?;
        }
    }
    Ok(resources)
}

pub(in crate::local_runtime) fn present_on_disk(path: &Path) -> Result<bool, String> {
    match std::fs::symlink_metadata(path) {
        Ok(_) => Ok(true),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(error) => Err(format!("cannot inspect {}: {error}", path.display())),
    }
}

/// Internal source identity: version tag, u64 big-endian entry count, then
/// length-prefixed Unix path bytes and revision bytes in `PathBuf` `BTreeMap` order.
/// Framing distinguishes both field and entry boundaries without requiring
/// filesystem paths to be Unicode or allocating a serialized manifest.
pub(in crate::local_runtime) fn source_manifest_revision(
    revisions: &BTreeMap<PathBuf, String>,
) -> String {
    let mut hash = Sha256::new();
    hash.update(b"rustx-source-manifest-v1");
    hash.update((revisions.len() as u64).to_be_bytes());
    for (path, revision) in revisions {
        for bytes in [path.as_os_str().as_bytes(), revision.as_bytes()] {
            hash.update((bytes.len() as u64).to_be_bytes());
            hash.update(bytes);
        }
    }
    format!("{:x}", hash.finalize())
}

/// The offline metadata-admission inputs of one composed configuration: the
/// native and Workflow Tool metadata, every configured external source as
/// `Unprepared`, and the native leaf identities. Capture and candidate
/// Workflow authoring checks admit with exactly these facts; nothing here
/// prepares a source, connects or reads a credential.
pub(in crate::local_runtime) struct MetadataAdmission {
    pub(in crate::local_runtime) definitions: Vec<crate::tools::types::ToolDefinition>,
    pub(in crate::local_runtime) catalog: crate::capabilities::AvailableToolCatalog,
    pub(in crate::local_runtime) availability: crate::capabilities::CapabilityAvailability,
    pub(in crate::local_runtime) native_leaves:
        std::collections::BTreeSet<crate::runtime::identity::ToolId>,
}

impl MetadataAdmission {
    pub(in crate::local_runtime) fn compose(
        config: &super::CurrentRuntimeConfig,
        subagents: &crate::runtime::subagent::AgentCatalog,
        workflows: &crate::runtime::workflow::WorkflowCatalog,
        managed_python: &crate::runtime::resources::ManagedPythonCatalog,
    ) -> Self {
        let main_subagents =
            subagents.selected_definitions(&config.agent.agents.iter().cloned().collect());
        let native_metadata =
            crate::tools::native::definitions(config.native_tools.to_policies(), &main_subagents);
        let native_leaves = native_metadata
            .iter()
            .filter(|(_, policy)| *policy == crate::tools::deadline::ForegroundPolicy::Leaf)
            .map(|(definition, _)| definition.id.clone())
            .collect();
        let mut definitions: Vec<_> = native_metadata
            .into_iter()
            .map(|(definition, _)| definition)
            .collect();
        definitions.extend(
            workflows
                .entries()
                .values()
                .map(|entry| &entry.source)
                .map(|program| crate::tools::native::workflow_definition(program)),
        );
        let availability = config
            .mcp_servers
            .keys()
            .cloned()
            .map(crate::capabilities::ToolSourceId::Mcp)
            .chain(managed_python.packages().keys().cloned())
            .map(|id| (id, crate::capabilities::CapabilitySourceState::Unprepared))
            .collect();
        Self {
            catalog: crate::capabilities::AvailableToolCatalog::metadata(definitions.clone()),
            definitions,
            availability,
            native_leaves,
        }
    }
}

#[cfg(test)]
mod source_manifest_tests {
    use super::*;
    use std::ffi::OsString;
    use std::os::unix::ffi::OsStringExt;

    #[test]
    fn exact_path_bytes_and_revisions_determine_identity() {
        let a = PathBuf::from(OsString::from_vec(b"/config-\xfe".to_vec()));
        let b = PathBuf::from(OsString::from_vec(b"/config-\xff".to_vec()));
        assert_eq!(a.to_string_lossy(), b.to_string_lossy());
        let manifest = BTreeMap::from([(a.clone(), "revision-x".into())]);
        let identity = source_manifest_revision(&manifest);
        assert_eq!(identity, source_manifest_revision(&manifest));
        assert_ne!(
            identity,
            source_manifest_revision(&BTreeMap::from([(b, "revision-x".into())]))
        );
        assert_ne!(
            identity,
            source_manifest_revision(&BTreeMap::from([(a, "revision-y".into())]))
        );
    }

    #[test]
    fn field_and_entry_boundaries_are_unambiguous() {
        let manifest = |entries: &[(&str, &str)]| {
            entries
                .iter()
                .map(|(path, revision)| (PathBuf::from(path), (*revision).to_owned()))
                .collect::<BTreeMap<_, _>>()
        };
        // Same concatenated payload, different path/revision split.
        assert_ne!(
            source_manifest_revision(&manifest(&[("ab", "c")])),
            source_manifest_revision(&manifest(&[("a", "bc")]))
        );
        // Same concatenated payload, different entry boundaries/count.
        assert_ne!(
            source_manifest_revision(&manifest(&[("a", "bcde")])),
            source_manifest_revision(&manifest(&[("a", "b"), ("c", "de")]))
        );
        // Same count and concatenated payload, different revision boundaries.
        assert_ne!(
            source_manifest_revision(&manifest(&[("a", "bc"), ("d", "e")])),
            source_manifest_revision(&manifest(&[("a", "b"), ("cd", "e")]))
        );
        let ordered = manifest(&[("a", "b"), ("c", "de")]);
        let reversed = manifest(&[("c", "de"), ("a", "b")]);
        assert_eq!(
            source_manifest_revision(&ordered),
            source_manifest_revision(&reversed)
        );
    }
}
