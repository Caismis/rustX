//! Bounded process-local discovery hints. A hit contains no transport,
//! executor, permission, or proof that a server still offers its tools.
//!
//! Only a complete, validated, epoch-stable live catalog may be published.
//! Every use still requires the owning connection's before-effect validation.
//! The SDK response cache remains disabled: network discovery never reads here.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, OnceLock};
use std::time::{Duration, Instant};

use rmcp::ClientHandler as _;
use sha2::{Digest as _, Sha256};

use crate::runtime::identity::McpServerId;
use crate::tools::workspace::Workspace;

use super::{CanonicalMcpTool, McpServerBinding, McpTransportConfig, identity};

const CAPACITY: usize = 32;
const TTL: Duration = Duration::from_mins(30);

pub(super) fn valid_catalog(tools: &[CanonicalMcpTool]) -> bool {
    let mut names = std::collections::BTreeSet::new();
    tools.iter().all(|tool| {
        names.insert(&tool.name)
            && !tool.name.is_empty()
            && crate::tools::executor::validate_model_facing_name(&tool.name).is_ok()
            && crate::tools::schema::validate_canonical_schema(&tool.input_schema).is_ok()
    })
}

/// Secret-bearing configuration is hashed at construction and is never retained.
#[derive(Clone, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) struct CatalogCacheKey([u8; 32]);

impl std::fmt::Debug for CatalogCacheKey {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("CatalogCacheKey(<opaque>)")
    }
}

impl CatalogCacheKey {
    /// The effective environment is explicit (`env_clear` in the process
    /// owner), with the same PATH default and credential overrides as connect.
    /// Source credentials are immutable host captures; unresolved references
    /// and unresolvable local boundaries disable caching rather than guessing.
    pub(crate) fn for_binding(
        server_id: &McpServerId,
        binding: &McpServerBinding,
        workspace: &Workspace,
    ) -> Option<Self> {
        let credentials = binding.credentials.resolve().ok()?;
        let mut hash = Sha256::new();
        field(&mut hash, b"RUSTX_MCP_CATALOG_HINT_V1");
        field(&mut hash, server_id.as_str().as_bytes());
        field(&mut hash, workspace.root().as_os_str().as_encoded_bytes());
        // Includes transport kind, all configured arguments, reference names,
        // static headers, and invocation policy. Only the digest survives.
        field(&mut hash, &serde_json::to_vec(binding).ok()?);
        for values in [&credentials.environment, &credentials.headers] {
            field(&mut hash, &u64::try_from(values.len()).ok()?.to_le_bytes());
            for (name, value) in values {
                field(&mut hash, name.as_bytes());
                field(&mut hash, value.expose().as_bytes());
            }
        }
        match &binding.transport {
            McpTransportConfig::Stdio {
                program,
                cwd,
                environment,
                ..
            } => {
                let cwd = cwd.as_ref().map_or_else(
                    || workspace.root().to_path_buf(),
                    |cwd| workspace.root().join(cwd),
                );
                if !cwd.is_dir() {
                    return None;
                }
                let cwd = cwd.canonicalize().ok()?;
                field(&mut hash, cwd.as_os_str().as_encoded_bytes());
                let mut effective = BTreeMap::from([(
                    "PATH".to_owned(),
                    "/usr/local/bin:/usr/bin:/bin".to_owned(),
                )]);
                effective.extend(environment.clone());
                effective.extend(
                    credentials
                        .environment
                        .iter()
                        .map(|(name, value)| (name.clone(), value.expose().to_owned())),
                );
                field(&mut hash, &serde_json::to_vec(&effective).ok()?);
                // A replaced executable or retargeted symlink invalidates its
                // hints too. Unknown PATH lookup is deliberately a cache miss.
                let executable = resolve_executable(program, &cwd, effective.get("PATH")?)?;
                let metadata = executable.metadata().ok()?;
                field(&mut hash, executable.as_os_str().as_encoded_bytes());
                field(&mut hash, &metadata.len().to_le_bytes());
                let modified = metadata
                    .modified()
                    .ok()?
                    .duration_since(std::time::UNIX_EPOCH)
                    .ok()?;
                field(&mut hash, &modified.as_nanos().to_le_bytes());
            }
            McpTransportConfig::StreamableHttp { .. } => {
                // reqwest's default proxy discovery belongs to the effective
                // transport boundary even when no proxy was configured here.
                for name in [
                    "HTTP_PROXY",
                    "http_proxy",
                    "HTTPS_PROXY",
                    "https_proxy",
                    "ALL_PROXY",
                    "all_proxy",
                    "NO_PROXY",
                    "no_proxy",
                ] {
                    field(&mut hash, name.as_bytes());
                    match std::env::var_os(name) {
                        Some(value) => {
                            field(&mut hash, b"present");
                            field(&mut hash, value.as_encoded_bytes());
                        }
                        None => field(&mut hash, b"absent"),
                    }
                }
            }
        }
        let versions = super::supported_protocol_versions();
        for version in versions {
            field(&mut hash, version.as_str().as_bytes());
        }
        field(
            &mut hash,
            super::legacy_handshake_version().as_str().as_bytes(),
        );
        field(
            &mut hash,
            if server_id
                .as_str()
                .starts_with(crate::tools::python::MANAGED_MCP_NAMESPACE)
            {
                b"discover"
            } else {
                b"auto"
            },
        );
        // Use the handler's actual handshake info, not a duplicate capability
        // declaration that could drift when the protocol implementation changes.
        field(
            &mut hash,
            &serde_json::to_vec(&super::handler::McpClientHandler::new().get_info()).ok()?,
        );
        Some(Self(hash.finalize().into()))
    }
}

fn resolve_executable(program: &str, cwd: &Path, search_path: &str) -> Option<PathBuf> {
    let program = Path::new(program);
    let candidate = if program.components().count() > 1 || program.is_absolute() {
        cwd.join(program)
    } else {
        std::env::split_paths(search_path)
            .map(|path| cwd.join(path).join(program))
            .find(|path| path.is_file())?
    };
    candidate
        .is_file()
        .then(|| candidate.canonicalize().ok())
        .flatten()
}

fn field(hash: &mut Sha256, bytes: &[u8]) {
    hash.update(
        u64::try_from(bytes.len())
            .expect("field length fits u64")
            .to_le_bytes(),
    );
    hash.update(bytes);
}

/// Fully canonical inert metadata and its exact schema/content identity.
/// No policy or executor is recovered from this object.
#[derive(Debug)]
pub(crate) struct CachedMcpCatalog {
    tools: Vec<CanonicalMcpTool>,
    content_identity: [u8; 32],
    expires_at: Instant,
    valid: Arc<AtomicBool>,
}

/// A frozen hint capture can outlive its cache slot, but invalidation,
/// replacement, eviction, and expiry prevent its subsequent publication.
#[derive(Clone, Debug)]
pub(crate) struct CatalogHint {
    key: CatalogCacheKey,
    catalog: Arc<CachedMcpCatalog>,
}

impl CatalogHint {
    pub(crate) fn tools(&self) -> &[CanonicalMcpTool] {
        &self.catalog.tools
    }

    pub(crate) fn valid(&self) -> bool {
        self.catalog.valid.load(Ordering::Acquire) && Instant::now() < self.catalog.expires_at
    }

    /// Exact comparison includes the full canonical schema, not just names
    /// and descriptions. Ordering of the input catalog is immaterial.
    #[cfg(test)]
    pub(crate) fn matches_tools(&self, tools: &[CanonicalMcpTool]) -> bool {
        self.catalog.content_identity == content_identity(tools)
    }
}

fn content_identity(tools: &[CanonicalMcpTool]) -> [u8; 32] {
    let mut ordered: Vec<_> = tools.iter().collect();
    ordered.sort_by(|left, right| left.name.cmp(&right.name));
    let mut hash = Sha256::new();
    field(&mut hash, b"RUSTX_MCP_CATALOG_CONTENT_V1");
    for tool in ordered {
        field(&mut hash, tool.name.as_bytes());
        field(&mut hash, tool.description.as_bytes());
        field(
            &mut hash,
            identity::canonical_json(&tool.input_schema).as_bytes(),
        );
    }
    hash.finalize().into()
}

#[derive(Debug)]
pub(crate) struct CatalogCacheTicket {
    key: CatalogCacheKey,
    slot: u64,
    fetch: u64,
}

struct Entry {
    slot: u64,
    touched: u64,
    last_published: u64,
    catalog: Option<Arc<CachedMcpCatalog>>,
}

impl Drop for Entry {
    fn drop(&mut self) {
        if let Some(catalog) = &self.catalog {
            catalog.valid.store(false, Ordering::Release);
        }
    }
}

#[derive(Default)]
struct CacheState {
    entries: BTreeMap<CatalogCacheKey, Entry>,
    sequence: u64,
}

impl CacheState {
    fn next(&mut self) -> u64 {
        self.sequence = self
            .sequence
            .checked_add(1)
            .expect("MCP catalog cache sequence exhausted");
        self.sequence
    }
}

/// The process host shares only this bounded metadata owner. Transport
/// lifetime, cancellation, and physical settlement remain conversation-local.
#[derive(Default)]
pub(crate) struct McpCatalogCache {
    state: Mutex<CacheState>,
}

impl McpCatalogCache {
    pub(crate) fn process() -> Arc<Self> {
        static CACHE: OnceLock<Arc<McpCatalogCache>> = OnceLock::new();
        Arc::clone(CACHE.get_or_init(|| Arc::new(Self::default())))
    }

    /// Commit lock order is coordinator state -> invalidation -> catalog.
    /// No cache operation takes either of the outer locks.
    pub(crate) fn lock(&self) -> CatalogCacheGuard<'_> {
        CatalogCacheGuard {
            state: self.state.lock().expect("MCP catalog cache lock poisoned"),
        }
    }

    pub(crate) fn lookup(&self, key: &CatalogCacheKey) -> Option<CatalogHint> {
        self.lookup_at(key, Instant::now())
    }

    fn lookup_at(&self, key: &CatalogCacheKey, now: Instant) -> Option<CatalogHint> {
        let mut guard = self.lock();
        let catalog = guard.state.entries.get(key)?.catalog.clone()?;
        if now >= catalog.expires_at || !catalog.valid.load(Ordering::Acquire) {
            guard.state.entries.remove(key);
            return None;
        }
        let touched = guard.state.next();
        guard.state.entries.get_mut(key)?.touched = touched;
        Some(CatalogHint {
            key: key.clone(),
            catalog,
        })
    }

    pub(crate) fn begin(&self, key: &CatalogCacheKey) -> CatalogCacheTicket {
        let mut guard = self.lock();
        let fetch = guard.state.next();
        if !guard.state.entries.contains_key(key) && guard.state.entries.len() == CAPACITY {
            let oldest = guard
                .state
                .entries
                .iter()
                .min_by_key(|(_, entry)| entry.touched)
                .map(|(key, _)| key.clone())
                .expect("a full cache has an LRU entry");
            guard.state.entries.remove(&oldest);
        }
        let entry = guard.state.entries.entry(key.clone()).or_insert(Entry {
            slot: fetch,
            touched: fetch,
            last_published: 0,
            catalog: None,
        });
        entry.touched = fetch;
        CatalogCacheTicket {
            key: key.clone(),
            slot: entry.slot,
            fetch,
        }
    }

    /// Caller proves successful negotiated handshake, complete pagination,
    /// canonical registration validation, and a stable invalidation epoch.
    /// This operation never turns a failed refresh into successful discovery.
    pub(crate) fn publish(
        &self,
        ticket: CatalogCacheTicket,
        tools: Vec<CanonicalMcpTool>,
    ) -> Option<CatalogHint> {
        self.publish_at(ticket, tools, Instant::now())
    }

    fn publish_at(
        &self,
        ticket: CatalogCacheTicket,
        mut tools: Vec<CanonicalMcpTool>,
        now: Instant,
    ) -> Option<CatalogHint> {
        tools.sort_by(|left, right| left.name.cmp(&right.name));
        // Defense at the metadata boundary; policy-specific validation remains
        // in the ordinary capability registry before the caller publishes.
        if !valid_catalog(&tools) {
            self.fail(&ticket);
            return None;
        }
        let mut guard = self.lock();
        let touched = guard.state.next();
        let entry = guard.state.entries.get_mut(&ticket.key)?;
        if entry.slot != ticket.slot || entry.last_published >= ticket.fetch {
            return None;
        }
        let content_identity = content_identity(&tools);
        let valid = entry
            .catalog
            .as_ref()
            .filter(|previous| {
                previous.content_identity == content_identity
                    && previous.valid.load(Ordering::Acquire)
            })
            .map_or_else(
                || Arc::new(AtomicBool::new(true)),
                |previous| Arc::clone(&previous.valid),
            );
        let catalog = Arc::new(CachedMcpCatalog {
            content_identity,
            tools,
            expires_at: now + TTL,
            valid,
        });
        if let Some(previous) = entry.catalog.replace(Arc::clone(&catalog))
            && !Arc::ptr_eq(&previous.valid, &catalog.valid)
        {
            previous.valid.store(false, Ordering::Release);
        }
        entry.touched = touched;
        entry.last_published = ticket.fetch;
        Some(CatalogHint {
            key: ticket.key,
            catalog,
        })
    }

    pub(crate) fn invalidate(&self, key: &CatalogCacheKey) {
        self.lock().state.entries.remove(key);
    }

    /// A failed fetch revokes its older hint, but an older failure cannot
    /// erase a newer successful catalog published by another conversation.
    pub(crate) fn fail(&self, ticket: &CatalogCacheTicket) {
        let mut guard = self.lock();
        if guard
            .state
            .entries
            .get(&ticket.key)
            .is_some_and(|entry| entry.slot == ticket.slot && entry.last_published <= ticket.fetch)
        {
            guard.state.entries.remove(&ticket.key);
        }
    }
}

pub(crate) struct CatalogCacheGuard<'a> {
    state: MutexGuard<'a, CacheState>,
}

impl CatalogCacheGuard<'_> {
    /// Hold this guard through candidate publication so a notification in
    /// any conversation and the cached publication have exactly one winner.
    pub(crate) fn valid(&self, hint: &CatalogHint) -> bool {
        hint.valid()
            && self
                .state
                .entries
                .get(&hint.key)
                .and_then(|entry| entry.catalog.as_ref())
                .is_some_and(|current| Arc::ptr_eq(&current.valid, &hint.catalog.valid))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::credentials::{CredentialSnapshot, SourceCredentials};
    use crate::tools::types::ToolInvocationPolicy;

    fn key(index: u8) -> CatalogCacheKey {
        CatalogCacheKey([index; 32])
    }

    fn tools(schema_type: &str) -> Vec<CanonicalMcpTool> {
        vec![CanonicalMcpTool {
            name: "lookup".into(),
            description: "Find an item".into(),
            input_schema: serde_json::json!({"type":"object","properties":{"id":{"type":schema_type}}}),
        }]
    }

    fn http_binding() -> McpServerBinding {
        McpServerBinding {
            credentials: SourceCredentials::default(),
            transport: McpTransportConfig::StreamableHttp {
                endpoint: "https://mcp.example.test/tools".into(),
                headers: BTreeMap::new(),
            },
            policy: ToolInvocationPolicy::default(),
        }
    }

    #[test]
    fn newest_valid_publication_wins_competing_fetches() {
        let cache = McpCatalogCache::default();
        let older = cache.begin(&key(1));
        let newer = cache.begin(&key(1));
        let newest = cache.publish(newer, tools("integer")).unwrap();
        assert!(cache.publish(older, tools("string")).is_none());
        assert!(
            cache
                .lookup(&key(1))
                .unwrap()
                .matches_tools(&tools("integer"))
        );
        assert!(cache.lock().valid(&newest));

        // A younger fetch that has only begun is not a successful publication.
        let ready = cache.begin(&key(1));
        let _failed = cache.begin(&key(1));
        assert!(cache.publish(ready, tools("number")).is_some());
        assert!(!newest.valid());
    }

    #[test]
    fn invalidation_rejects_inflight_fetch_and_frozen_capture() {
        let cache = McpCatalogCache::default();
        let hint = cache
            .publish(cache.begin(&key(1)), tools("string"))
            .unwrap();
        let obsolete = cache.begin(&key(1));
        cache.invalidate(&key(1));
        assert!(!hint.valid());
        assert!(!cache.lock().valid(&hint));
        assert!(cache.lookup(&key(1)).is_none());
        let fresh = cache.begin(&key(1));
        assert!(cache.publish(obsolete, tools("string")).is_none());
        assert!(cache.publish(fresh, tools("integer")).is_some());
    }

    #[test]
    fn equivalent_live_publication_preserves_warm_capture_until_invalidation() {
        let cache = McpCatalogCache::default();
        let captured = cache
            .publish(cache.begin(&key(1)), tools("string"))
            .unwrap();
        let live = cache
            .publish(cache.begin(&key(1)), tools("string"))
            .unwrap();
        assert!(captured.valid());
        assert!(cache.lock().valid(&captured));
        assert!(cache.lock().valid(&live));
        cache.invalidate(&key(1));
        assert!(!captured.valid());
        assert!(!live.valid());
    }

    #[test]
    fn failed_older_fetch_cannot_erase_newer_success_or_recreated_slot() {
        let cache = McpCatalogCache::default();
        let older = cache.begin(&key(1));
        let latest = cache
            .publish(cache.begin(&key(1)), tools("string"))
            .unwrap();
        cache.fail(&older);
        assert!(latest.valid());
        assert!(cache.lock().valid(&latest));

        let outdated = cache.begin(&key(1));
        cache.invalidate(&key(1));
        let recreated = cache
            .publish(cache.begin(&key(1)), tools("integer"))
            .unwrap();
        cache.fail(&outdated);
        assert!(recreated.valid());
        let failed = cache.begin(&key(1));
        cache.fail(&failed);
        assert!(!recreated.valid());
        assert!(cache.lookup(&key(1)).is_none());
    }

    #[test]
    fn expiry_never_returns_stale_metadata_on_failed_refresh() {
        let cache = McpCatalogCache::default();
        let now = Instant::now();
        let hint = cache
            .publish_at(cache.begin(&key(1)), tools("string"), now)
            .unwrap();
        assert!(
            cache
                .lookup_at(
                    &key(1),
                    (now + TTL).checked_sub(Duration::from_nanos(1)).unwrap()
                )
                .is_some()
        );
        let failed = cache.begin(&key(1));
        assert!(cache.lookup_at(&key(1), now + TTL).is_none());
        assert!(!hint.valid());
        assert!(cache.publish(failed, tools("integer")).is_none());
    }

    #[test]
    fn lru_is_bounded_and_eviction_invalidates_captured_and_inflight_hints() {
        let cache = McpCatalogCache::default();
        let first = cache
            .publish(cache.begin(&key(0)), tools("string"))
            .unwrap();
        let second_ticket = cache.begin(&key(1));
        for index in 2..u8::try_from(CAPACITY).unwrap() {
            cache
                .publish(cache.begin(&key(index)), tools("string"))
                .unwrap();
        }
        cache.lookup(&key(0)).unwrap();
        cache.begin(&key(32));
        assert!(
            first.valid(),
            "the lookup refreshed the first slot's LRU order"
        );
        assert!(cache.publish(second_ticket, tools("integer")).is_none());
        for index in 33..65 {
            cache.begin(&key(index));
        }
        assert!(!first.valid());
        assert_eq!(cache.lock().state.entries.len(), CAPACITY);
    }

    #[test]
    fn exact_catalog_identity_detects_schema_only_changes_and_ignores_order() {
        let cache = McpCatalogCache::default();
        let mut catalog = tools("string");
        let mut other = catalog[0].clone();
        other.name = "other".into();
        catalog.push(other);
        let hint = cache
            .publish(cache.begin(&key(1)), catalog.clone())
            .unwrap();
        catalog.reverse();
        assert!(hint.matches_tools(&catalog));
        catalog[1].input_schema = tools("integer")[0].input_schema.clone();
        assert!(!hint.matches_tools(&catalog));
        assert_eq!(hint.tools().len(), 2);
    }

    #[test]
    fn invalid_or_duplicate_catalogs_cannot_enter_cache() {
        let cache = McpCatalogCache::default();
        let mut duplicate = tools("string");
        duplicate.push(duplicate[0].clone());
        assert!(cache.publish(cache.begin(&key(1)), duplicate).is_none());
        let mut invalid = tools("string");
        invalid[0].input_schema = serde_json::json!({"type":"array"});
        assert!(cache.publish(cache.begin(&key(1)), invalid).is_none());
        assert!(cache.lookup(&key(1)).is_none());
    }

    #[test]
    fn identity_separates_workspace_server_transport_policy_and_resolved_secrets() {
        let root = tempfile::tempdir().unwrap();
        let other_root = tempfile::tempdir().unwrap();
        let workspace = Workspace::new(root.path()).unwrap();
        let other_workspace = Workspace::new(other_root.path()).unwrap();
        let server = McpServerId::new("fixture");
        let baseline = http_binding();
        let identity = CatalogCacheKey::for_binding(&server, &baseline, &workspace).unwrap();
        assert_ne!(
            Some(&identity),
            CatalogCacheKey::for_binding(&server, &baseline, &other_workspace).as_ref()
        );
        assert_ne!(
            Some(&identity),
            CatalogCacheKey::for_binding(&McpServerId::new("other"), &baseline, &workspace)
                .as_ref()
        );
        let mut changed = baseline.clone();
        changed.policy.approval = crate::tools::types::ToolApprovalPolicy::Always;
        assert_ne!(
            Some(&identity),
            CatalogCacheKey::for_binding(&server, &changed, &workspace).as_ref()
        );
        let mut changed = baseline.clone();
        let McpTransportConfig::StreamableHttp { endpoint, headers } = &mut changed.transport
        else {
            unreachable!()
        };
        endpoint.push_str("/other");
        headers.insert("Authorization".into(), "SECRET_STATIC_SENTINEL".into());
        assert_ne!(
            Some(&identity),
            CatalogCacheKey::for_binding(&server, &changed, &workspace).as_ref()
        );
        let mut changed = baseline;
        changed.credentials =
            serde_json::from_value(serde_json::json!({"headers":{"Authorization":"$TOKEN"}}))
                .unwrap();
        assert!(CatalogCacheKey::for_binding(&server, &changed, &workspace).is_none());
        changed.credentials.capture(CredentialSnapshot::new([(
            "TOKEN".into(),
            "SECRET_FIRST_SENTINEL".into(),
        )]));
        let first = CatalogCacheKey::for_binding(&server, &changed, &workspace).unwrap();
        changed.credentials.capture(CredentialSnapshot::new([(
            "TOKEN".into(),
            "SECRET_SECOND_SENTINEL".into(),
        )]));
        let second = CatalogCacheKey::for_binding(&server, &changed, &workspace).unwrap();
        assert_ne!(first, second);
        assert!(!format!("{first:?} {second:?}").contains("SENTINEL"));
    }

    #[test]
    fn stdio_identity_covers_effective_command_args_cwd_and_environment() {
        let root = tempfile::tempdir().unwrap();
        std::fs::create_dir(root.path().join("subdir")).unwrap();
        let workspace = Workspace::new(root.path()).unwrap();
        let server = McpServerId::new("fixture");
        let mut binding = http_binding();
        binding.transport = McpTransportConfig::Stdio {
            program: "/bin/sh".into(),
            args: vec!["-c".into(), "example".into()],
            cwd: None,
            environment: BTreeMap::new(),
        };
        let baseline = CatalogCacheKey::for_binding(&server, &binding, &workspace).unwrap();
        for variant in 0..4 {
            let mut changed = binding.clone();
            let McpTransportConfig::Stdio {
                program,
                args,
                cwd,
                environment,
            } = &mut changed.transport
            else {
                unreachable!()
            };
            match variant {
                0 => *program = "/bin/cat".into(),
                1 => args.push("extra".into()),
                2 => *cwd = Some(PathBuf::from("subdir")),
                _ => {
                    environment.insert("TOKEN".into(), "SECRET_ENV_SENTINEL".into());
                }
            }
            assert_ne!(
                Some(&baseline),
                CatalogCacheKey::for_binding(&server, &changed, &workspace).as_ref()
            );
        }
        let McpTransportConfig::Stdio { program, .. } = &mut binding.transport else {
            unreachable!()
        };
        *program = "missing-executable-that-cannot-be-resolved".into();
        assert!(CatalogCacheKey::for_binding(&server, &binding, &workspace).is_none());
    }

    #[test]
    fn attached_source_change_and_notification_invalidate_shared_metadata() {
        let root = tempfile::tempdir().unwrap();
        let workspace = Workspace::new(root.path()).unwrap();
        let server = McpServerId::new("fixture");
        let binding = http_binding();
        let key = CatalogCacheKey::for_binding(&server, &binding, &workspace).unwrap();
        let cache = McpCatalogCache::process();
        let state = super::super::McpInvalidationState::new();
        state.attach_cache_key(&server, Some(key.clone()));
        let first = cache.publish(cache.begin(&key), tools("string")).unwrap();
        state.lock().advance(&server);
        assert!(!first.valid());
        let second = cache.publish(cache.begin(&key), tools("string")).unwrap();
        state.attach_cache_key(&server, None);
        assert!(!second.valid());
    }
}
