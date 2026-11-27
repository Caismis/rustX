//! Process-local storage for validated MCP declarations. A cache entry holds
//! no transport, execution lease or credential.
use std::collections::VecDeque;
use std::os::unix::ffi::OsStrExt;
use std::path::Path;
use std::sync::{Mutex, OnceLock};
use std::time::Duration;

use sha2::{Digest, Sha256};
use tokio::time::Instant;

use super::{McpDiscoveryOutcome, McpServerBinding, McpTransportConfig};
use crate::runtime::identity::McpServerId;
use crate::tools::workspace::Workspace;

const CAPACITY: usize = 32;
const TTL: Duration = Duration::from_mins(30);
const MAX_DECLARATION_BYTES: usize = 512 * 1024;

#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) struct DeclarationDomain([u8; 32]);

#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) struct DeclarationKey {
    pub(super) domain: DeclarationDomain,
    binding: [u8; 32],
}

impl DeclarationKey {
    pub(crate) fn for_source(
        isolation: &Path,
        server: &McpServerId,
        binding: &McpServerBinding,
        workspace: &Workspace,
    ) -> Option<Self> {
        if binding.policy.execution != crate::tools::types::ToolExecutionPolicy::ForegroundOnly
            || server
                .as_str()
                .starts_with(crate::tools::python::MANAGED_MCP_NAMESPACE)
        {
            return None;
        }
        let mut domain = Fingerprint::new();
        domain.add(isolation.as_os_str().as_bytes());
        domain.add(workspace.root().as_os_str().as_bytes());
        domain.add(server.as_str().as_bytes());

        let credentials = binding.credentials.resolve().ok()?;
        let mut fingerprint = Fingerprint::new();
        fingerprint.add(&serde_json::to_vec(binding).ok()?);
        for secrets in [
            &credentials.environment,
            &credentials.headers,
            &credentials.templates,
        ] {
            fingerprint.add(&secrets.len().to_le_bytes());
            for (name, secret) in secrets {
                fingerprint.add(name.as_bytes());
                fingerprint.add(secret.expose().as_bytes());
            }
        }
        if !matches!(binding.transport, McpTransportConfig::StreamableHttp { .. }) {
            let scope = workspace.execution_scope()?;
            if scope.delegated_work().is_some() {
                return None;
            }
            let endpoint = scope.endpoint().ok()?;
            endpoint.control()?;
            fingerprint.add(&serde_json::to_vec(endpoint.admission()?).ok()?);
        }
        Some(Self {
            domain: DeclarationDomain(domain.finish()),
            binding: fingerprint.finish(),
        })
    }
}

// Random process-local salt keeps the digests private and unrelated across
// processes. Length delimiters include all fields without ambiguous joins.
struct Fingerprint(Sha256);

impl Fingerprint {
    fn new() -> Self {
        Self(Sha256::new())
    }

    fn add(&mut self, bytes: &[u8]) {
        self.0.update(bytes.len().to_le_bytes());
        self.0.update(bytes);
    }

    fn finish(mut self) -> [u8; 32] {
        static SALT: OnceLock<[u8; 32]> = OnceLock::new();
        let salt = SALT.get_or_init(|| {
            let mut bytes = [0; 32];
            getrandom::fill(&mut bytes).expect("process entropy");
            bytes
        });
        self.0.update(salt);
        self.0.finalize().into()
    }
}

struct Entry {
    key: DeclarationKey,
    observation: u64,
    catalog: Option<(Instant, McpDiscoveryOutcome)>,
}

#[derive(Default)]
struct Cache {
    sequence: u64,
    entries: VecDeque<Entry>,
}

fn cache() -> &'static Mutex<Cache> {
    static CACHE: OnceLock<Mutex<Cache>> = OnceLock::new();
    CACHE.get_or_init(|| Mutex::new(Cache::default()))
}

impl Cache {
    fn get(&mut self, key: DeclarationKey, now: Instant) -> Option<McpDiscoveryOutcome> {
        let index = self.entries.iter().position(|entry| entry.key == key)?;
        let mut entry = self.entries.remove(index)?;
        if entry
            .catalog
            .as_ref()
            .is_some_and(|(expires, _)| now >= *expires)
        {
            entry.catalog = None;
        }
        let catalog = entry.catalog.as_ref().map(|(_, catalog)| catalog.clone());
        self.entries.push_back(entry);
        catalog
    }

    fn begin(&mut self, key: DeclarationKey) -> u64 {
        self.sequence = self
            .sequence
            .checked_add(1)
            .expect("MCP cache observation sequence");
        let catalog = self
            .entries
            .iter()
            .position(|entry| entry.key == key)
            .and_then(|index| self.entries.remove(index))
            .and_then(|entry| entry.catalog);
        if self.entries.len() == CAPACITY {
            self.entries.pop_front();
        }
        self.entries.push_back(Entry {
            key,
            observation: self.sequence,
            catalog,
        });
        self.sequence
    }

    fn complete(
        &mut self,
        key: DeclarationKey,
        observation: u64,
        catalog: Option<McpDiscoveryOutcome>,
        now: Instant,
    ) {
        if let Some(entry) = self
            .entries
            .iter_mut()
            .find(|entry| entry.key == key && entry.observation == observation)
        {
            entry.catalog = catalog.map(|catalog| (now + TTL, catalog));
        }
    }
}

pub(crate) fn get(key: DeclarationKey) -> Option<McpDiscoveryOutcome> {
    cache()
        .lock()
        .expect("MCP declaration cache")
        .get(key, Instant::now())
}

pub(super) fn begin(key: DeclarationKey) -> u64 {
    cache().lock().expect("MCP declaration cache").begin(key)
}

pub(super) fn invalidate(domain: DeclarationDomain) {
    cache()
        .lock()
        .expect("MCP declaration cache")
        .entries
        .retain(|entry| entry.key.domain != domain);
}

pub(super) fn complete(
    key: DeclarationKey,
    observation: u64,
    catalog: Option<(&McpDiscoveryOutcome, &crate::credentials::SourceCredentials)>,
) {
    let catalog = catalog.and_then(|(catalog, credentials)| cacheable(catalog, credentials));
    cache().lock().expect("MCP declaration cache").complete(
        key,
        observation,
        catalog,
        Instant::now(),
    );
}

fn cacheable(
    catalog: &McpDiscoveryOutcome,
    credentials: &crate::credentials::SourceCredentials,
) -> Option<McpDiscoveryOutcome> {
    let mut budget = MAX_DECLARATION_BYTES;
    for tool in &catalog.tools {
        let declaration = serde_json::to_value(tool).ok()?;
        budget = budget.checked_sub(serde_json::to_vec(&declaration).ok()?.len())?;
        let contains_credential = |text: &str| credentials.redact(text) != text;
        if super::diagnostics::peer_json_matches(
            &declaration,
            &contains_credential,
            &contains_credential,
        ) {
            return None;
        }
    }
    Some(catalog.clone())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tools::mcp::{CanonicalMcpTool, McpNegotiatedCapabilities};

    fn key(domain: u8, binding: u8) -> DeclarationKey {
        DeclarationKey {
            domain: DeclarationDomain([domain; 32]),
            binding: [binding; 32],
        }
    }

    fn catalog(name: &str) -> McpDiscoveryOutcome {
        McpDiscoveryOutcome {
            capabilities: McpNegotiatedCapabilities {
                tools: true,
                resources: false,
            },
            tools: vec![CanonicalMcpTool {
                name: name.to_owned(),
                description: String::new(),
                input_schema: serde_json::json!({"type": "object", "properties": {}}),
                output_schema: None,
            }],
        }
    }

    #[tokio::test(start_paused = true)]
    async fn newest_live_observation_has_fixed_expiry_even_on_a_hit() {
        let mut cache = Cache::default();
        let key = key(1, 2);
        let first = cache.begin(key);
        let second = cache.begin(key);
        cache.complete(key, second, Some(catalog("new")), Instant::now());
        cache.complete(key, first, Some(catalog("old")), Instant::now());
        tokio::time::advance(TTL.checked_sub(Duration::from_secs(1)).unwrap()).await;
        assert_eq!(cache.get(key, Instant::now()), Some(catalog("new")));
        tokio::time::advance(Duration::from_secs(1)).await;
        assert!(cache.get(key, Instant::now()).is_none());
    }

    #[test]
    fn capacity_eviction_prevents_inflight_republication_and_keys_are_exact() {
        let mut cache = Cache::default();
        let old = key(1, 2);
        let sequence = cache.begin(old);
        let now = Instant::now();
        cache.complete(old, sequence, Some(catalog("other")), now);
        assert!(cache.get(key(2, 2), now).is_none());
        assert!(cache.get(key(1, 3), now).is_none());
        for index in 0..u8::try_from(CAPACITY).unwrap() {
            cache.begin(key(3, index));
        }
        cache.complete(old, sequence, Some(catalog("too-late")), now);
        assert_eq!(cache.entries.len(), CAPACITY);
        assert!(cache.get(old, now).is_none());
    }

    #[test]
    fn same_reference_with_a_rotated_value_has_a_different_key() {
        let directory = tempfile::tempdir().unwrap();
        let workspace = Workspace::new(directory.path()).unwrap();
        let server = McpServerId::new("fixture");
        let mut binding: McpServerBinding = serde_json::from_value(serde_json::json!({
            "credentials": { "headers": { "authorization": "$FIXTURE_TOKEN" } },
            "transport": { "transport": "streamable_http", "endpoint": "https://example.test/mcp", "headers": {} },
            "policy": crate::tools::types::ToolInvocationPolicy::default(),
        })).unwrap();
        let scope = directory.path().join("conversation");
        assert!(DeclarationKey::for_source(&scope, &server, &binding, &workspace).is_none());
        binding
            .credentials
            .capture(crate::credentials::CredentialSnapshot::new([(
                "FIXTURE_TOKEN".into(),
                "credential-one".into(),
            )]));
        let first = DeclarationKey::for_source(&scope, &server, &binding, &workspace).unwrap();
        let same = DeclarationKey::for_source(&scope, &server, &binding, &workspace).unwrap();
        assert!(first == same);
        binding
            .credentials
            .capture(crate::credentials::CredentialSnapshot::new([(
                "FIXTURE_TOKEN".into(),
                "credential-two".into(),
            )]));
        let rotated = DeclarationKey::for_source(&scope, &server, &binding, &workspace).unwrap();
        assert!(first != rotated);
        let other_scope = directory.path().join("other-conversation");
        let separated =
            DeclarationKey::for_source(&other_scope, &server, &binding, &workspace).unwrap();
        assert!(rotated != separated);
    }

    #[test]
    fn echoed_secret_in_structured_declaration_is_refused_by_cache() {
        let mut declaration = catalog("echo");
        let secret = "private-credential";
        let credentials = crate::credentials::SourceCredentials {
            literal_redactions: vec![crate::model::catalog::ResolvedCredential::new(secret)],
            ..Default::default()
        };
        assert!(cacheable(&declaration, &credentials).is_some());
        declaration.tools[0].input_schema["description"] = serde_json::json!(secret);
        assert!(cacheable(&declaration, &credentials).is_none());
    }
}
