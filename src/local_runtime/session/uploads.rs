//! Session-owned mutable workspace uploads. A durable allocation claim precedes
//! all filesystem mutations. Complete bytes and directory entries are synced
//! before the ready registry commit; only that commit permits a usable receipt.
use super::{SessionCatalog, SessionError, SessionId};
use crate::message::content::UploadedFileRef;
use nix::fcntl::{OFlag, open, openat};
use nix::sys::stat::{Mode, mkdirat};
use nix::unistd::{UnlinkatFlags, unlinkat};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};
use std::ffi::OsStr;
use std::fs::File;
use std::io::{self, Write};
use std::os::unix::fs::MetadataExt;
use std::path::{Component, Path, PathBuf};

/// Bytes are transport-independent; clients never choose a destination path.
#[derive(Debug)]
pub struct UploadFile {
    pub name: String,
    pub bytes: Vec<u8>,
}
/// A server-issued capability, scoped to exactly one Session.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct UploadReceipt {
    pub session_id: SessionId,
    pub batch_id: String,
    pub token: String,
}
/// Clients author text and reference completed server receipts only.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum UserInputBlock {
    Text(crate::message::content::TextBlock),
    Upload(UploadReceipt),
}

/// A successful ordered file allocation. Paths are a presentation of ownership,
/// never input authority or canonical message identity.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct UploadedFile {
    pub receipt: UploadReceipt,
    pub file: UploadedFileRef,
    pub path: String,
}
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct UploadRegistry {
    pub allocations: BTreeMap<String, UploadAllocation>,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct UploadAllocation {
    pub workspace: PathBuf,
    pub files: Vec<UploadEntry>,
    pub ready: bool,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct UploadEntry {
    pub name: String,
    pub token: String,
}
fn invalid(message: &str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidInput, message)
}
/// Reject path-shaped and nonportable filenames rather than renaming them.
/// # Errors
/// Rejects separators, reserved names, control characters and invalid basenames.
pub fn validate_name(name: &str) -> io::Result<()> {
    let stem = name
        .split('.')
        .next()
        .unwrap_or_default()
        .to_ascii_uppercase();
    if name.is_empty()
        || name.len() > 255
        || name == "."
        || name == ".."
        || name.ends_with(['.', ' '])
        || name.chars().any(|c| {
            c.is_control() || matches!(c, '/' | '\\' | ':' | '*' | '?' | '"' | '<' | '>' | '|')
        })
        || matches!(stem.as_str(), "CON" | "PRN" | "AUX" | "NUL")
        || (stem.len() == 4
            && (stem.starts_with("COM") || stem.starts_with("LPT"))
            && stem.as_bytes()[3].is_ascii_digit())
    {
        return Err(invalid("upload name must be one safe basename"));
    }
    Ok(())
}
fn validate_workspace(workspace: &Path) -> io::Result<()> {
    if !workspace.is_absolute() {
        return Err(invalid("upload workspace must be absolute"));
    }
    let mut previous = None;
    for component in workspace.components() {
        match component {
            Component::RootDir => {}
            Component::Normal(name) => {
                if previous == Some(OsStr::new(".agents")) && name == "uploads" {
                    return Err(invalid(
                        "upload allocations cannot nest inside upload storage",
                    ));
                }
                previous = Some(name);
            }
            _ => return Err(invalid("unsafe upload workspace component")),
        }
    }
    Ok(())
}
fn identity() -> io::Result<String> {
    use std::fmt::Write;
    let mut bytes = [0; 16];
    getrandom::fill(&mut bytes).map_err(io::Error::other)?;
    Ok(bytes.iter().fold(String::with_capacity(32), |mut s, b| {
        write!(s, "{b:02x}").expect("string write");
        s
    }))
}
fn valid_identity(id: &str) -> bool {
    id.len() == 32 && id.bytes().all(|c| c.is_ascii_hexdigit())
}
fn directory_at(parent: &File, name: &OsStr) -> io::Result<File> {
    Ok(File::from(openat(
        parent,
        name,
        OFlag::O_RDONLY | OFlag::O_DIRECTORY | OFlag::O_NOFOLLOW | OFlag::O_CLOEXEC,
        Mode::empty(),
    )?))
}
fn stable_directory(path: &Path) -> io::Result<File> {
    if !path.is_absolute() {
        return Err(invalid("workspace must be absolute"));
    }
    let mut current = File::from(open(
        Path::new("/"),
        OFlag::O_RDONLY | OFlag::O_DIRECTORY | OFlag::O_CLOEXEC,
        Mode::empty(),
    )?);
    for component in path.components() {
        match component {
            Component::RootDir => {}
            Component::Normal(name) => current = directory_at(&current, name)?,
            _ => return Err(invalid("unsafe workspace component")),
        }
    }
    Ok(current)
}
fn ensure_directory(parent: &File, name: &str) -> io::Result<File> {
    match mkdirat(parent, name, Mode::from_bits_truncate(0o700)) {
        Ok(()) | Err(nix::errno::Errno::EEXIST) => {}
        Err(e) => return Err(e.into()),
    }
    let child = directory_at(parent, OsStr::new(name))?;
    child.sync_all()?;
    parent.sync_all()?;
    Ok(child)
}
fn session_directory(workspace: &Path, session: &SessionId, create: bool) -> io::Result<File> {
    super::validate_id(session.as_str(), "upload Session").map_err(io::Error::other)?;
    let mut current = stable_directory(workspace)?;
    for name in [".agents", "uploads", session.as_str()] {
        current = if create {
            ensure_directory(&current, name)?
        } else {
            directory_at(&current, OsStr::new(name))?
        };
    }
    Ok(current)
}
fn file_path(workspace: &Path, session: &SessionId, batch: &str, name: &str) -> PathBuf {
    workspace
        .join(".agents/uploads")
        .join(session.as_str())
        .join(batch)
        .join(name)
}
impl UploadRegistry {
    pub(crate) fn validate(&self) -> io::Result<()> {
        for (batch, allocation) in &self.allocations {
            validate_workspace(&allocation.workspace)?;
            if !valid_identity(batch) || allocation.files.is_empty() {
                return Err(invalid("invalid owned upload allocation"));
            }
            let mut names = BTreeSet::new();
            for file in &allocation.files {
                validate_name(&file.name)?;
                if !valid_identity(&file.token) || !names.insert(&file.name) {
                    return Err(invalid("invalid owned upload entry"));
                }
            }
        }
        Ok(())
    }
    pub(crate) fn roots(&self) -> Vec<PathBuf> {
        self.allocations
            .values()
            .map(|a| a.workspace.clone())
            .collect::<BTreeSet<_>>()
            .into_iter()
            .collect()
    }
    pub(crate) fn claim(&mut self, workspace: PathBuf, files: &[UploadFile]) -> io::Result<String> {
        validate_workspace(&workspace)?;
        if files.is_empty() {
            return Err(invalid("upload batch must not be empty"));
        }
        let mut names = BTreeSet::new();
        let entries = files
            .iter()
            .map(|file| {
                validate_name(&file.name)?;
                if !names.insert(&file.name) {
                    return Err(invalid("duplicate upload basename"));
                }
                Ok(UploadEntry {
                    name: file.name.clone(),
                    token: identity()?,
                })
            })
            .collect::<io::Result<Vec<_>>>()?;
        let batch = identity()?;
        if self.allocations.contains_key(&batch) {
            return Err(invalid("upload allocation collision"));
        }
        self.allocations.insert(
            batch.clone(),
            UploadAllocation {
                workspace,
                files: entries,
                ready: false,
            },
        );
        Ok(batch)
    }
    pub(crate) fn materialize(
        &self,
        session: &SessionId,
        batch: &str,
        files: &[UploadFile],
    ) -> io::Result<()> {
        let allocation = self
            .allocations
            .get(batch)
            .ok_or_else(|| invalid("unclaimed upload"))?;
        if allocation.files.len() != files.len() {
            return Err(invalid("upload batch length mismatch"));
        }
        let root = session_directory(&allocation.workspace, session, true)?;
        // Exclusive batch creation: never adopt or overwrite existing residue.
        mkdirat(&root, batch, Mode::from_bits_truncate(0o700))?;
        root.sync_all()?;
        let directory = directory_at(&root, OsStr::new(batch))?;
        for (entry, input) in allocation.files.iter().zip(files) {
            if entry.name != input.name {
                return Err(invalid("upload allocation mismatch"));
            }
            let mut output = File::from(openat(
                &directory,
                entry.name.as_str(),
                OFlag::O_WRONLY
                    | OFlag::O_CREAT
                    | OFlag::O_EXCL
                    | OFlag::O_NOFOLLOW
                    | OFlag::O_CLOEXEC,
                Mode::from_bits_truncate(0o600),
            )?);
            output.write_all(&input.bytes)?;
            #[cfg(test)]
            cap_std_validation::native_sync_checkpoint("file sync")?;
            output.sync_all()?;
        }
        #[cfg(test)]
        cap_std_validation::native_sync_checkpoint("directory sync")?;
        directory.sync_all()?;
        root.sync_all()?;
        // Re-open the declared path before readiness: an ancestor swap must not
        // turn an fd-relative write into a fabricated model-usable receipt.
        let reopened = session_directory(&allocation.workspace, session, false)?;
        let reopened = directory_at(&reopened, OsStr::new(batch))?;
        let expected = directory.metadata()?;
        let actual = reopened.metadata()?;
        if (expected.dev(), expected.ino()) != (actual.dev(), actual.ino()) {
            return Err(invalid("upload path changed during materialization"));
        }
        Ok(())
    }
    pub(crate) fn verify_materialized(&self, session: &SessionId, batch: &str) -> io::Result<()> {
        let allocation = self
            .allocations
            .get(batch)
            .ok_or_else(|| invalid("unclaimed upload"))?;
        let root = session_directory(&allocation.workspace, session, false)?;
        let directory = directory_at(&root, OsStr::new(batch))?;
        for entry in &allocation.files {
            let file = File::from(openat(
                &directory,
                entry.name.as_str(),
                OFlag::O_RDONLY | OFlag::O_NOFOLLOW | OFlag::O_NONBLOCK | OFlag::O_CLOEXEC,
                Mode::empty(),
            )?);
            if !file.metadata()?.is_file() {
                return Err(invalid("upload is not a regular file"));
            }
        }
        Ok(())
    }
    pub(crate) fn verify_all_materialized(&self, session: &SessionId) -> io::Result<()> {
        for batch in self.allocations.keys() {
            self.verify_materialized(session, batch)?;
        }
        Ok(())
    }
    pub(crate) fn receipt_ref(
        &self,
        session: &SessionId,
        receipt: &UploadReceipt,
    ) -> io::Result<UploadedFileRef> {
        if &receipt.session_id != session {
            return Err(invalid("upload receipt belongs to another Session"));
        }
        let allocation = self
            .allocations
            .get(&receipt.batch_id)
            .filter(|a| a.ready)
            .ok_or_else(|| invalid("upload is not committed"))?;
        let entry = allocation
            .files
            .iter()
            .find(|f| f.token == receipt.token)
            .ok_or_else(|| invalid("unknown upload receipt"))?;
        Ok(UploadedFileRef {
            batch_id: receipt.batch_id.clone(),
            name: entry.name.clone(),
        })
    }
    pub(crate) fn resolve(
        &self,
        session: &SessionId,
        reference: &UploadedFileRef,
    ) -> io::Result<PathBuf> {
        let allocation = self
            .allocations
            .get(&reference.batch_id)
            .filter(|a| a.ready)
            .ok_or_else(|| invalid("unknown upload allocation"))?;
        if !allocation.files.iter().any(|e| e.name == reference.name) {
            return Err(invalid("unknown uploaded file"));
        }
        Ok(file_path(
            &allocation.workspace,
            session,
            &reference.batch_id,
            &reference.name,
        ))
    }
    pub(crate) fn editor_input(
        &self,
        session: &SessionId,
        content: &[crate::message::types::UserContentBlock],
    ) -> io::Result<Vec<UserInputBlock>> {
        use crate::message::types::UserContentBlock;
        content
            .iter()
            .map(|block| match block {
                UserContentBlock::Text(text) => Ok(UserInputBlock::Text(text.clone())),
                UserContentBlock::UploadedFile(reference) => {
                    self.resolve(session, reference)?;
                    let entry = self.allocations[&reference.batch_id]
                        .files
                        .iter()
                        .find(|e| e.name == reference.name)
                        .expect("resolved upload");
                    Ok(UserInputBlock::Upload(UploadReceipt {
                        session_id: session.clone(),
                        batch_id: reference.batch_id.clone(),
                        token: entry.token.clone(),
                    }))
                }
                _ => Err(invalid("unsupported editor content")),
            })
            .collect()
    }
    pub(crate) fn receipts(
        &self,
        session: &SessionId,
        batch: &str,
    ) -> io::Result<Vec<UploadedFile>> {
        let allocation = &self.allocations[batch];
        allocation
            .files
            .iter()
            .map(|entry| {
                let receipt = UploadReceipt {
                    session_id: session.clone(),
                    batch_id: batch.into(),
                    token: entry.token.clone(),
                };
                let file = self.receipt_ref(session, &receipt)?;
                let path = self
                    .resolve(session, &file)?
                    .into_os_string()
                    .into_string()
                    .map_err(|_| invalid("workspace is not UTF-8"))?;
                Ok(UploadedFile {
                    receipt,
                    file,
                    path,
                })
            })
            .collect()
    }
}
/// Remove only an identity-derived Session allocation, through directory handles.
/// Symlinks within a root are unlinked, never traversed; ancestor symlinks fail closed.
pub(crate) fn cleanup(workspace: &Path, session: &SessionId) -> io::Result<()> {
    if !workspace.is_absolute() {
        return Err(invalid("cleanup workspace must be absolute"));
    }
    super::validate_id(session.as_str(), "upload Session").map_err(io::Error::other)?;
    // Missing residue still needs a barrier before deletion can be declared
    // durable. Walk to the nearest existing parent without following links.
    let mut current = File::from(open(
        Path::new("/"),
        OFlag::O_RDONLY | OFlag::O_DIRECTORY | OFlag::O_CLOEXEC,
        Mode::empty(),
    )?);
    for component in workspace.components() {
        match component {
            Component::RootDir => {}
            Component::Normal(name) => match directory_at(&current, name) {
                Ok(child) => current = child,
                Err(e) if e.kind() == io::ErrorKind::NotFound => return current.sync_all(),
                Err(e) => return Err(e),
            },
            _ => return Err(invalid("unsafe cleanup workspace")),
        }
    }
    for name in [".agents", "uploads"] {
        match directory_at(&current, OsStr::new(name)) {
            Ok(child) => current = child,
            Err(e) if e.kind() == io::ErrorKind::NotFound => return current.sync_all(),
            Err(e) => return Err(e),
        }
    }
    let owned = match directory_at(&current, OsStr::new(session.as_str())) {
        Ok(owned) => owned,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return current.sync_all(),
        Err(e) => return Err(e),
    };
    remove_contents(&owned)?;
    match unlinkat(&current, session.as_str(), UnlinkatFlags::RemoveDir) {
        Ok(()) | Err(nix::errno::Errno::ENOENT) => {}
        Err(e) => return Err(e.into()),
    }
    current.sync_all()
}

fn remove_contents(directory: &File) -> io::Result<()> {
    let mut entries = nix::dir::Dir::from_fd(directory.try_clone()?.into())?;
    let names = entries
        .iter()
        .map(|e| e.map(|e| e.file_name().to_owned()))
        .collect::<Result<Vec<_>, _>>()?;
    for name in names {
        if name.as_bytes() == b"." || name.as_bytes() == b".." {
            continue;
        }
        let name = OsStr::from_bytes(name.as_bytes());
        match directory_at(directory, name) {
            Ok(child) => {
                remove_contents(&child)?;
                unlinkat(directory, name, UnlinkatFlags::RemoveDir)?;
            }
            Err(e) if matches!(e.raw_os_error(), Some(libc::ENOTDIR | libc::ELOOP)) => {
                unlinkat(directory, name, UnlinkatFlags::NoRemoveDir)?;
            }
            Err(e) if e.kind() == io::ErrorKind::NotFound => {}
            Err(e) => return Err(e),
        }
    }
    directory.sync_all()
}
use std::os::unix::ffi::OsStrExt;

impl SessionCatalog {
    pub(crate) fn upload_registry(
        &self,
        session: &SessionId,
    ) -> Result<UploadRegistry, SessionError> {
        self.document
            .sessions
            .get(session)
            .map(|s| s.uploads.clone())
            .ok_or_else(|| SessionError::Catalog {
                detail: "upload Session is unavailable".into(),
            })
    }
    pub(crate) fn commit_uploads(
        &mut self,
        session: &SessionId,
        registry: UploadRegistry,
    ) -> Result<(), SessionError> {
        registry.validate().map_err(|e| SessionError::Catalog {
            detail: e.to_string(),
        })?;
        let mut next = self.document.clone();
        next.sessions
            .get_mut(session)
            .ok_or_else(|| SessionError::Catalog {
                detail: "upload Session is unavailable".into(),
            })?
            .uploads = registry;
        self.commit(next)
    }
}

impl UploadRegistry {
    /// Copy only canonical facts present in the prepared historical cut.
    pub(crate) fn copy_required(
        &self,
        source: &SessionId,
        destination: &SessionId,
        workspace: &Path,
        references: &[UploadedFileRef],
    ) -> io::Result<Self> {
        validate_workspace(workspace)?;
        let mut result = Self::default();
        for reference in references {
            self.resolve(source, reference)?;
            let original = &self.allocations[&reference.batch_id];
            let entry = original
                .files
                .iter()
                .find(|e| e.name == reference.name)
                .expect("resolved entry");
            let allocation = result
                .allocations
                .entry(reference.batch_id.clone())
                .or_insert_with(|| UploadAllocation {
                    workspace: workspace.to_path_buf(),
                    files: Vec::new(),
                    ready: false,
                });
            if !allocation.files.iter().any(|e| e.name == entry.name) {
                allocation.files.push(UploadEntry {
                    name: entry.name.clone(),
                    token: identity()?,
                });
            }
        }
        let copy = || -> io::Result<()> {
            if result.allocations.is_empty() {
                return Ok(());
            }
            let destination_root = session_directory(workspace, destination, true)?;
            for (batch, allocation) in &result.allocations {
                let original = &self.allocations[batch];
                let source_root = session_directory(&original.workspace, source, false)?;
                let source_batch = directory_at(&source_root, OsStr::new(batch))?;
                mkdirat(
                    &destination_root,
                    batch.as_str(),
                    Mode::from_bits_truncate(0o700),
                )?;
                destination_root.sync_all()?;
                let destination_batch = directory_at(&destination_root, OsStr::new(batch))?;
                for entry in &allocation.files {
                    let mut input = File::from(openat(
                        &source_batch,
                        entry.name.as_str(),
                        OFlag::O_RDONLY | OFlag::O_NOFOLLOW | OFlag::O_NONBLOCK | OFlag::O_CLOEXEC,
                        Mode::empty(),
                    )?);
                    if !input.metadata()?.is_file() {
                        return Err(invalid("required upload is not a regular file"));
                    }
                    let mut output = File::from(openat(
                        &destination_batch,
                        entry.name.as_str(),
                        OFlag::O_WRONLY
                            | OFlag::O_CREAT
                            | OFlag::O_EXCL
                            | OFlag::O_NOFOLLOW
                            | OFlag::O_CLOEXEC,
                        Mode::from_bits_truncate(0o600),
                    )?);
                    io::copy(&mut input, &mut output)?;
                    output.sync_all()?;
                }
                destination_batch.sync_all()?;
            }
            destination_root.sync_all()
        };
        copy()?;
        for allocation in result.allocations.values_mut() {
            allocation.ready = true;
        }
        Ok(result)
    }
}

/// Read-only resolution capability. The durable Session catalog owns lifetime;
/// cloning or dropping a runtime resolver never allocates or removes uploads.
#[derive(Debug, Clone)]
pub(crate) struct SessionUploadResolver {
    catalog: PathBuf,
    conversation: crate::runtime::identity::ConversationId,
}
impl SessionUploadResolver {
    pub(crate) fn new(root: &Path, conversation: crate::runtime::identity::ConversationId) -> Self {
        Self {
            catalog: root.join("sessions/catalog.json"),
            conversation,
        }
    }
}
impl crate::model::uploads::UploadProjectionResolver for SessionUploadResolver {
    fn resolve(
        &self,
        messages: &[crate::model::input::ModelInputMessage],
    ) -> Result<crate::model::uploads::UploadProjection, String> {
        use crate::message::types::{MessageBlock, UserContentBlock};
        let references = messages
            .iter()
            .filter_map(|m| m.as_canonical())
            .filter_map(|m| match m {
                MessageBlock::User(u) => Some(u),
                _ => None,
            })
            .flat_map(|u| u.content.iter())
            .filter_map(|b| match b {
                UserContentBlock::UploadedFile(f) => Some(f),
                _ => None,
            })
            .collect::<BTreeSet<_>>();
        if references.is_empty() {
            return Ok(crate::model::uploads::UploadProjection::default());
        }
        let document: super::CatalogDocument =
            serde_json::from_reader(File::open(&self.catalog).map_err(|e| e.to_string())?)
                .map_err(|e| e.to_string())?;
        if document.schema_version != super::SESSION_CATALOG_SCHEMA_VERSION {
            return Err("unsupported upload catalog".into());
        }
        let session = document
            .sessions
            .values()
            .find(|s| {
                s.nodes
                    .values()
                    .any(|n| n.conversation_id == self.conversation)
            })
            .ok_or("upload Session is unavailable")?;
        session.uploads.validate().map_err(|e| e.to_string())?;
        let files = references
            .into_iter()
            .map(|reference| {
                let path = session
                    .uploads
                    .resolve(&session.id, reference)
                    .map_err(|e| e.to_string())?;
                Ok(crate::model::uploads::ResolvedUpload {
                    file: reference.clone(),
                    path: path
                        .into_os_string()
                        .into_string()
                        .map_err(|_| "upload path is not UTF-8")?,
                })
            })
            .collect::<Result<_, String>>()?;
        Ok(crate::model::uploads::UploadProjection { files })
    }
}

impl SessionCatalog {
    pub(crate) fn discard_prepared_session(
        &self,
        prepared: &super::PreparedLineage,
    ) -> Result<(), SessionError> {
        self.discard_private_session_id(&prepared.session_id)
    }
    pub(crate) fn discard_prepared_node(
        &self,
        prepared: &super::PreparedLineage,
    ) -> Result<(), SessionError> {
        let session = self
            .document
            .sessions
            .get(&prepared.session_id)
            .ok_or_else(|| SessionError::UnknownSession {
                session_id: prepared.session_id.clone(),
            })?;
        if session
            .nodes
            .values()
            .any(|n| n.conversation_id == prepared.conversation_id)
        {
            return Err(SessionError::Catalog {
                detail: "cannot discard a published node".into(),
            });
        }
        let database = self.database_path(&prepared.session_id, &prepared.conversation_id);
        let directory = database.parent().expect("conversation parent");
        let directory = self
            .product
            .confined(directory)
            .map_err(|e| SessionError::Catalog {
                detail: e.to_string(),
            })?;
        match std::fs::remove_dir_all(&directory) {
            Ok(()) => {}
            Err(e) if e.kind() == io::ErrorKind::NotFound => {}
            Err(e) => {
                return Err(SessionError::Catalog {
                    detail: e.to_string(),
                });
            }
        }
        File::open(directory.parent().expect("conversations root"))
            .and_then(|f| f.sync_all())
            .map_err(|e| SessionError::Catalog {
                detail: e.to_string(),
            })
    }
    fn discard_private_session_id(&self, id: &SessionId) -> Result<(), SessionError> {
        if self.document.sessions.contains_key(id) {
            return Err(SessionError::Catalog {
                detail: "cannot discard a published Session".into(),
            });
        }
        let path = self
            .product
            .confined(&self.root.join(id.as_str()))
            .map_err(|e| SessionError::Catalog {
                detail: e.to_string(),
            })?;
        match std::fs::remove_dir_all(path) {
            Ok(()) => {}
            Err(e) if e.kind() == io::ErrorKind::NotFound => {}
            Err(e) => {
                return Err(SessionError::Catalog {
                    detail: e.to_string(),
                });
            }
        }
        File::open(&self.root)
            .and_then(|f| f.sync_all())
            .map_err(|e| SessionError::Catalog {
                detail: e.to_string(),
            })
    }
}
#[cfg(test)]
mod cap_std_validation;
#[cfg(test)]
mod tests;

impl SessionCatalog {
    pub(crate) fn claim_upload_preparation(
        &mut self,
        id: &SessionId,
        workspaces: &[PathBuf],
    ) -> Result<(), SessionError> {
        if self.document.sessions.contains_key(id)
            || self.document.upload_preparations.contains_key(id)
        {
            return Err(SessionError::Catalog {
                detail: "upload copy destination is already allocated".into(),
            });
        }
        let mut next = self.document.clone();
        next.upload_preparations
            .insert(id.clone(), workspaces.to_vec());
        self.commit(next)
    }
    pub(crate) fn finish_upload_preparation(&mut self, id: &SessionId) -> Result<(), SessionError> {
        if !self.document.upload_preparations.contains_key(id) {
            return Ok(());
        }
        let mut next = self.document.clone();
        next.upload_preparations.remove(id);
        self.commit(next)
    }
    /// Frozen workspace roots and the identity-derived private Session allocation
    /// form one cleanup workset. No metadata is consumed by this operation.
    pub(crate) fn cleanup_upload_preparation(&self, id: &SessionId) -> Result<(), SessionError> {
        #[cfg(test)]
        if self
            .preparation_cleanup_fault
            .load(std::sync::atomic::Ordering::SeqCst)
        {
            return Err(SessionError::Catalog {
                detail: "injected private upload cleanup failure".into(),
            });
        }
        let roots =
            self.document
                .upload_preparations
                .get(id)
                .ok_or_else(|| SessionError::Catalog {
                    detail: "missing private preparation authority".into(),
                })?;
        for workspace in roots {
            cleanup(workspace, id).map_err(|e| SessionError::Catalog {
                detail: e.to_string(),
            })?;
        }
        self.discard_private_session_id(id)
    }
    pub(crate) fn claim_upload_workspace(
        &mut self,
        id: &SessionId,
        workspace: &Path,
    ) -> Result<(), SessionError> {
        validate_workspace(workspace).map_err(|e| SessionError::Catalog {
            detail: e.to_string(),
        })?;
        let mut next = self.document.clone();
        let roots = next
            .upload_preparations
            .get_mut(id)
            .ok_or_else(|| SessionError::Catalog {
                detail: "missing private preparation authority".into(),
            })?;
        if !roots.contains(&workspace.to_path_buf()) {
            roots.push(workspace.to_path_buf());
        }
        self.commit(next)
    }
    /// Root admission retries exactly the frozen workset. Only successful cleanup
    /// permits the durable claim to be consumed.
    pub(crate) fn recover_upload_preparations(&mut self) -> Result<(), SessionError> {
        for id in self
            .document
            .upload_preparations
            .keys()
            .cloned()
            .collect::<Vec<_>>()
        {
            self.cleanup_upload_preparation(&id)?;
            self.finish_upload_preparation(&id)?;
        }
        Ok(())
    }
}

pub(super) fn validate_preparations(document: &super::CatalogDocument) -> Result<(), SessionError> {
    for (id, roots) in &document.upload_preparations {
        super::validate_id(id.as_str(), "prepared upload Session")?;
        if document.sessions.contains_key(id)
            || document.deletions.contains_key(id)
            || roots.iter().any(|root| {
                !root.is_absolute()
                    || root.components().any(|c| {
                        !matches!(
                            c,
                            std::path::Component::RootDir | std::path::Component::Normal(_)
                        )
                    })
            })
        {
            return Err(SessionError::Catalog {
                detail: "invalid upload preparation authority".into(),
            });
        }
    }
    Ok(())
}
