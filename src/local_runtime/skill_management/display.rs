//! Optional skill presentation metadata, read only for the management catalog.
use base64::Engine as _;
use serde::{Deserialize, Serialize};
use sha2::{Digest as _, Sha256};
use std::ffi::OsStr;
use std::io::{self, Read};
use std::path::{Component, Path, PathBuf};

use crate::local_runtime::configuration::{
    UserConfigManager,
    observation::digest,
    settings::{SettingsError, SourceTarget},
};
use crate::local_runtime::settings::{SourceDirectory, revision};
use crate::package_io::PackageDirectory;
use crate::runtime::resources::SourceScope;
use crate::skills::{SkillPackage, package::SKILL_MARKDOWN_FILE};

const MAX_METADATA_BYTES: u64 = 16 * 1024;
const MAX_ICON_BYTES: u64 = 32 * 1024;

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SkillDisplayWrite {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub display_name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub short_description: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub icon_small: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub icon_large: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub icon: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct SkillDisplay {
    pub authored: SkillDisplayWrite,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub icon: Option<SkillDisplayIcon>,
    /// An exact write fence when this package has an editable sidecar.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub revision: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum SkillDisplayIcon {
    Asset {
        media_type: SkillIconMediaType,
        data_base64: String,
    },
    Iconify {
        name: String,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
pub enum SkillIconMediaType {
    #[serde(rename = "image/png")]
    Png,
    #[serde(rename = "image/jpeg")]
    Jpeg,
    #[serde(rename = "image/webp")]
    Webp,
    #[serde(rename = "image/svg+xml")]
    Svg,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SkillIconUpload {
    pub media_type: SkillIconMediaType,
    pub data_base64: String,
}

const SIDECAR: &str = "agents/openai.yaml";
const INTERFACE: &str = "interface";
const FIELDS: [(&str, usize); 5] = [
    ("display_name", 128),
    ("short_description", 1024),
    ("icon_small", 1024),
    ("icon_large", 1024),
    ("icon", 128),
];

fn read(directory: &PackageDirectory, relative: &Path, limit: u64) -> io::Result<Option<Vec<u8>>> {
    let file = match directory.file(relative) {
        Ok(file) => file,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error),
    };
    let mut bytes = Vec::new();
    file.take(limit + 1).read_to_end(&mut bytes)?;
    if bytes.len() as u64 > limit {
        return Err(io::Error::other("Skill display file exceeds size limit"));
    }
    Ok(Some(bytes))
}

fn asset_path(value: &str) -> Option<&Path> {
    let path = Path::new(value);
    let path = path.strip_prefix(".").unwrap_or(path);
    (path.starts_with("assets")
        && path
            .components()
            .all(|part| matches!(part, Component::Normal(_))))
    .then_some(path)
}

fn asset(directory: &PackageDirectory, value: &str) -> Option<SkillDisplayIcon> {
    let path = asset_path(value)?;
    let media_type = match path.extension()?.to_str()?.to_ascii_lowercase().as_str() {
        "png" => SkillIconMediaType::Png,
        "jpg" | "jpeg" => SkillIconMediaType::Jpeg,
        "webp" => SkillIconMediaType::Webp,
        "svg" => SkillIconMediaType::Svg,
        _ => return None,
    };
    let bytes = read(directory, path, MAX_ICON_BYTES).ok()??;
    media_type.matches(&bytes).then(|| SkillDisplayIcon::Asset {
        media_type,
        data_base64: base64::engine::general_purpose::STANDARD.encode(bytes),
    })
}

impl SkillIconMediaType {
    fn matches(&self, bytes: &[u8]) -> bool {
        match self {
            Self::Png => bytes.starts_with(b"\x89PNG\r\n\x1a\n"),
            Self::Jpeg => bytes.starts_with(b"\xff\xd8\xff"),
            Self::Webp => bytes.starts_with(b"RIFF") && bytes.get(8..12) == Some(b"WEBP"),
            Self::Svg => std::str::from_utf8(bytes).is_ok_and(|xml| xml.contains("<svg")),
        }
    }

    const fn extension(&self) -> &'static str {
        match self {
            Self::Png => "png",
            Self::Jpeg => "jpg",
            Self::Webp => "webp",
            Self::Svg => "svg",
        }
    }
}

impl SkillIconUpload {
    fn decode(&self) -> Result<(String, Vec<u8>), SettingsError> {
        if self.data_base64.len() as u64 > MAX_ICON_BYTES.div_ceil(3) * 4 {
            return Err(SettingsError::Invalid);
        }
        let bytes = base64::engine::general_purpose::STANDARD
            .decode(&self.data_base64)
            .map_err(|_| SettingsError::Invalid)?;
        if bytes.len() as u64 > MAX_ICON_BYTES || !self.media_type.matches(&bytes) {
            return Err(SettingsError::Invalid);
        }
        let name = format!(
            "icon-{:x}.{}",
            Sha256::digest(&bytes),
            self.media_type.extension()
        );
        Ok((name, bytes))
    }
}

fn publish_asset(root: &Path, name: &str, bytes: &[u8]) -> Result<SourceDirectory, SettingsError> {
    let directory = SourceDirectory::create_directory(&root.join("assets"))
        .map_err(|_| SettingsError::Io)?
        .with_byte_limit(MAX_ICON_BYTES);
    let name = OsStr::new(name);
    directory.confirm().map_err(|_| SettingsError::Io)?;
    // Publish an immutable asset before the sidecar can point to it. Identical
    // uploads reuse the same file, and a failed sidecar write leaves its old icon usable.
    let existing = directory.read(name).map_err(|_| SettingsError::Io)?;
    if existing.is_none() {
        let staged = directory.stage(bytes).map_err(|_| SettingsError::Io)?;
        match directory.link(&staged, name) {
            Ok(()) => {}
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {}
            Err(_) => return Err(SettingsError::Io),
        }
    }
    if directory
        .read(name)
        .map_err(|_| SettingsError::Io)?
        .as_deref()
        != Some(bytes)
    {
        return Err(SettingsError::Invalid);
    }
    directory.sync().map_err(|_| SettingsError::Io)?;
    Ok(directory)
}

fn iconify(value: &str) -> Option<SkillDisplayIcon> {
    // The web console ships the Lucide set with the Workflow icon picker.
    let name = value.strip_prefix("lucide:")?;
    (!name.is_empty()
        && name
            .bytes()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-'))
    .then(|| SkillDisplayIcon::Iconify { name: value.into() })
}

fn package_root(package: &SkillPackage) -> Option<PathBuf> {
    let location = Path::new(package.location());
    if location.file_name()? != SKILL_MARKDOWN_FILE
        || location.canonicalize().ok()? != package.instruction_source()
    {
        return None;
    }
    location.parent()?.canonicalize().ok()
}

fn editable_root(package: &SkillPackage, root: &Path, collection: &Path) -> bool {
    let location = Path::new(package.location());
    location == package.instruction_source()
        && location.parent() == Some(root)
        && root.starts_with(collection)
}

fn document(bytes: Option<&[u8]>) -> Option<serde_yaml::Mapping> {
    let Some(bytes) = bytes else {
        return Some(serde_yaml::Mapping::new());
    };
    let value: serde_yaml::Mapping = serde_yaml::from_slice(bytes).ok()?;
    if value
        .get(INTERFACE)
        .is_some_and(|value| !value.is_mapping())
    {
        return None;
    }
    Some(value)
}

fn display_revision(package: &SkillPackage, bytes: Option<&[u8]>) -> Result<String, SettingsError> {
    digest(&(
        package.location(),
        package.instruction_source(),
        revision(bytes),
    ))
    .map_err(|_| SettingsError::Io)
}

pub(super) fn read_display(package: &SkillPackage, collection: &Path) -> Option<SkillDisplay> {
    let root = package_root(package)?;
    let directory = PackageDirectory::open(&root).ok()?;
    let bytes = read(&directory, Path::new(SIDECAR), MAX_METADATA_BYTES).ok()?;
    let document = document(bytes.as_deref())?;
    let interface = document
        .get(INTERFACE)
        .and_then(serde_yaml::Value::as_mapping);
    let get = |(key, limit)| {
        let value = interface?.get(key)?.as_str()?.trim();
        (!value.is_empty() && value.len() <= limit).then(|| value.to_owned())
    };
    let [
        display_name,
        short_description,
        icon_small,
        icon_large,
        icon,
    ] = FIELDS.map(get);
    let authored = SkillDisplayWrite {
        display_name,
        short_description,
        icon_small,
        icon_large,
        icon,
    };
    let icon = [
        authored.icon_small.as_deref(),
        authored.icon_large.as_deref(),
    ]
    .into_iter()
    .flatten()
    .find_map(|value| asset(&directory, value))
    .or_else(|| iconify(authored.icon.as_deref()?));
    let revision = if editable_root(package, &root, collection) {
        Some(display_revision(package, bytes.as_deref()).ok()?)
    } else {
        None
    };
    (revision.is_some() || authored != SkillDisplayWrite::default()).then_some(SkillDisplay {
        authored,
        icon,
        revision,
    })
}

fn candidate(
    original: Option<&[u8]>,
    authored: SkillDisplayWrite,
) -> Result<Vec<u8>, SettingsError> {
    let mut document = document(original).ok_or(SettingsError::Invalid)?;
    let interface = document
        .entry(INTERFACE.into())
        .or_insert_with(|| serde_yaml::Value::Mapping(serde_yaml::Mapping::new()))
        .as_mapping_mut()
        .ok_or(SettingsError::Invalid)?;
    for ((key, limit), value) in FIELDS.into_iter().zip([
        authored.display_name,
        authored.short_description,
        authored.icon_small,
        authored.icon_large,
        authored.icon,
    ]) {
        if let Some(value) = value {
            let value = value.trim();
            if value.is_empty()
                || value.len() > limit
                || (key == "icon" && iconify(value).is_none())
                || (matches!(key, "icon_small" | "icon_large") && asset_path(value).is_none())
            {
                return Err(SettingsError::Invalid);
            }
            interface.insert(key.into(), value.into());
        } else {
            interface.remove(key);
        }
    }
    let bytes = serde_yaml::to_string(&document)
        .map_err(|_| SettingsError::Invalid)?
        .into_bytes();
    if bytes.len() as u64 > MAX_METADATA_BYTES {
        return Err(SettingsError::Invalid);
    }
    Ok(bytes)
}

impl UserConfigManager {
    pub(in crate::local_runtime) fn write_skill_display(
        &self,
        target: &SourceTarget,
        name: &str,
        expected: &str,
        mut authored: SkillDisplayWrite,
        upload: Option<&SkillIconUpload>,
    ) -> Result<String, SettingsError> {
        let discovered = self
            .discover_skills(target.workspace())
            .ok_or(SettingsError::Invalid)?;
        let package = discovered
            .packages
            .iter()
            .find(|package| {
                package.name() == name
                    && matches!(
                        (package.source(), target.scope()),
                        (crate::skills::SkillSource::User, SourceScope::User)
                            | (
                                crate::skills::SkillSource::Workspace,
                                SourceScope::Workspace
                            )
                    )
            })
            .ok_or(SettingsError::Invalid)?;
        let root = package_root(package).ok_or(SettingsError::Invalid)?;
        let collection = self.resource_root(target).join(crate::skills::SKILLS_ROOT);
        if !editable_root(package, &root, &collection) {
            return Err(SettingsError::Invalid);
        }
        let path = root.join(SIDECAR);
        let file = path.file_name().ok_or(SettingsError::Invalid)?;
        let directory = SourceDirectory::create(&path)
            .map_err(|_| SettingsError::Io)?
            .with_byte_limit(MAX_METADATA_BYTES);
        let _lock = directory.lock(file).map_err(|_| SettingsError::Io)?;
        let original = directory.read(file).map_err(|_| SettingsError::Io)?;
        let actual = display_revision(package, original.as_deref())?;
        if actual != expected {
            return Err(SettingsError::Conflict {
                scope: target.scope(),
                expected: expected.into(),
                actual,
            });
        }
        let asset = upload.map(SkillIconUpload::decode).transpose()?;
        if let Some((name, _)) = &asset {
            authored.icon_small = Some(format!("./assets/{name}"));
            authored.icon_large = None;
            authored.icon = None;
        }
        let bytes = candidate(original.as_deref(), authored)?;
        if package_root(package).as_deref() != Some(&root) {
            return Err(SettingsError::Invalid);
        }
        if let Some((name, bytes)) = &asset {
            let assets = publish_asset(&root, name, bytes)?;
            if !assets.holds() || !directory.holds() {
                return Err(SettingsError::Invalid);
            }
        }
        self.publish_locked(
            &directory,
            file,
            target.scope(),
            &revision(original.as_deref()),
            Some(&bytes),
            original.is_some(),
        )?;
        display_revision(package, Some(&bytes))
    }
}
