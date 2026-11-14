//! Definition semantics and real I/O boundaries, independent of asset size.
use super::{SkillDiscovery, SkillPackage};
use crate::runtime::CancellationSignal;
use std::io::Write;
use std::path::Path;

fn skill(root: &Path, text: impl AsRef<[u8]>) -> SkillPackage {
    std::fs::create_dir_all(root.join("guide")).unwrap();
    std::fs::write(root.join("guide/SKILL.md"), text).unwrap();
    let outcome = SkillDiscovery::user_root(root.to_path_buf()).discover();
    assert!(outcome.diagnostics.is_empty(), "{:?}", outcome.diagnostics);
    outcome.packages.into_iter().next().unwrap()
}
const HEADER: &str = "---\nname: guide\ndescription: Guide\n---\n";

#[test]
fn definition_identity_normalizes_yaml_and_includes_all_standard_fields() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().canonicalize().unwrap();
    let first = skill(&root, HEADER);
    let formatted = skill(
        &root,
        "---\r\n# formatting\r\ndescription: 'Guide'\r\nname: guide\r\ndisable-model-invocation: false\r\n---\r\nbody",
    );
    assert_eq!(first.version_id(), formatted.version_id());
    let with_metadata = skill(
        &root,
        "---\nname: guide\ndescription: Guide\nmetadata: {z: b, a: c}\n---\n",
    );
    let reordered = skill(
        &root,
        "---\nmetadata: {a: c, z: b}\nname: guide\ndescription: Guide\n---\n",
    );
    assert_eq!(with_metadata.version_id(), reordered.version_id());
    for field in [
        "description: Changed",
        "license: MIT",
        "compatibility: Python 3",
        "allowed-tools: Read Bash",
        "disable-model-invocation: true",
        "metadata: {author: Someone}",
        r#"metadata: {rustx.python-dependencies: '{"requests":"2.0"}'}"#,
    ] {
        let header = if field.starts_with("description:") {
            HEADER.replace("description: Guide", field)
        } else {
            HEADER.replace(
                "description: Guide",
                &format!("description: Guide\n{field}"),
            )
        };
        let changed = skill(&root, header);
        assert_ne!(first.version_id(), changed.version_id(), "{field}");
    }
}

#[test]
fn dependency_metadata_is_uninterpreted_frontmatter() {
    let dir = tempfile::tempdir().unwrap();
    let one = skill(
        dir.path(),
        "---\nname: guide\ndescription: Guide\nmetadata:\n  rustx.python-dependencies: '{\"PyPDF\":\"1.0\", \"Pillow\": \"2.0\"}'\n  rustx.node-dependencies: not-a-manifest\n---\n",
    );
    let two = skill(
        dir.path(),
        "---\nname: guide\ndescription: Guide\nmetadata:\n  rustx.python-dependencies: '{\"pillow\":\"2.0\",\"pypdf\":\"1.0\"}'\n  rustx.node-dependencies: not-a-manifest\n---\n",
    );
    assert_eq!(
        one.metadata()["rustx.python-dependencies"].as_str(),
        Some(r#"{"PyPDF":"1.0", "Pillow": "2.0"}"#)
    );
    assert_eq!(
        one.metadata()["rustx.node-dependencies"].as_str(),
        Some("not-a-manifest")
    );
    assert_ne!(
        one.version_id(),
        two.version_id(),
        "uninterpreted metadata keeps its literal text in the definition"
    );
}

#[test]
fn bodies_assets_and_machine_paths_are_not_definition_identity() {
    let one = tempfile::tempdir().unwrap();
    let two = tempfile::tempdir().unwrap();
    let a = skill(one.path(), format!("{HEADER}first body"));
    let b = skill(two.path(), format!("{HEADER}second body"));
    assert_eq!(a.version_id(), b.version_id());
    assert_ne!(a.location(), b.location());
    std::fs::create_dir(one.path().join("guide/assets")).unwrap();
    std::fs::write(one.path().join("guide/assets/changed.bin"), [0xff, 0, 1]).unwrap();
    let mut bytes = HEADER.as_bytes().to_vec();
    bytes.extend_from_slice(&[0xff, 0xfe]);
    assert_eq!(
        a.version_id(),
        skill(one.path(), bytes).version_id(),
        "body UTF-8 is checked only by runtime Tools"
    );
}

#[test]
fn sparse_gigabytes_and_thousands_of_assets_cost_only_bounded_header_io() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().canonicalize().unwrap();
    let first = skill(&root, HEADER);
    let package = root.join("guide");
    let asset = std::fs::File::create(package.join("asset.bin")).unwrap();
    asset.set_len(10 * 1024 * 1024 * 1024).unwrap();
    let body = std::fs::OpenOptions::new()
        .write(true)
        .open(package.join("SKILL.md"))
        .unwrap();
    body.set_len(4 * 1024 * 1024 * 1024).unwrap();
    let nested = package.join("assets/deep");
    std::fs::create_dir_all(&nested).unwrap();
    for index in 0..4200 {
        std::fs::write(nested.join(format!("{index}.bin")), []).unwrap();
    }
    std::os::unix::fs::symlink("/nonexistent/runtime-only", package.join("references")).unwrap();
    crate::package_io::OPENED_FILES.with(|v| v.borrow_mut().clear());
    crate::package_io::DIRECTORY_LISTS.with(|v| v.set(0));
    super::frontmatter::READ_BYTES.with(|v| v.set(0));
    for _ in 0..2 {
        let discovered = SkillDiscovery::user_root(root.clone()).discover();
        assert!(
            discovered.diagnostics.is_empty(),
            "{:?}",
            discovered.diagnostics
        );
        assert_eq!(discovered.packages[0].version_id(), first.version_id());
    }
    crate::package_io::OPENED_FILES
        .with(|v| assert_eq!(*v.borrow(), vec![Path::new("SKILL.md"); 4]));
    crate::package_io::DIRECTORY_LISTS.with(|v| assert_eq!(v.get(), 0));
    super::frontmatter::READ_BYTES.with(|v| assert_eq!(v.get(), 4 * 4096));
}

#[test]
fn malformed_prefixes_are_diagnosed_and_large_definitions_are_loaded() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().canonicalize().unwrap();
    std::fs::create_dir(root.join("guide")).unwrap();
    for header in ["not YAML", "---\nname: guide\n", "---\nname: [\n---\n"] {
        std::fs::write(root.join("guide/SKILL.md"), header).unwrap();
        let outcome = SkillDiscovery::user_root(root.clone()).discover();
        assert!(outcome.packages.is_empty());
        assert_eq!(outcome.diagnostics.len(), 1);
    }
    let note = "x".repeat(1024 * 1024 + 1);
    std::fs::write(
        root.join("guide/SKILL.md"),
        format!("\u{feff}---\rname: guide\rdescription: Guide\rmetadata:\r  note: {note}\r---\r"),
    )
    .unwrap();
    let outcome = SkillDiscovery::user_root(root.clone()).discover();
    assert_eq!(
        outcome.packages[0].metadata()["note"].as_str(),
        Some(note.as_str())
    );
    assert!(outcome.diagnostics.is_empty());
    let signal = CancellationSignal::new();
    signal.cancel();
    super::frontmatter::READ_BYTES.with(|v| v.set(0));
    assert!(
        super::frontmatter::read(&root.join("guide/SKILL.md"), &signal)
            .unwrap_err()
            .contains("cancelled")
    );
    super::frontmatter::READ_BYTES.with(|v| assert_eq!(v.get(), 0));
}

#[test]
fn unterminated_frontmatter_stops_at_the_prefix_budget() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().canonicalize().unwrap().join("SKILL.md");
    let mut file = std::fs::File::create(&path).unwrap();
    file.write_all(b"---\nname: guide\ndescription: ").unwrap();
    file.set_len(64 * 1024 * 1024).unwrap();
    super::frontmatter::READ_BYTES.with(|count| count.set(0));
    let error = super::frontmatter::read(&path, &CancellationSignal::new()).unwrap_err();
    assert!(error.contains("frontmatter exceeds"), "{error}");
    super::frontmatter::READ_BYTES.with(|count| {
        let limit = super::frontmatter::MAX_FRONTMATTER_BYTES as u64;
        assert!((limit..=limit + 4096).contains(&count.get()));
    });
}

#[test]
fn prefix_cancellation_and_definition_replacement_use_deterministic_io_hooks() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().canonicalize().unwrap();
    skill(&root, HEADER);
    let path = root.join("guide/SKILL.md");
    let replacement = path.clone();
    super::frontmatter::AFTER_PREFIX.with(|hook| {
        *hook.borrow_mut() = Some(Box::new(move || {
            std::fs::write(
                replacement.with_extension("next"),
                HEADER.replace("Guide", "Successor"),
            )
            .unwrap();
            std::fs::rename(replacement.with_extension("next"), replacement).unwrap();
        }));
    });
    assert!(
        super::frontmatter::read(&root.join("guide/SKILL.md"), &CancellationSignal::new())
            .unwrap_err()
            .contains("changed during")
    );
    let mut file = std::fs::File::create(path).unwrap();
    file.write_all(b"---\nname: guide\ndescription: ").unwrap();
    file.set_len(4 * 1024 * 1024 * 1024).unwrap();
    let signal = CancellationSignal::new();
    let cancel = signal.clone();
    super::frontmatter::ON_CHUNK
        .with(|hook| *hook.borrow_mut() = Some(Box::new(move || cancel.cancel())));
    super::frontmatter::READ_BYTES.with(|count| count.set(0));
    assert!(
        super::frontmatter::read(&root.join("guide/SKILL.md"), &signal)
            .unwrap_err()
            .contains("cancelled")
    );
    super::frontmatter::READ_BYTES.with(|count| assert_eq!(count.get(), 4096));
}
